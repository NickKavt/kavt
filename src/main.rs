//! Budget: a self-hosted family budget for the browser, and optionally a Telegram Mini App.
//!
//! Every person has their own budget; data is shared only between people who added each other.
//! `budget-server health` checks a running server (used by the Docker health check);
//! `reset-password <login>` and `reset-2fa <login>` help someone locked out.

mod auth;
mod db;
mod limit;
mod local;
mod import;
mod migrate;
mod rates;
mod recurring;
mod savings;
mod totp;

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceBuilder;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use crate::{auth::CurrentUser, limit::Limiter};

// ---------------------------------------------------------------------------
// Config, state, errors
// ---------------------------------------------------------------------------
pub struct Config {
    pub bot_token: String,
    pub owner_telegram_id: Option<String>,
    /// Whether someone nobody invited may start a budget of their own (`BUDGET_SIGNUP=open`).
    /// With `invite`, only the owner and the people added in the app can sign in.
    pub open_signup: bool,
    pub session_ttl: i64,
    /// `Some` when `BUDGET_SESSION_COOKIE_SECURE` says; `None` (auto) marks the cookie HTTPS-only
    /// when the proxy in front says the request came over HTTPS.
    pub cookie_secure: Option<bool>,
    pub max_sessions: i64,
    pub init_data_ttl: i64,
    pub init_data_future_skew: i64,
    pub static_dir: String,
    /// Where users find the source of the code they are running (AGPL section 13). A modified
    /// copy must point it at its own source with `BUDGET_SOURCE_URL`.
    pub source_url: String,
    /// The donation ways shown in Settings, as JSON for the page.
    pub donate: serde_json::Value,
}

/// Where the author of the app takes donations. An installation shows them in Settings unless
/// its own `BUDGET_DONATE_*` settings change them; set to empty, a way is hidden.
const DONATE_TON: &str = "";
const DONATE_GITHUB: &str = "";
const DONATE_URL: &str = "";

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// A setting that is present but empty turns the way off; an absent one keeps the author's.
fn env_or_default(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.into()).trim().to_string()
}

fn donate_from_env() -> serde_json::Value {
    let ton = env_or_default("BUDGET_DONATE_TON", DONATE_TON);
    let github = env_or_default("BUDGET_DONATE_GITHUB", DONATE_GITHUB);
    let url = env_or_default("BUDGET_DONATE_URL", DONATE_URL);
    // Only plain values reach the page: an address, a GitHub login and an https link.
    let ton_ok = (40..=80).contains(&ton.len()) && ton.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':');
    let github_ok = !github.is_empty() && github.len() <= 39 && github.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let url_ok = url.starts_with("https://") && !url.chars().any(char::is_whitespace);
    for (name, value, ok) in [("BUDGET_DONATE_TON", &ton, ton_ok), ("BUDGET_DONATE_GITHUB", &github, github_ok), ("BUDGET_DONATE_URL", &url, url_ok)] {
        if !value.is_empty() && !ok {
            tracing::warn!("{name} is not valid and is not shown");
        }
    }
    json!({
        "ton": if ton_ok { ton.as_str() } else { "" },
        "github": if github_ok { github.as_str() } else { "" },
        "url": if url_ok { url.as_str() } else { "" },
    })
}

impl Config {
    /// Whether the session cookie must be HTTPS-only for this request. On plain HTTP (a home
    /// network) such a cookie would never be stored, and signing in would silently fail.
    pub fn secure_cookie(&self, headers: &HeaderMap) -> bool {
        self.cookie_secure.unwrap_or_else(|| {
            headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
        })
    }

    fn from_env() -> Self {
        Self {
            bot_token: env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default(),
            owner_telegram_id: env::var("OWNER_TELEGRAM_ID").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
            open_signup: env::var("BUDGET_SIGNUP").is_ok_and(|v| v.trim().eq_ignore_ascii_case("open")),
            // A month: the app is opened from the home screen and shouldn't ask for the password often.
            session_ttl: env_or("BUDGET_SESSION_TTL_SECONDS", 30 * 86_400),
            cookie_secure: env::var("BUDGET_SESSION_COOKIE_SECURE").ok().and_then(|v| match v.trim() {
                "1" | "true" | "yes" => Some(true),
                "0" | "false" | "no" => Some(false),
                _ => None,
            }),
            max_sessions: env_or("BUDGET_MAX_ACTIVE_SESSIONS_PER_USER", 5),
            init_data_ttl: env_or("TELEGRAM_INIT_DATA_TTL_SECONDS", 86_400),
            init_data_future_skew: env_or("TELEGRAM_INIT_DATA_FUTURE_SKEW_SECONDS", 300),
            static_dir: env::var("BUDGET_STATIC_DIR").unwrap_or_else(|_| "app/static".into()),
            donate: donate_from_env(),
            source_url: env::var("BUDGET_SOURCE_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| env!("CARGO_PKG_REPOSITORY").into()),
        }
    }
}

pub struct AppState {
    pub db: PgPool,
    pub cfg: Config,
    /// Fingerprint of the new interface's files; it is part of their addresses, so a
    /// release is never mixed with files cached from an older one.
    pub v2_version: String,
    pub rates: rates::Cache,
}

type AppRef = State<Arc<AppState>>;

#[derive(Debug)]
pub enum ApiError {
    Status(StatusCode, String),
    Db(sqlx::Error),
}

impl ApiError {
    pub fn status(code: u16, msg: &str) -> Self {
        ApiError::Status(StatusCode::from_u16(code).unwrap(), msg.to_string())
    }
    fn internal(msg: &str) -> Self {
        Self::status(500, msg)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Db(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, text) = match self {
            ApiError::Status(code, text) => (code, text),
            ApiError::Db(sqlx::Error::RowNotFound) => (StatusCode::INTERNAL_SERVER_ERROR, "no rows".to_string()),
            ApiError::Db(sqlx::Error::Database(e)) => {
                let detail = e
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .and_then(|pg| pg.detail().map(|d| format!("\nDETAIL: {d}")))
                    .unwrap_or_default();
                (StatusCode::INTERNAL_SERVER_ERROR, format!("db error → ERROR: {}{detail}", e.message()))
            }
            ApiError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("db error → {e}"))
            }
        };
        if code.is_server_error() {
            tracing::warn!(%code, %text, "request failed");
        }
        (code, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn ok() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn user(state: &AppState, headers: &HeaderMap) -> ApiResult<CurrentUser> {
    auth::current_user(&state.db, headers).await
}

async fn admin(state: &AppState, headers: &HeaderMap) -> ApiResult<CurrentUser> {
    let me = user(state, headers).await?;
    if !me.is_admin() {
        return Err(ApiError::status(403, "Недостаточно прав"));
    }
    Ok(me)
}

fn not_found_category() -> ApiError {
    ApiError::status(404, "Категория не найдена")
}

#[derive(Deserialize)]
struct MonthQuery {
    month: Option<NaiveDate>,
}

impl MonthQuery {
    fn bounds(&self) -> (String, String) {
        db::month_bounds(self.month.unwrap_or_else(|| Utc::now().date_naive()))
    }
}

// ---------------------------------------------------------------------------
// Service and auth
// ---------------------------------------------------------------------------
async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "time": Utc::now().to_rfc3339() }))
}

async fn config() -> Json<serde_json::Value> {
    Json(json!({ "name": env!("CARGO_PKG_NAME"), "version": env!("CARGO_PKG_VERSION") }))
}

#[derive(Deserialize)]
struct TelegramAuthIn {
    init_data: String,
}

async fn auth_telegram(
    State(state): AppRef,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<TelegramAuthIn>,
) -> ApiResult<Response> {
    let tg = auth::validate_init_data(&input.init_data, &state.cfg)?;
    let ip = limit::client_ip(&headers, Some(addr));
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let (user_id, token) = auth::login(&state.db, &state.cfg, &tg, &ip, ua).await?;
    let user = db::user_by_id(&state.db, user_id).await?.ok_or_else(|| ApiError::status(404, "Пользователь не найден"))?;
    let cookie = auth::session_cookie(&token, state.cfg.session_ttl, state.cfg.secure_cookie(&headers));
    Ok(([(header::SET_COOKIE, cookie)], Json(user)).into_response())
}

async fn logout(State(state): AppRef, headers: HeaderMap) -> ApiResult<Response> {
    let me = user(&state, &headers).await?;
    sqlx::query("DELETE FROM sessions WHERE token = $1").bind(&me.token).execute(&state.db).await?;
    let cookie = auth::session_cookie("", 0, state.cfg.secure_cookie(&headers));
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

async fn revoke_others(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token != $2")
        .bind(me.id).bind(&me.token)
        .execute(&state.db)
        .await?;
    Ok(ok())
}

async fn me(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let (login, has_password) = local::has_password(&state, me.id).await?;
    let name: Option<String> = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(me.id).fetch_one(&state.db).await?;
    Ok(Json(json!({
        "id": me.id, "telegram_id": me.telegram_id, "username": null, "first_name": name,
        "last_name": null, "language_code": me.language_code, "role": me.role,
        "login": login, "has_password": has_password, "totp": totp::enabled(&state.db, me.id).await?,
    })))
}

async fn list_sessions(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let me = user(&state, &headers).await?;
    let rows: Vec<(String, String, Option<String>, Option<String>, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT token, COALESCE(token_suffix, ''), ip_address, user_agent, created_at, last_used_at, expires_at
         FROM sessions
         WHERE user_id = $1 AND expires_at > $2
         ORDER BY last_used_at DESC, created_at DESC",
    )
    .bind(me.id)
    .bind(Utc::now().timestamp())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(token, suffix, ip, ua, created, used, expires)| {
                json!({
                    "token_suffix": suffix, "ip_address": ip, "user_agent": ua, "created_at": created,
                    "last_used_at": used,
                    "expires_at": chrono::DateTime::from_timestamp(expires, 0).map(|d| d.to_rfc3339()),
                    "is_current": token == me.token,
                })
            })
            .collect(),
    ))
}

async fn delete_session(State(state): AppRef, headers: HeaderMap, Path(suffix): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let done = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_suffix = $2")
        .bind(me.id).bind(suffix)
        .execute(&state.db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::status(404, "Сессия не найдена"));
    }
    Ok(ok())
}

async fn perf_startup(body: axum::body::Bytes) -> Json<serde_json::Value> {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
        tracing::info!(target: "budget_perf", beacon = %v, "startup perf beacon");
    }
    ok()
}

// ---------------------------------------------------------------------------
// Home, categories, statistics
// ---------------------------------------------------------------------------
async fn home(State(state): AppRef, headers: HeaderMap, Query(q): Query<MonthQuery>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    let month = q.bounds();
    // Recurring operations and interest due are in before the month's totals are counted.
    recurring::sync_budget(&state.db, budget).await?;
    savings::sync_budget(&state.db, budget).await?;
    Ok(Json(json!({
        "categories": db::list_categories(&state.db, budget, Some(&month)).await?,
        "chart": db::stats_by_category(&state.db, budget, &month).await?,
        "month_summary": db::month_summary(&state.db, budget, &month).await?,
        "savings_summary": db::savings_summary(&state.db, budget, &month.1).await?,
        "settings": rates::settings(&state.db, budget).await?,
    })))
}

async fn list_categories(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<db::CategoryOut>>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    Ok(Json(db::list_categories(&state.db, budget, None).await?))
}

async fn stats(State(state): AppRef, headers: HeaderMap, Query(q): Query<MonthQuery>) -> ApiResult<Json<Vec<db::ChartRow>>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    Ok(Json(db::stats_by_category(&state.db, budget, &q.bounds()).await?))
}

async fn month_summary(State(state): AppRef, headers: HeaderMap, Query(q): Query<MonthQuery>) -> ApiResult<Json<db::MonthSummary>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    Ok(Json(db::month_summary(&state.db, budget, &q.bounds()).await?))
}

async fn savings_summary(State(state): AppRef, headers: HeaderMap, Query(q): Query<MonthQuery>) -> ApiResult<Json<Option<db::SavingsSummary>>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    Ok(Json(db::savings_summary(&state.db, budget, &q.bounds().1).await?))
}

#[derive(Deserialize)]
struct ReportQuery {
    from: NaiveDate,
    to: NaiveDate,
}

/// Operations of any period, both ends included, for the reports screen.
async fn report(State(state): AppRef, headers: HeaderMap, Query(q): Query<ReportQuery>) -> ApiResult<Json<db::Report>> {
    let me = user(&state, &headers).await?;
    if q.from > q.to {
        return Err(ApiError::status(400, "Начало периода позже конца"));
    }
    if (q.to - q.from).num_days() > 3700 {
        return Err(ApiError::status(400, "Период больше 10 лет"));
    }
    Ok(Json(db::report(&state.db, me.budget_owner_id, q.from, q.to).await?))
}

async fn category_details(
    State(state): AppRef,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<MonthQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    let month = q.bounds();
    let cat = db::root_category_row(&state.db, budget, id).await?.ok_or_else(not_found_category)?;
    let mut ids = vec![cat.id];
    ids.extend(db::child_ids(&state.db, cat.id).await?);
    let totals = db::category_currency_totals(&state.db, &cat.kind, &ids, &month).await?;
    let total: f64 = totals.values().sum();
    // The chips offer the subcategories that exist in that month; the list shows every
    // operation the total counts, also those of subcategories deleted since, so they agree.
    let subcategories = db::visible_subcategories(&state.db, cat.id, &month.0).await?;
    let listed = ids;
    Ok(Json(json!({
        "id": cat.id, "name": cat.name, "emoji": cat.emoji, "color": cat.color, "type": cat.kind,
        "total": total, "totals_by_currency": totals, "target_amount": cat.target_amount,
        "subcategories": subcategories,
        "transactions": db::category_transactions(&state.db, &cat.kind, &listed, &month).await?,
    })))
}

#[derive(Deserialize)]
struct CategoryCreate {
    name: String,
    emoji: String,
    color: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    target_amount: Option<f64>,
}

async fn create_category(State(state): AppRef, headers: HeaderMap, Json(c): Json<CategoryCreate>) -> ApiResult<Json<db::CategoryOut>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    let kind = c.kind.as_deref().unwrap_or("expense");
    if !matches!(kind, "expense" | "income" | "savings") {
        return Err(ApiError::status(400, "Неизвестный тип категории"));
    }
    if c.name.trim().is_empty() {
        return Err(ApiError::status(400, "Название категории не может быть пустым"));
    }
    // A budget has one income and one savings category; extra ones would be merged away
    // together with their operations, so they are refused here.
    if kind != "expense" {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE budget_id = $1 AND parent_id IS NULL AND type = $2)")
            .bind(budget).bind(kind)
            .fetch_one(&state.db)
            .await?;
        if exists {
            return Err(ApiError::status(400, "Такая категория уже есть"));
        }
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO categories (name, emoji, color, type, target_amount, budget_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(c.name.trim()).bind(&c.emoji).bind(&c.color).bind(kind).bind(c.target_amount).bind(budget)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(db::category_out_all_time(&state.db, budget, id).await?.ok_or(ApiError::Db(sqlx::Error::RowNotFound))?))
}

#[derive(Deserialize)]
struct CategoryUpdate {
    name: Option<String>,
    emoji: Option<String>,
    color: Option<String>,
    target_amount: Option<f64>,
}

async fn update_category(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>, Json(c): Json<CategoryUpdate>) -> ApiResult<Json<db::CategoryOut>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    if db::root_category_row(&state.db, budget, id).await?.is_none() {
        return Err(not_found_category());
    }
    sqlx::query("UPDATE categories SET name = COALESCE($1, name), emoji = COALESCE($2, emoji), color = COALESCE($3, color), target_amount = $4 WHERE id = $5")
        .bind(&c.name).bind(&c.emoji).bind(&c.color).bind(c.target_amount).bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(db::category_out_all_time(&state.db, budget, id).await?.ok_or(ApiError::Db(sqlx::Error::RowNotFound))?))
}

async fn delete_category(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let cat = db::root_category_row(&state.db, me.budget_owner_id, id).await?.ok_or_else(not_found_category)?;
    if cat.kind == "income" || cat.kind == "savings" {
        return Err(ApiError::internal("Системную категорию удалять нельзя"));
    }
    let mut tx = state.db.begin().await?;
    let children: Vec<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE parent_id = $1").bind(id).fetch_all(&mut *tx).await?;
    sqlx::query("DELETE FROM transactions WHERE category_id = ANY($1) OR category_id = $2").bind(&children).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM categories WHERE parent_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM categories WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct SubcategoryIn {
    name: String,
    emoji: String,
    color: String,
    /// The month on screen when it is added: a subcategory added while looking at a past
    /// month shows from that month. Never later than the current month.
    #[serde(default)]
    month: Option<NaiveDate>,
}

async fn create_subcategory(State(state): AppRef, headers: HeaderMap, Path(parent): Path<i64>, Json(s): Json<SubcategoryIn>) -> ApiResult<Json<db::SubcategoryOut>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    let parent_row = db::category_row(&state.db, budget, parent).await?.ok_or_else(not_found_category)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO categories (name, emoji, color, type, parent_id, visible_from, budget_id) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&s.name).bind(&s.emoji).bind(&s.color).bind(parent_row.kind).bind(parent)
    .bind(s.month.map(|m| db::month_bounds(m).0).filter(|m| *m < db::current_month_start()).unwrap_or_else(db::current_month_start))
    .bind(budget)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(db::SubcategoryOut { id, name: s.name, emoji: s.emoji, color: s.color }))
}

async fn update_subcategory(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>, Json(s): Json<SubcategoryIn>) -> ApiResult<Json<db::SubcategoryOut>> {
    let me = user(&state, &headers).await?;
    if db::subcategory(&state.db, me.budget_owner_id, id).await?.is_none() {
        return Err(ApiError::status(404, "Подкатегория не найдена"));
    }
    sqlx::query("UPDATE categories SET name = $1, emoji = $2, color = $3 WHERE id = $4")
        .bind(&s.name).bind(&s.emoji).bind(&s.color).bind(id)
        .execute(&state.db)
        .await?;
    Ok(Json(db::SubcategoryOut { id, name: s.name, emoji: s.emoji, color: s.color }))
}

/// Subcategories are archived from the current month on, so past months keep showing them.
async fn delete_subcategory(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    if db::subcategory(&state.db, me.budget_owner_id, id).await?.is_none() {
        return Err(ApiError::status(404, "Подкатегория не найдена"));
    }
    sqlx::query("UPDATE categories SET archived_from = $1 WHERE id = $2")
        .bind(db::current_month_start()).bind(id)
        .execute(&state.db)
        .await?;
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Savings accounts with interest (src/savings.rs)
// ---------------------------------------------------------------------------
fn account_not_found() -> ApiError {
    ApiError::status(404, "Счёт не найден")
}

/// A savings subcategory of the user's budget that is still in use.
async fn savings_sub(state: &AppState, me: &CurrentUser, id: i64) -> ApiResult<()> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM categories WHERE id = $1 AND budget_id = $2 AND parent_id IS NOT NULL
           AND type = 'savings' AND archived_from IS NULL)",
    )
    .bind(id).bind(me.budget_owner_id)
    .fetch_one(&state.db)
    .await?;
    if found { Ok(()) } else { Err(account_not_found()) }
}

fn check_account(a: &savings::AccountIn) -> ApiResult<()> {
    a.check().map_err(|m| ApiError::status(400, m))?;
    check_amount(1.0, &a.currency)
}

async fn list_accounts(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<savings::AccountOut>>> {
    let me = user(&state, &headers).await?;
    Ok(Json(savings::list(&state.db, me.budget_owner_id).await?))
}

/// A new account: a savings subcategory with interest settings.
async fn create_account(State(state): AppRef, headers: HeaderMap, Json(a): Json<savings::AccountIn>) -> ApiResult<Json<savings::AccountOut>> {
    let me = user(&state, &headers).await?;
    check_account(&a)?;
    let name = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::status(400, "Название категории не может быть пустым"))?;
    let budget = me.budget_owner_id;
    let root = db::ensure_system_root(&state.db, budget, "savings").await?;
    // Visible from its start month, so the months it earned in show it.
    let visible_from = db::month_bounds(a.start).0;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO categories (name, emoji, color, type, parent_id, visible_from, budget_id) VALUES ($1, '•', '#545c7e', 'savings', $2, $3, $4) RETURNING id",
    )
    .bind(name).bind(root).bind(visible_from).bind(budget)
    .fetch_one(&state.db)
    .await?;
    savings::configure(&state.db, id, budget, &a).await?;
    Ok(Json(savings::one(&state.db, id, budget).await?.ok_or_else(account_not_found)?))
}

/// New settings for a savings subcategory: it becomes an account, or an account changes.
async fn update_account(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>, Json(a): Json<savings::AccountIn>) -> ApiResult<Json<savings::AccountOut>> {
    let me = user(&state, &headers).await?;
    savings_sub(&state, &me, id).await?;
    check_account(&a)?;
    if let Some(name) = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        sqlx::query("UPDATE categories SET name = $1 WHERE id = $2").bind(name).bind(id).execute(&state.db).await?;
    }
    savings::configure(&state.db, id, me.budget_owner_id, &a).await?;
    Ok(Json(savings::one(&state.db, id, me.budget_owner_id).await?.ok_or_else(account_not_found)?))
}

/// Money taken out of savings (the category or one of its accounts) back into the month.
/// It can't be more than is there in that currency on that day.
async fn withdraw(State(state): AppRef, headers: HeaderMap, Query(q): Query<CategoryIdQuery>, Json(t): Json<IncomeCreate>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    check_amount(t.amount, &t.currency)?;
    let root = db::ensure_system_root(&state.db, budget, "savings").await?;
    let target = q.category_id.unwrap_or(root);
    let fits: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM categories WHERE id = $1 AND budget_id = $2 AND type = 'savings')")
        .bind(target).bind(budget)
        .fetch_one(&state.db)
        .await?;
    if !fits {
        return Err(not_found_category());
    }
    let mut ids = vec![target];
    if target == root {
        ids.extend(db::child_ids(&state.db, root).await?);
    }
    let day = t.transaction_date.unwrap_or_else(|| Utc::now().date_naive()).to_string();
    let currency = t.currency.trim().to_string();
    let available: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN source = 'withdrawal' THEN -amount ELSE amount END), 0.0) FROM transactions
         WHERE type = 'savings' AND category_id = ANY($1) AND currency = $2 AND transaction_date <= $3",
    )
    .bind(&ids).bind(&currency).bind(&day)
    .fetch_one(&state.db)
    .await?;
    if t.amount > available + 0.005 {
        return Err(ApiError::status(400, "На счёте нет такой суммы"));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, transaction_date, source)
         VALUES ($1, $2, 'savings', $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(me.id).bind(target).bind(t.amount).bind(&currency).bind(t.note.as_deref().unwrap_or("")).bind(&day).bind(db::WITHDRAWAL)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct TransferIn {
    from_id: i64,
    to_id: i64,
    amount: f64,
    currency: String,
    note: Option<String>,
    transaction_date: Option<NaiveDate>,
}

/// Moves money between two savings accounts of the budget (or the savings without an
/// account): a withdrawal from one and a deposit into the other, linked to each other.
/// Savings in total and the month's balance stay the same.
async fn transfer(State(state): AppRef, headers: HeaderMap, Json(t): Json<TransferIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    check_amount(t.amount, &t.currency)?;
    if t.from_id == t.to_id {
        return Err(ApiError::status(400, "Выберите два разных счёта"));
    }
    let found: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE id = ANY($1) AND budget_id = $2 AND type = 'savings' AND archived_from IS NULL")
        .bind([t.from_id, t.to_id]).bind(budget)
        .fetch_one(&state.db)
        .await?;
    if found != 2 {
        return Err(account_not_found());
    }
    let day = t.transaction_date.unwrap_or_else(|| Utc::now().date_naive()).to_string();
    let currency = t.currency.trim().to_string();
    let note = t.note.as_deref().map(str::trim).unwrap_or("");
    let mut tx = state.db.begin().await?;
    // The source account is locked, so two transfers at once can't both spend the same money.
    sqlx::query("SELECT id FROM categories WHERE id = $1 FOR UPDATE").bind(t.from_id).execute(&mut *tx).await?;
    let available: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN source = 'withdrawal' THEN -amount ELSE amount END), 0.0) FROM transactions
         WHERE type = 'savings' AND category_id = $1 AND currency = $2 AND transaction_date <= $3",
    )
    .bind(t.from_id).bind(&currency).bind(&day)
    .fetch_one(&mut *tx)
    .await?;
    if t.amount > available + 0.005 {
        return Err(ApiError::status(400, "На счёте нет такой суммы"));
    }
    let insert = "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, transaction_date, source)
                  VALUES ($1, $2, 'savings', $3, $4, $5, $6, $7) RETURNING id";
    let out: i64 = sqlx::query_scalar(insert)
        .bind(me.id).bind(t.from_id).bind(t.amount).bind(&currency).bind(note).bind(&day).bind(db::WITHDRAWAL)
        .fetch_one(&mut *tx)
        .await?;
    let into: i64 = sqlx::query_scalar(insert)
        .bind(me.id).bind(t.to_id).bind(t.amount).bind(&currency).bind(note).bind(&day).bind(None::<String>)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE transactions SET pair_id = CASE id WHEN $1 THEN $2 ELSE $1 END WHERE id IN ($1, $2)")
        .bind(out).bind(into)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "from": out, "to": into })))
}

#[derive(Deserialize)]
struct RatesQuery {
    source: Option<String>,
}

/// Today's exchange rates from the chosen source (the Turkish central bank by default).
async fn exchange_rates(State(state): AppRef, headers: HeaderMap, Query(q): Query<RatesQuery>) -> ApiResult<Json<rates::Rates>> {
    user(&state, &headers).await?;
    let source = rates::source_name(q.source.as_deref().unwrap_or("tcmb")).ok_or_else(|| ApiError::status(400, "Неизвестный источник курсов"))?;
    state.rates.get(source).await.map(Json).map_err(|_| ApiError::status(502, "Курсы сейчас недоступны"))
}

async fn get_settings(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<rates::Settings>> {
    let me = user(&state, &headers).await?;
    Ok(Json(rates::settings(&state.db, me.budget_owner_id).await?))
}

/// The main currency and where rates come from; for admins. Rates are looked up again.
async fn put_settings(State(state): AppRef, headers: HeaderMap, Json(s): Json<rates::Settings>) -> ApiResult<Json<rates::Settings>> {
    let me = admin(&state, &headers).await?;
    let main = rates::symbol(&s.main_currency).to_string();
    if !["₺", "$", "€", "₽"].contains(&main.as_str()) {
        return Err(ApiError::status(400, "Неизвестная валюта"));
    }
    if s.rate_source != "off" && rates::source_name(&s.rate_source).is_none() {
        return Err(ApiError::status(400, "Неизвестный источник курсов"));
    }
    let s = rates::Settings { main_currency: main, rate_source: s.rate_source };
    rates::save_settings(&state.db, me.budget_owner_id, &s).await?;
    lookup_rates(&state.db);
    Ok(Json(s))
}

/// The subcategory stays with its operations; it just stops earning.
async fn stop_account(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    savings_sub(&state, &me, id).await?;
    savings::stop(&state.db, id, me.budget_owner_id).await?;
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct TransactionCreate {
    amount: f64,
    currency: String,
    note: Option<String>,
    is_debt: Option<bool>,
    subcategory_id: Option<i64>,
    transaction_date: Option<NaiveDate>,
}

#[derive(Deserialize)]
struct IncomeCreate {
    amount: f64,
    currency: String,
    note: Option<String>,
    transaction_date: Option<NaiveDate>,
}

#[derive(Deserialize)]
struct CategoryIdQuery {
    category_id: Option<i64>,
}

/// A sum of money must be positive, and a currency is a short symbol or code like ₺ or USD.
fn check_amount(amount: f64, currency: &str) -> ApiResult<()> {
    if !amount.is_finite() || amount <= 0.0 || amount > 1e12 {
        return Err(ApiError::status(400, "Сумма должна быть больше нуля"));
    }
    let currency = currency.trim();
    if currency.is_empty() || currency.chars().count() > 8 {
        return Err(ApiError::status(400, "Неизвестная валюта"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_transaction(
    db: &PgPool, user_id: i64, category_id: Option<i64>, kind: &str, amount: f64, currency: &str, note: Option<&str>, is_debt: bool,
    date: Option<NaiveDate>,
) -> ApiResult<Json<serde_json::Value>> {
    check_amount(amount, currency)?;
    let date = date.unwrap_or_else(|| Utc::now().date_naive()).to_string();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, is_debt, transaction_date)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(user_id).bind(category_id).bind(kind).bind(amount).bind(currency).bind(note).bind(is_debt as i64).bind(date)
    .fetch_one(db)
    .await?;
    lookup_rates(db);
    Ok(Json(json!({ "id": id, "ok": true })))
}

/// Looks up the rates of new or changed operations in the background; the answer doesn't wait.
fn lookup_rates(db: &PgPool) {
    let db = db.clone();
    tokio::spawn(async move {
        if let Err(e) = rates::fill(&db, None).await {
            tracing::warn!(error = %e, "rates of operations not filled");
        }
    });
}

/// An expense with a subcategory is booked on the subcategory itself.
async fn create_expense(State(state): AppRef, headers: HeaderMap, Path(category): Path<i64>, Json(t): Json<TransactionCreate>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    if db::category_row(&state.db, budget, category).await?.is_none() {
        return Err(not_found_category());
    }
    if let Some(sub) = t.subcategory_id {
        if db::subcategory(&state.db, budget, sub).await?.is_none() {
            return Err(ApiError::status(404, "Подкатегория не найдена"));
        }
    }
    let target = t.subcategory_id.unwrap_or(category);
    insert_transaction(&state.db, me.id, Some(target), "expense", t.amount, &t.currency, t.note.as_deref(), t.is_debt.unwrap_or(false), t.transaction_date).await
}

async fn create_income(State(state): AppRef, headers: HeaderMap, Query(q): Query<CategoryIdQuery>, Json(t): Json<IncomeCreate>) -> ApiResult<Json<serde_json::Value>> {
    create_typed(state, headers, q, t, "income").await
}

async fn create_savings(State(state): AppRef, headers: HeaderMap, Query(q): Query<CategoryIdQuery>, Json(t): Json<IncomeCreate>) -> ApiResult<Json<serde_json::Value>> {
    create_typed(state, headers, q, t, "savings").await
}

async fn create_typed(state: Arc<AppState>, headers: HeaderMap, q: CategoryIdQuery, t: IncomeCreate, kind: &str) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    let category = match q.category_id {
        Some(id) if db::category_row(&state.db, budget, id).await?.is_some() => id,
        Some(_) => return Err(not_found_category()),
        None => db::ensure_system_root(&state.db, budget, kind).await?,
    };
    insert_transaction(&state.db, me.id, Some(category), kind, t.amount, &t.currency, t.note.as_deref(), false, t.transaction_date).await
}

#[derive(Deserialize)]
struct TransactionUpdate {
    amount: Option<f64>,
    currency: Option<String>,
    note: Option<String>,
    transaction_date: Option<NaiveDate>,
    /// The operation's category or one of its subcategories (moving between them).
    category_id: Option<i64>,
}

/// Changes an operation of the budget. It can move between a category and its own
/// subcategories, not to another category, so its type always fits where it is.
async fn update_transaction(
    State(state): AppRef,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(u): Json<TransactionUpdate>,
) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let row: Option<(f64, String, i64)> = sqlx::query_as(
        "SELECT t.amount, t.currency, COALESCE(c.parent_id, c.id)
         FROM transactions t JOIN categories c ON c.id = t.category_id
         WHERE t.id = $1 AND c.budget_id = $2",
    )
    .bind(id).bind(me.budget_owner_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((amount, currency, root)) = row else { return Err(ApiError::status(404, "Операция не найдена")) };
    check_amount(u.amount.unwrap_or(amount), u.currency.as_deref().unwrap_or(&currency))?;
    let changes_rate = u.currency.is_some() || u.transaction_date.is_some();
    if let Some(target) = u.category_id {
        let fits: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE id = $1 AND budget_id = $2 AND (id = $3 OR parent_id = $3))")
            .bind(target).bind(me.budget_owner_id).bind(root)
            .fetch_one(&state.db)
            .await?;
        if !fits {
            return Err(ApiError::status(404, "Подкатегория не найдена"));
        }
    }
    sqlx::query(
        "UPDATE transactions SET
            amount = COALESCE($2, amount),
            currency = COALESCE($3, currency),
            note = COALESCE($4, note),
            transaction_date = COALESCE($5, transaction_date),
            category_id = COALESCE($6, category_id),
            -- Interest the user corrects to match the bank is never recalculated again.
            source = CASE WHEN source = 'interest' THEN 'interest_fixed' ELSE source END
         WHERE id = $1",
    )
    .bind(id).bind(u.amount).bind(u.currency.map(|c| c.trim().to_string())).bind(u.note)
    .bind(u.transaction_date.map(|d| d.to_string())).bind(u.category_id)
    .execute(&state.db)
    .await?;
    // Both halves of a transfer keep the same amount, currency, day and description.
    sqlx::query(
        "UPDATE transactions p SET amount = t.amount, currency = t.currency, transaction_date = t.transaction_date, note = t.note
         FROM transactions t WHERE t.id = $1 AND p.id = t.pair_id",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    if changes_rate {
        rates::forget(&state.db, id).await?;
        lookup_rates(&state.db);
    }
    Ok(ok())
}

/// Members of a budget can delete each other's operations, but not other budgets'.
async fn delete_transaction(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    // The other half of a transfer goes with it.
    let done = sqlx::query(
        "DELETE FROM transactions t USING categories c
         WHERE c.id = t.category_id AND c.budget_id = $2
           AND (t.id = $1 OR t.id = (SELECT pair_id FROM transactions WHERE id = $1))",
    )
    .bind(id).bind(me.budget_owner_id)
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::status(404, "Операция не найдена"));
    }
    Ok(ok())
}

// ---------------------------------------------------------------------------
// People of a budget. Whoever opens the bot gets a budget of their own and manages it;
// adding a person brings them into your budget (their own records stay in theirs).
// ---------------------------------------------------------------------------
async fn list_users(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<db::UserOut>>> {
    let me = admin(&state, &headers).await?;
    Ok(Json(db::list_budget_users(&state.db, me.budget_owner_id).await?))
}

#[derive(Deserialize)]
struct UserCreateIn {
    query: Option<String>,
    telegram_id: Option<String>,
    username: Option<String>,
    first_name: Option<String>,
    role: Option<String>,
}

/// "123456", "@name", "https://t.me/name" -> numeric Telegram id, or "pending:name" for a
/// person known only by username until they open the bot.
/// Telegram usernames: Latin letters, digits and underscores, 4 to 32 characters,
/// starting with a letter. Anything else could never be matched at login.
fn is_telegram_username(q: &str) -> bool {
    (4..=32).contains(&q.len())
        && q.starts_with(|c: char| c.is_ascii_alphabetic())
        && q.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_user_query(raw: &str) -> Option<String> {
    let mut q = raw.trim();
    for prefix in ["https://", "http://"] {
        q = q.strip_prefix(prefix).unwrap_or(q);
    }
    for prefix in ["t.me/", "telegram.me/", "www.t.me/"] {
        q = q.strip_prefix(prefix).unwrap_or(q);
    }
    let q = q.trim_start_matches('@').trim_end_matches('/').trim();
    if q.is_empty() {
        None
    } else if q.chars().all(|c| c.is_ascii_digit()) {
        Some(q.to_string())
    } else if is_telegram_username(q) {
        Some(format!("pending:{}", q.to_lowercase()))
    } else {
        None
    }
}

async fn create_user(State(state): AppRef, headers: HeaderMap, Json(u): Json<UserCreateIn>) -> ApiResult<Json<db::UserOut>> {
    let me = admin(&state, &headers).await?;
    let raw = u.query.or(u.telegram_id).or(u.username).unwrap_or_default();
    let telegram_id = parse_user_query(&raw)
        .ok_or_else(|| ApiError::status(400, "Введите @username (латиница, цифры и _, от 4 до 32 символов), Telegram ID или ссылку t.me"))?;
    let role = u.role.unwrap_or_else(|| "user".into());
    if !matches!(role.as_str(), "user" | "admin" | "owner") {
        return Err(ApiError::status(400, "Неизвестная роль"));
    }
    if role == "owner" && db::owner_exists(&state.db).await? {
        return Err(ApiError::internal("Owner уже существует"));
    }
    let id = match db::user_by_telegram_id(&state.db, &telegram_id).await? {
        Some(existing) => {
            let existing_budget: Option<i64> = sqlx::query_scalar("SELECT budget_owner_id FROM users WHERE id = $1")
                .bind(existing.id)
                .fetch_one(&state.db)
                .await?;
            if existing.role == "owner" && existing_budget.unwrap_or(existing.id) != me.budget_owner_id {
                return Err(ApiError::internal("Нельзя перенести owner в другой бюджет"));
            }
            // Their records stay with the categories of their own budget.
            sqlx::query(
                "UPDATE users SET first_name = COALESCE(NULLIF($1, ''), first_name), role = $2, budget_owner_id = $3 WHERE id = $4",
            )
            .bind(&u.first_name).bind(&role).bind(me.budget_owner_id).bind(existing.id)
            .execute(&state.db)
            .await?;
            existing.id
        }
        None => {
            sqlx::query_scalar("INSERT INTO users (telegram_id, username, first_name, role, budget_owner_id) VALUES ($1, $2, $3, $4, $5) RETURNING id")
                .bind(&telegram_id).bind(None::<String>).bind(&u.first_name).bind(&role).bind(me.budget_owner_id)
                .fetch_one(&state.db)
                .await?
        }
    };
    Ok(Json(db::user_by_id(&state.db, id).await?.ok_or(ApiError::Db(sqlx::Error::RowNotFound))?))
}

/// A person of the caller's budget, or 404.
async fn budget_member(state: &AppState, me: &CurrentUser, id: i64) -> ApiResult<db::UserOut> {
    let budget: Option<i64> = sqlx::query_scalar("SELECT COALESCE(budget_owner_id, id) FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    if budget != Some(me.budget_owner_id) {
        return Err(ApiError::status(404, "Пользователь не найден"));
    }
    db::user_by_id(&state.db, id).await?.ok_or_else(|| ApiError::status(404, "Пользователь не найден"))
}

#[derive(Deserialize)]
struct UserRoleUpdateIn {
    role: String,
}

async fn update_user_role(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>, Json(r): Json<UserRoleUpdateIn>) -> ApiResult<Json<db::UserOut>> {
    let me = admin(&state, &headers).await?;
    let target = budget_member(&state, &me, id).await?;
    if r.role == "owner" && db::owner_exists(&state.db).await? {
        return Err(ApiError::internal("Owner уже существует"));
    }
    if !matches!(r.role.as_str(), "user" | "admin" | "owner") {
        return Err(ApiError::status(400, "Неизвестная роль"));
    }
    // The person whose budget it is always keeps the right to manage it.
    if target.id == me.budget_owner_id && r.role == "user" {
        return Err(ApiError::internal("Нельзя снять права с хозяина бюджета"));
    }
    sqlx::query("UPDATE users SET role = $1 WHERE id = $2").bind(&r.role).bind(id).execute(&state.db).await?;
    Ok(Json(db::user_by_id(&state.db, id).await?.ok_or(ApiError::Db(sqlx::Error::RowNotFound))?))
}

/// Removes a person from the budget. Their records in this budget stay here; they go back to
/// their own budget (an invitation by username that was never used is simply deleted).
async fn delete_user(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = admin(&state, &headers).await?;
    if id == me.id {
        return Err(ApiError::internal("Нельзя удалить самого себя"));
    }
    let target = budget_member(&state, &me, id).await?;
    if target.role == "owner" {
        return Err(ApiError::internal("Нельзя удалить owner"));
    }
    if target.id == me.budget_owner_id {
        return Err(ApiError::internal("Нельзя удалить хозяина бюджета"));
    }
    if target.telegram_id.starts_with("pending:") {
        sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(&state.db).await?;
    } else {
        sqlx::query("UPDATE users SET budget_owner_id = id, role = 'admin' WHERE id = $1").bind(id).execute(&state.db).await?;
        db::seed_budget(&state.db, id).await?;
    }
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Frontend
// ---------------------------------------------------------------------------
/// The interface, built from source in `app/static/v2`.
/// Its page refers to files as `/v2/a/__V__/…`; `__V__` becomes the files' fingerprint,
/// `__SOURCE__` the address of the source code and `__DONATE__` the donation ways.
async fn index(State(state): AppRef) -> Response {
    let mut res = page(&format!("{}/v2/index.html", state.cfg.static_dir)).await;
    if res.status().is_success() {
        let body = axum::body::to_bytes(std::mem::take(res.body_mut()), 1 << 20).await.unwrap_or_default();
        let html = String::from_utf8_lossy(&body)
            .replace("__V__", &state.v2_version)
            .replace("__SOURCE__", &html_attr(&state.cfg.source_url))
            .replace("__DONATE__", &html_attr(&state.cfg.donate.to_string()));
        *res.body_mut() = axum::body::Body::from(html);
        res.headers_mut().remove(header::CONTENT_LENGTH);
    }
    res
}

fn html_attr(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A file of the new interface under its fingerprinted address; it never changes there,
/// so browsers and Cloudflare may keep it for a year.
async fn v2_asset(State(state): AppRef, Path((_version, path)): Path<(String, String)>) -> Response {
    if path.split('/').any(|part| part.is_empty() || part.starts_with('.')) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let kind = match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "text/plain; charset=utf-8",
    };
    match tokio::fs::read(format!("{}/v2/{path}", state.cfg.static_dir)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(kind)),
                (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable")),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Short hash of every file's path and contents under `dir`.
fn v2_fingerprint(dir: &str) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out) } else { out.push(p) }
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(dir), &mut files);
    files.sort();
    let mut hash = Sha256::new();
    for f in files {
        hash.update(f.to_string_lossy().as_bytes());
        hash.update(std::fs::read(&f).unwrap_or_default());
    }
    hex::encode(hash.finalize())[..10].to_string()
}

async fn page(path: &str) -> Response {
    match tokio::fs::read(path).await {
        Ok(html) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache, must-revalidate")),
            ],
            html,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Background jobs: expired sessions are removed once a minute
// ---------------------------------------------------------------------------
async fn worker(db: PgPool, every: Duration) {
    let mut tick = tokio::time::interval(every);
    let mut last_interest = None;
    loop {
        tick.tick().await;
        let now = Utc::now().timestamp();
        let res = async {
            sqlx::query(
                "INSERT INTO jobs (kind, payload, status, run_at)
                 SELECT 'cleanup_expired_sessions', '{}', 'pending', $1
                 WHERE NOT EXISTS (SELECT 1 FROM jobs WHERE kind = 'cleanup_expired_sessions' AND status IN ('pending','running'))",
            )
            .bind(now)
            .execute(&db)
            .await?;
            let job: Option<(i64, String)> = sqlx::query_as(
                "UPDATE jobs
                 SET status = 'running', attempts = attempts + 1, locked_at = $1, updated_at = CURRENT_TIMESTAMP::text
                 WHERE id = (SELECT id FROM jobs WHERE status = 'pending' AND run_at <= $1 ORDER BY run_at ASC, id ASC LIMIT 1 FOR UPDATE SKIP LOCKED)
                 RETURNING id, kind",
            )
            .bind(now)
            .fetch_optional(&db)
            .await?;
            if let Some((id, kind)) = job {
                if kind == "cleanup_expired_sessions" {
                    let deleted = sqlx::query("DELETE FROM sessions WHERE expires_at <= $1").bind(now).execute(&db).await?.rows_affected();
                    tracing::debug!(deleted, "expired sessions removed");
                }
                sqlx::query("UPDATE jobs SET status = 'done', last_error = NULL, updated_at = CURRENT_TIMESTAMP::text WHERE id = $1")
                    .bind(id)
                    .execute(&db)
                    .await?;
                // Finished jobs are only history; keep a day of it.
                sqlx::query("DELETE FROM jobs WHERE status = 'done' AND run_at < $1").bind(now - 86_400).execute(&db).await?;
            }
            Ok::<_, sqlx::Error>(())
        }
        .await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "job worker tick failed");
        }
        // Interest is paid on time even when nobody opens the app; once an hour is plenty.
        if last_interest.is_none_or(|t: std::time::Instant| t.elapsed() >= Duration::from_secs(3600)) {
            last_interest = Some(std::time::Instant::now());
            if let Err(e) = recurring::sync_all(&db).await {
                tracing::warn!(error = %e, "recurring operations failed");
            }
            if let Err(e) = savings::sync_all(&db).await {
                tracing::warn!(error = %e, "interest accrual failed");
            }
            // Operations whose rate source didn't answer before, and those added by
            // recurring payments, interest or an import.
            if let Err(e) = rates::fill(&db, None).await {
                tracing::warn!(error = %e, "rates of operations not filled");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------
fn app(state: Arc<AppState>) -> Router {
    let leak = |l: Limiter| -> &'static Limiter { Box::leak(Box::new(l)) };
    let auth_limit = leak(Limiter::new(3, Duration::from_secs(1)));
    let write_limit = leak(Limiter::new(5, Duration::from_secs(2)));
    // Reads change nothing and need a session; the old 1 per 10 s ran out during normal use.
    let read_limit = leak(Limiter::new(60, Duration::from_millis(500)));
    let reads = from_fn_with_state(read_limit, limit::limit);
    // Reports are flipped through period by period, so they get their own, faster bucket.
    let report_limit = leak(Limiter::new(30, Duration::from_millis(500)));

    let writes = Router::new()
        .route("/api/auth/revoke-others", post(revoke_others))
        .route("/api/users/{user_id}/role", patch(update_user_role))
        .route("/api/users/{user_id}", delete(delete_user))
        .route("/api/categories/{category_id}", patch(update_category).delete(delete_category))
        .route("/api/subcategories/{subcategory_id}", patch(update_subcategory).delete(delete_subcategory))
        .route("/api/categories/{category_id}/subcategories", post(create_subcategory))
        .route("/api/categories/{category_id}/expenses", post(create_expense))
        .route("/api/income", post(create_income))
        .route("/api/savings", post(create_savings))
        .route("/api/transactions/{transaction_id}", delete(delete_transaction).patch(update_transaction))
        .route("/api/savings/accounts/{account_id}", patch(update_account).delete(stop_account))
        .route("/api/savings/withdraw", post(withdraw))
        .route("/api/savings/transfer", post(transfer))
        .route("/api/recurring", post(recurring::create))
        .route("/api/settings", axum::routing::put(put_settings))
        .route("/api/recurring/{recurring_id}", patch(recurring::update).delete(recurring::stop))
        .route_layer(from_fn_with_state(write_limit, limit::limit));

    let login = Router::new()
        .route("/api/auth/telegram", post(auth_telegram))
        .route_layer(from_fn_with_state(auth_limit, limit::limit));
    // Passwords are guessed slowly: five tries per address, then one every two seconds.
    let password_limit = leak(Limiter::new(5, Duration::from_secs(2)));
    let passwords = Router::new()
        .route("/api/setup", post(local::setup))
        .route("/api/auth/login", post(local::login))
        .route("/api/auth/register", post(local::register))
        .route("/api/auth/password", post(local::change_password))
        .route("/api/invite/{token}", post(local::accept_invite))
        .route("/api/2fa/setup", post(totp::setup))
        .route("/api/2fa/enable", post(totp::enable))
        .route("/api/2fa/disable", post(totp::disable))
        .route("/api/2fa/recovery-codes", post(totp::recovery_codes))
        .route_layer(from_fn_with_state(password_limit, limit::limit));
    // Asked on every start before signing in; an invitation link can't be guessed (192 bits).
    let public = Router::new()
        .route("/api/setup", get(local::setup_state))
        .route("/api/invite/{token}", get(local::check_invite))
        .route_layer(from_fn_with_state(auth_limit, limit::limit));

    Router::new()
        .merge(writes)
        .merge(login)
        .merge(passwords)
        .merge(public)
        .route("/api/invites", get(local::list_invites).post(local::create_invite))
        .route("/api/invites/{invite_id}", delete(local::delete_invite))
        .route("/api/auth/logout", post(logout))
        .route("/api/me", get(me).route_layer(reads.clone()))
        .route("/api/users", get(list_users).route_layer(reads.clone()).post(create_user))
        .route("/api/categories", get(list_categories).route_layer(reads.clone()).post(create_category))
        .route("/api/home", get(home).route_layer(reads.clone()))
        .route("/api/categories/{category_id}/details", get(category_details).route_layer(reads.clone()))
        .route("/api/stats/expenses-by-category", get(stats).route_layer(reads.clone()))
        .route("/api/month-summary", get(month_summary).route_layer(reads.clone()))
        .route("/api/savings-summary", get(savings_summary).route_layer(reads.clone()))
        .route("/api/savings/accounts", get(list_accounts).route_layer(reads.clone()).post(create_account))
        .route("/api/rates", get(exchange_rates).route_layer(reads.clone()))
        .route("/api/settings", get(get_settings).route_layer(reads.clone()))
        .route("/api/recurring", get(recurring::list).route_layer(reads.clone()))
        .route("/api/export.csv", get(import::export).route_layer(reads))
        // A whole history is a few megabytes of JSON; one import at a time is plenty.
        .route(
            "/api/import",
            post(import::import)
                .layer(axum::extract::DefaultBodyLimit::max(32 << 20))
                .route_layer(from_fn_with_state(leak(Limiter::new(3, Duration::from_secs(3))), limit::limit)),
        )
        .route("/api/report", get(report).route_layer(from_fn_with_state(report_limit, limit::limit)))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{suffix}", delete(delete_session))
        .route("/api/perf/startup", post(perf_startup))
        .route("/api/config", get(config))
        .route("/health", get(health))
        .route("/", get(index))
        .route("/app/", get(index))
        .route("/app/{*rest}", get(index))
        // Addresses of the beta, kept for links people saved.
        .route("/v2", get(index))
        .route("/v2/", get(index))
        .route("/v2/a/{version}/{*path}", get(v2_asset))
        .route("/v2/{*rest}", get(index))
        // Files keep their names between releases, so browsers must check for a newer copy.
        .nest_service(
            "/static",
            ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-cache")))
                .service(ServeDir::new(&state.cfg.static_dir)),
        )
        .fallback(get(index))
        .with_state(state)
}

/// Asks the local server for `/health`; exits with an error unless it answers 200.
async fn health_check() -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port: u16 = env_or("PORT", 8009);
    let mut stream = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(("127.0.0.1", port))).await??;
    stream.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n").await?;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut reply)).await??;
    anyhow::ensure!(reply.starts_with(b"HTTP/1.0 200") || reply.starts_with(b"HTTP/1.1 200"), "server is not healthy");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["health"] => return health_check().await,
        [cmd @ ("reset-password" | "reset-2fa"), login] => {
            let url = env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
            let db = PgPoolOptions::new().max_connections(1).connect(&url).await?;
            return if cmd == "reset-2fa" { totp::reset(&db, login).await } else { local::reset_password(&db, login).await };
        }
        [] => {}
        _ => anyhow::bail!("usage: budget-server [health | reset-password <login> | reset-2fa <login>]"),
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env();
    if cfg.bot_token.is_empty() {
        tracing::info!("TELEGRAM_BOT_TOKEN is not set: sign-in with a password only");
    }
    let database_url = env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let db = PgPoolOptions::new().max_connections(8).acquire_timeout(Duration::from_secs(10)).connect(&database_url).await?;
    migrate::run(&db).await?;
    tokio::spawn(worker(db.clone(), Duration::from_secs(env_or("BUDGET_WORKER_INTERVAL_SECONDS", 60))));

    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = env_or("PORT", 8009);
    let v2_version = v2_fingerprint(&format!("{}/v2", cfg.static_dir));
    let state = Arc::new(AppState { db, cfg, v2_version, rates: rates::Cache::default() });
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    tracing::info!("budget-server {} on http://{host}:{port}", env!("CARGO_PKG_VERSION"));
    axum::serve(listener, app(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
