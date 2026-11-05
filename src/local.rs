//! Sign-in with a login and password, the first-run setup and invitation links, so the app
//! works in any browser without Telegram.
//!
//! - On a fresh install the first visitor creates the owner (`/api/setup`).
//! - The owner or an admin makes a one-time link (`/api/invites`); whoever opens it picks a
//!   login and password and joins that budget (`/api/invite/{token}`).
//! - Passwords are stored as Argon2id hashes. People who signed in with a password have
//!   `telegram_id = 'local:<login>'`, which keeps the column unique and the rest of the app
//!   unchanged.

use std::{net::SocketAddr, sync::OnceLock};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use rand::RngExt;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{ApiError, ApiResult, AppRef, AppState, admin, auth, db, limit, totp, user};

const INVITE_DAYS: i64 = 7;
const LOCAL_PREFIX: &str = "local:";

#[derive(Deserialize)]
pub struct AccountIn {
    login: String,
    password: String,
    #[serde(default)]
    name: String,
    /// The interface language, for the names in a new budget.
    #[serde(default)]
    lang: Option<String>,
}

/// 3–32 lowercase Latin letters, digits, dots, dashes or underscores.
fn clean_login(raw: &str) -> ApiResult<String> {
    let login = raw.trim().to_lowercase();
    let ok = (3..=32).contains(&login.len()) && login.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c));
    if ok { Ok(login) } else { Err(ApiError::status(400, "Логин: от 3 до 32 латинских букв, цифр и знаков . _ -")) }
}

fn check_password(p: &str) -> ApiResult<()> {
    if (8..=200).contains(&p.chars().count()) { Ok(()) } else { Err(ApiError::status(400, "Пароль — не короче 8 символов")) }
}

fn clean_name(raw: &str, login: &str) -> String {
    let name: String = raw.trim().chars().take(60).collect();
    if name.is_empty() { login.to_string() } else { name }
}

/// Hashing takes tens of milliseconds on purpose, so it runs off the async threads. Each hash
/// gets its own random salt.
async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default().hash_password(password.as_bytes()).map(|h| h.to_string())
    })
    .await
    .map_err(|_| ApiError::status(500, "hash failed"))?
    .map_err(|_| ApiError::status(500, "hash failed"))
}

async fn verify_password(hash: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
    })
    .await
    .unwrap_or(false)
}

/// A hash to check against when the login doesn't exist, so a wrong login takes as long as
/// a wrong password and doesn't tell which logins are taken.
fn dummy_hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| Argon2::default().hash_password(b"no such user").map(|h| h.to_string()).unwrap_or_default())
        .clone()
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Opens a session and answers with the user and the session cookie.
pub(crate) async fn signed_in(state: &AppState, headers: &HeaderMap, addr: SocketAddr, user_id: i64) -> ApiResult<Response> {
    let ip = limit::client_ip(headers, Some(addr));
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let token = auth::open_session(&state.db, &state.cfg, user_id, &ip, ua).await?;
    let user = db::user_by_id(&state.db, user_id).await?.ok_or_else(|| ApiError::status(404, "Пользователь не найден"))?;
    let cookie = auth::session_cookie(&token, state.cfg.session_ttl, state.cfg.secure_cookie(headers));
    Ok(([(header::SET_COOKIE, cookie)], Json(user)).into_response())
}

/// A person with a password. `budget` is the budget they join, or `None` for one of their own.
async fn create_person(state: &AppState, a: &AccountIn, role: &str, budget: Option<i64>) -> ApiResult<i64> {
    let login = clean_login(&a.login)?;
    check_password(&a.password)?;
    let hash = hash_password(a.password.clone()).await?;
    let res = sqlx::query_scalar(
        "INSERT INTO users (telegram_id, login, password_hash, first_name, role, budget_owner_id)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(format!("{LOCAL_PREFIX}{login}")).bind(&login).bind(&hash).bind(clean_name(&a.name, &login)).bind(role).bind(budget)
    .fetch_one(&state.db)
    .await;
    let id: i64 = match res {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(if e.constraint() == Some("idx_users_single_owner") {
                ApiError::status(409, "Хозяин уже есть")
            } else {
                ApiError::status(409, "Такой логин уже занят")
            });
        }
        Err(e) => return Err(e.into()),
    };
    if budget.is_none() {
        sqlx::query("UPDATE users SET budget_owner_id = id WHERE id = $1").bind(id).execute(&state.db).await?;
        db::seed_budget(&state.db, id, db::budget_lang_of(a.lang.as_deref())).await?;
    }
    Ok(id)
}

/// What the start page needs before anyone has signed in.
pub async fn setup_state(State(state): AppRef) -> ApiResult<Json<serde_json::Value>> {
    let has_owner: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'owner')").fetch_one(&state.db).await?;
    Ok(Json(json!({
        "needs_setup": !has_owner && !state.cfg.demo,
        "telegram": !state.cfg.bot_token.is_empty(),
        "signup_open": state.cfg.open_signup,
        // The public demo: no owner, a «try it» button instead of signing in.
        "demo": state.cfg.demo,
        "source": state.cfg.source_url,
    })))
}

/// The first person on a fresh install becomes the owner.
pub async fn setup(State(state): AppRef, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(a): Json<AccountIn>) -> ApiResult<Response> {
    let has_owner: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'owner')").fetch_one(&state.db).await?;
    if has_owner {
        return Err(ApiError::status(409, "Хозяин уже есть"));
    }
    let id = create_person(&state, &a, "owner", None).await?;
    tracing::info!(user = id, "owner created");
    signed_in(&state, &headers, addr, id).await
}

/// Anyone may make a budget of their own, if the installation allows it (`BUDGET_SIGNUP=open`).
pub async fn register(State(state): AppRef, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(a): Json<AccountIn>) -> ApiResult<Response> {
    if !state.cfg.open_signup {
        return Err(ApiError::status(403, "Вас ещё не пригласили в бюджет"));
    }
    let id = create_person(&state, &a, "admin", None).await?;
    signed_in(&state, &headers, addr, id).await
}

#[derive(Deserialize)]
pub struct LoginIn {
    login: String,
    password: String,
    /// The second step, when two-factor sign-in is on: a code from the app or a recovery code.
    code: Option<String>,
}

pub async fn login(State(state): AppRef, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(l): Json<LoginIn>) -> ApiResult<Response> {
    let found: Option<(i64, Option<String>)> = sqlx::query_as("SELECT id, password_hash FROM users WHERE login = $1")
        .bind(l.login.trim().to_lowercase())
        .fetch_optional(&state.db)
        .await?;
    let (id, hash) = match found {
        Some((id, Some(hash))) => (Some(id), hash),
        _ => (None, dummy_hash()),
    };
    let good = verify_password(hash, l.password).await;
    match id {
        Some(id) if good => {
            totp::check(&state.db, id, l.code.as_deref()).await?;
            signed_in(&state, &headers, addr, id).await
        }
        _ => Err(ApiError::status(401, "Неверный логин или пароль")),
    }
}

#[derive(Deserialize)]
pub struct PasswordIn {
    current: String,
    new: String,
}

/// A new password; other devices are signed out, this one stays.
pub async fn change_password(State(state): AppRef, headers: HeaderMap, Json(p): Json<PasswordIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1").bind(me.id).fetch_one(&state.db).await?;
    let Some(hash) = hash else { return Err(ApiError::status(400, "У вас вход через Telegram, пароля нет")) };
    if !verify_password(hash, p.current).await {
        return Err(ApiError::status(400, "Текущий пароль неверный"));
    }
    check_password(&p.new)?;
    let new_hash = hash_password(p.new).await?;
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2").bind(new_hash).bind(me.id).execute(&state.db).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token != $2").bind(me.id).bind(&me.token).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct InviteIn {
    #[serde(default = "member")]
    role: String,
}

fn member() -> String {
    "user".into()
}

/// A one-time link into the budget, valid for a week. Only its hash is stored, so the link
/// itself is shown once, to the person who made it.
pub async fn create_invite(State(state): AppRef, headers: HeaderMap, Json(i): Json<InviteIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = admin(&state, &headers).await?;
    if !matches!(i.role.as_str(), "user" | "admin") {
        return Err(ApiError::status(400, "Неизвестная роль"));
    }
    let token = hex::encode(rand::rng().random::<[u8; 24]>());
    let expires_at = Utc::now().timestamp() + INVITE_DAYS * 86_400;
    let id: i64 = sqlx::query_scalar("INSERT INTO invites (token_hash, budget_id, role, created_by, expires_at) VALUES ($1, $2, $3, $4, $5) RETURNING id")
        .bind(token_hash(&token)).bind(me.budget_owner_id).bind(&i.role).bind(me.id).bind(expires_at)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": id, "token": token, "role": i.role, "expires_at": expires_at })))
}

/// Links of the budget not used yet.
pub async fn list_invites(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let me = admin(&state, &headers).await?;
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT id, role, expires_at FROM invites WHERE budget_id = $1 AND used_by IS NULL AND expires_at > $2 ORDER BY id DESC",
    )
    .bind(me.budget_owner_id).bind(Utc::now().timestamp())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(|(id, role, expires_at)| json!({ "id": id, "role": role, "expires_at": expires_at })).collect()))
}

pub async fn delete_invite(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = admin(&state, &headers).await?;
    sqlx::query("DELETE FROM invites WHERE id = $1 AND budget_id = $2").bind(id).bind(me.budget_owner_id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The budget an unused, unexpired link leads to: (invite id, budget, role).
async fn open_invite(state: &AppState, token: &str) -> ApiResult<(i64, i64, String)> {
    sqlx::query_as("SELECT id, budget_id, role FROM invites WHERE token_hash = $1 AND used_by IS NULL AND expires_at > $2")
        .bind(token_hash(token)).bind(Utc::now().timestamp())
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::status(404, "Приглашение не найдено или уже использовано"))
}

/// Who invites, for the page that opens from the link.
pub async fn check_invite(State(state): AppRef, Path(token): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let (_, budget, _) = open_invite(&state, &token).await?;
    let owner: Option<String> = sqlx::query_scalar("SELECT COALESCE(first_name, login, username) FROM users WHERE id = $1")
        .bind(budget)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    Ok(Json(json!({ "owner": owner })))
}

/// Joins the budget of the link with a new login and password; the link is used up.
pub async fn accept_invite(
    State(state): AppRef,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(a): Json<AccountIn>,
) -> ApiResult<Response> {
    let (invite, budget, role) = open_invite(&state, &token).await?;
    // Claim the link first, so two people can't both use it.
    let claimed = sqlx::query("UPDATE invites SET used_by = 0 WHERE id = $1 AND used_by IS NULL").bind(invite).execute(&state.db).await?;
    if claimed.rows_affected() == 0 {
        return Err(ApiError::status(404, "Приглашение не найдено или уже использовано"));
    }
    match create_person(&state, &a, &role, Some(budget)).await {
        Ok(id) => {
            sqlx::query("UPDATE invites SET used_by = $1 WHERE id = $2").bind(id).bind(invite).execute(&state.db).await?;
            signed_in(&state, &headers, addr, id).await
        }
        Err(e) => {
            // A taken login or a short password: the link stays usable.
            sqlx::query("UPDATE invites SET used_by = NULL WHERE id = $1").bind(invite).execute(&state.db).await?;
            Err(e)
        }
    }
}

/// `budget-server reset-password <login>`: a new random password, printed once; the person's
/// sessions end.
pub async fn reset_password(db: &sqlx::PgPool, login: &str) -> anyhow::Result<()> {
    let login = login.trim().to_lowercase();
    let password = hex::encode(rand::rng().random::<[u8; 8]>());
    let hash = hash_password(password.clone()).await.map_err(|_| anyhow::anyhow!("hash failed"))?;
    let id: Option<i64> = sqlx::query_scalar("UPDATE users SET password_hash = $2 WHERE login = $1 RETURNING id")
        .bind(&login).bind(hash)
        .fetch_optional(db)
        .await?;
    let id = id.ok_or_else(|| anyhow::anyhow!("no user with the login {login}"))?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(db).await?;
    println!("New password for {login}: {password}\nChange it in Settings after signing in.");
    Ok(())
}

/// Whether this person signs in with a password, for Settings.
pub async fn has_password(state: &AppState, id: i64) -> ApiResult<(Option<String>, bool)> {
    let row: (Option<String>, bool) = sqlx::query_as("SELECT login, password_hash IS NOT NULL FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(row)
}
