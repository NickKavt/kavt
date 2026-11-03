//! Telegram Mini App login (initData check) and cookie sessions. Sign-in with a password is
//! in `local.rs`; both end in `open_session`.

use std::collections::HashMap;

use axum::http::HeaderMap;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use serde::Deserialize;
use sha2::Sha256;
use sqlx::{PgPool, Row};

use crate::{ApiError, Config};

pub const COOKIE: &str = "budget_session";

#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub telegram_id: String,
    pub role: String,
    pub language_code: Option<String>,
    /// Id of the user whose budget this user shares (their own id for a budget owner).
    pub budget_owner_id: i64,
    pub token: String,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin" || self.role == "owner"
    }
}

#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: Option<i64>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language_code: Option<String>,
}

/// Checks Telegram WebApp initData: freshness of auth_date, then the HMAC signature
/// (secret = HMAC_SHA256(key = "WebAppData", msg = bot token)), then the user field.
pub fn validate_init_data(init_data: &str, cfg: &Config) -> Result<TelegramUser, ApiError> {
    let fields: HashMap<String, String> = serde_urlencoded::from_str::<Vec<(String, String)>>(init_data)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let unauthorized = |m: &str| ApiError::status(401, m);

    let auth_date = fields.get("auth_date").ok_or_else(|| unauthorized("auth_date is missing"))?;
    let auth_date: i64 = auth_date.parse().map_err(|_| unauthorized("auth_date is invalid"))?;
    let now = Utc::now().timestamp();
    if now - auth_date > cfg.init_data_ttl {
        return Err(unauthorized("init_data is expired"));
    }
    if auth_date - now > cfg.init_data_future_skew {
        return Err(unauthorized("auth_date is in the future"));
    }

    let mut pairs: Vec<(&String, &String)> = fields.iter().filter(|(k, _)| k.as_str() != "hash").collect();
    pairs.sort();
    let check_string = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
    let mut secret = Hmac::<Sha256>::new_from_slice(b"WebAppData").expect("any key length");
    secret.update(cfg.bot_token.as_bytes());
    let secret = secret.finalize().into_bytes();
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).expect("any key length");
    mac.update(check_string.as_bytes());
    let valid = fields
        .get("hash")
        .and_then(|h| hex::decode(h).ok())
        .is_some_and(|h| mac.verify_slice(&h).is_ok());
    if !valid {
        return Err(unauthorized(if fields.contains_key("signature") { "invalid init_data signature" } else { "hash/signature is missing" }));
    }

    let user = fields.get("user").ok_or_else(|| unauthorized("user is missing"))?;
    let user: TelegramUser = serde_json::from_str(user).map_err(|_| unauthorized("user is missing"))?;
    if user.id.is_none() {
        return Err(unauthorized("user.id is missing"));
    }
    Ok(user)
}

pub fn session_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

pub fn session_cookie(token: &str, max_age: i64, secure: bool) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}", if secure { "; Secure" } else { "" })
}

/// Resolves the session cookie to a user and marks the session as used.
pub async fn current_user(db: &PgPool, headers: &HeaderMap) -> Result<CurrentUser, ApiError> {
    let Some(token) = session_token_from_headers(headers) else {
        return Err(ApiError::status(401, "Telegram Web App authorization required"));
    };
    let row = sqlx::query(
        "SELECT u.id, u.telegram_id, u.role, u.language_code, COALESCE(u.budget_owner_id, u.id)
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.token = $1 AND s.expires_at > $2",
    )
    .bind(&token)
    .bind(Utc::now().timestamp())
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Err(ApiError::status(401, "Unauthorized")) };
    sqlx::query("UPDATE sessions SET last_used_at = $1 WHERE token = $2")
        .bind(Utc::now().to_rfc3339())
        .bind(&token)
        .execute(db)
        .await?;
    Ok(CurrentUser {
        id: row.get(0),
        telegram_id: row.get(1),
        role: row.get(2),
        language_code: row.get(3),
        budget_owner_id: row.get(4),
        token,
    })
}

/// Finds the Telegram user (or the invitation made for their username), or creates a new
/// person with a budget of their own, and opens a session.
pub async fn login(db: &PgPool, cfg: &Config, tg: &TelegramUser, ip: &str, user_agent: Option<&str>) -> Result<(i64, String), ApiError> {
    let telegram_id = tg.id.expect("validated").to_string();
    let mut existing: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE telegram_id = $1")
        .bind(&telegram_id)
        .fetch_optional(db)
        .await?;
    if let (None, Some(username)) = (existing, tg.username.as_deref().filter(|u| !u.is_empty())) {
        existing = sqlx::query_scalar(
            "UPDATE users SET telegram_id = $1
             WHERE id = (SELECT id FROM users WHERE telegram_id = $2 ORDER BY id ASC LIMIT 1)
             RETURNING id",
        )
        .bind(&telegram_id)
        .bind(format!("pending:{}", username.to_lowercase()))
        .fetch_optional(db)
        .await?;
    }
    let user_id = match existing {
        Some(id) => {
            sqlx::query("UPDATE users SET username = $1, first_name = $2, last_name = $3, language_code = $4 WHERE id = $5")
                .bind(&tg.username).bind(&tg.first_name).bind(&tg.last_name).bind(&tg.language_code).bind(id)
                .execute(db)
                .await?;
            id
        }
        None => {
            // The configured owner, or on a fresh install without one, the first person to sign in.
            let has_owner: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'owner')")
                .fetch_one(db)
                .await?;
            // There is one owner: whoever set the install up (with a password or Telegram) first.
            let is_owner = !has_owner && cfg.owner_telegram_id.as_deref().is_none_or(|owner| owner == telegram_id);
            if !is_owner && !cfg.open_signup {
                return Err(ApiError::status(403, "Вас ещё не пригласили в бюджет"));
            }
            // A new person manages their own budget.
            let role = if is_owner { "owner" } else { "admin" };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO users (telegram_id, username, first_name, last_name, language_code, role, budget_owner_id)
                 VALUES ($1, $2, $3, $4, $5, $6, NULL) RETURNING id",
            )
            .bind(&telegram_id).bind(&tg.username).bind(&tg.first_name).bind(&tg.last_name).bind(&tg.language_code).bind(role)
            .fetch_one(db)
            .await?;
            sqlx::query("UPDATE users SET budget_owner_id = COALESCE(budget_owner_id, id) WHERE id = $1")
                .bind(id)
                .execute(db)
                .await?;
            id
        }
    };

    let budget: i64 = sqlx::query_scalar("SELECT COALESCE(budget_owner_id, id) FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(db)
        .await?;
    crate::db::seed_budget(db, budget).await?;
    let token = open_session(db, cfg, user_id, ip, user_agent).await?;
    Ok((user_id, token))
}

/// A new session for a user, by whatever way they signed in; only their newest ones are kept.
pub async fn open_session(db: &PgPool, cfg: &Config, user_id: i64, ip: &str, user_agent: Option<&str>) -> Result<String, ApiError> {
    // The thread-local generator must not live across an await.
    let (token, suffix) = {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::rng();
        let token = hex::encode(rng.random::<[u8; 32]>());
        let suffix: String = (0..8).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
        (token, suffix)
    };
    let expires_at = Utc::now().timestamp() + cfg.session_ttl;
    sqlx::query("INSERT INTO sessions (token, user_id, expires_at, ip_address, user_agent, token_suffix) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(&token).bind(user_id).bind(expires_at).bind(ip).bind(user_agent).bind(&suffix)
        .execute(db)
        .await?;
    // Keep only the newest sessions of this user.
    sqlx::query(
        "DELETE FROM sessions
         WHERE user_id = $1 AND token NOT IN (
             SELECT token FROM sessions
             WHERE user_id = $1
             ORDER BY expires_at DESC, created_at DESC, token DESC
             LIMIT $2
         )",
    )
    .bind(user_id)
    .bind(cfg.max_sessions)
    .execute(db)
    .await?;
    Ok(token)
}
