//! Two-factor sign-in: after the password, a six-digit code from an authenticator app
//! (RFC 6238: HMAC-SHA1, 30-second steps), or one of ten one-time recovery codes.
//!
//! - `/api/2fa/setup` makes a secret and shows it as a QR code; nothing changes yet.
//! - `/api/2fa/enable` with a code from the app turns it on and returns the recovery codes,
//!   shown once; only their hashes are kept.
//! - Turning it off or making new recovery codes asks for a current code again.
//! - `budget-server reset-2fa <login>` turns it off from the server, for a lost phone.
//!
//! A code is accepted one step early or late, for clocks that differ, and only once.

use axum::{Json, extract::State, http::HeaderMap};
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use qrcode::{QrCode, render::svg};
use rand::RngExt;
use serde::Deserialize;
use serde_json::json;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::{ApiError, ApiResult, AppRef, user};

/// What the sign-in answers when the password was right and a code is needed; the page
/// recognises it and asks for the code.
pub const CODE_NEEDED: &str = "Нужен код из приложения";
const STEP: i64 = 30;
const RECOVERY_CODES: usize = 10;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0);
    for &b in bytes {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        let v = ALPHABET.iter().position(|a| *a == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

fn code_at(secret: &[u8], step: i64) -> u32 {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let at = (h[19] & 0x0f) as usize;
    (u32::from_be_bytes([h[at], h[at + 1], h[at + 2], h[at + 3]]) & 0x7fff_ffff) % 1_000_000
}

/// The step a six-digit code belongs to, if it is right now or one step either side.
fn matching_step(secret_b32: &str, code: &str, now: i64) -> Option<i64> {
    let secret = unbase32(secret_b32)?;
    let code: u32 = code.parse().ok()?;
    let step = now / STEP;
    (step - 1..=step + 1).find(|s| code_at(&secret, *s) == code)
}

fn digits(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect()
}

fn recovery_hash(code: &str) -> String {
    hex::encode(Sha256::digest(digits(code).to_lowercase().as_bytes()))
}

/// A code from the app, used up so it can't be used again.
async fn use_app_code(db: &PgPool, user_id: i64, secret: &str, code: &str) -> sqlx::Result<bool> {
    let Some(step) = matching_step(secret, code, Utc::now().timestamp()) else { return Ok(false) };
    let done = sqlx::query("UPDATE users SET totp_step = $2 WHERE id = $1 AND (totp_step IS NULL OR totp_step < $2)")
        .bind(user_id).bind(step)
        .execute(db)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Checks the second factor of someone whose password was right. Without two-factor sign-in
/// turned on, there is nothing to check.
pub async fn check(db: &PgPool, user_id: i64, code: Option<&str>) -> ApiResult<()> {
    let secret: Option<String> = sqlx::query_scalar("SELECT totp_secret FROM users WHERE id = $1").bind(user_id).fetch_one(db).await?;
    let Some(secret) = secret else { return Ok(()) };
    let Some(code) = code.map(digits).filter(|c| !c.is_empty()) else { return Err(ApiError::status(401, CODE_NEEDED)) };
    let good = if code.chars().all(|c| c.is_ascii_digit()) {
        use_app_code(db, user_id, &secret, &code).await?
    } else {
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1 AND code_hash = $2")
            .bind(user_id).bind(recovery_hash(&code))
            .execute(db)
            .await?
            .rows_affected()
            == 1
    };
    if good { Ok(()) } else { Err(ApiError::status(401, "Неверный код")) }
}

pub async fn enabled(db: &PgPool, user_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = $1").bind(user_id).fetch_one(db).await
}

/// New recovery codes in place of the old ones; returned once.
async fn new_recovery_codes(db: &PgPool, user_id: i64) -> sqlx::Result<Vec<String>> {
    // The generator isn't kept across an await: it can't move between threads.
    let codes: Vec<String> = (0..RECOVERY_CODES)
        .map(|_| {
            let raw = base32(&rand::rng().random::<[u8; 7]>()).to_lowercase();
            format!("{}-{}", &raw[..5], &raw[5..10])
        })
        .collect();
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1").bind(user_id).execute(&mut *tx).await?;
    for c in &codes {
        sqlx::query("INSERT INTO recovery_codes (user_id, code_hash) VALUES ($1, $2)").bind(user_id).bind(recovery_hash(c)).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(codes)
}

/// A secret to add to the authenticator app: as a QR code, a link that opens the app on this
/// phone, and as text to type in.
pub async fn setup(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let (login, on): (Option<String>, bool) = sqlx::query_as("SELECT login, totp_secret IS NOT NULL FROM users WHERE id = $1")
        .bind(me.id)
        .fetch_one(&state.db)
        .await?;
    let Some(login) = login else { return Err(ApiError::status(400, "У вас вход через Telegram, второй шаг не нужен")) };
    if on {
        return Err(ApiError::status(409, "Двухэтапный вход уже включён"));
    }
    let secret = base32(&rand::rng().random::<[u8; 20]>());
    sqlx::query("UPDATE users SET totp_pending = $2 WHERE id = $1").bind(me.id).bind(&secret).execute(&state.db).await?;
    let uri = format!("otpauth://totp/Budget:{login}?secret={secret}&issuer=Budget&algorithm=SHA1&digits=6&period={STEP}");
    let qr = QrCode::new(uri.as_bytes())
        .map(|q| q.render::<svg::Color<'_>>().min_dimensions(200, 200).quiet_zone(true).build())
        .map_err(|_| ApiError::status(500, "QR failed"))?;
    Ok(Json(json!({ "secret": secret, "uri": uri, "qr_svg": qr })))
}

#[derive(Deserialize)]
pub struct CodeIn {
    code: String,
}

/// Turns two-factor sign-in on once the app shows the right code; returns the recovery codes.
pub async fn enable(State(state): AppRef, headers: HeaderMap, Json(c): Json<CodeIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let pending: Option<String> = sqlx::query_scalar("SELECT totp_pending FROM users WHERE id = $1").bind(me.id).fetch_one(&state.db).await?;
    let Some(secret) = pending else { return Err(ApiError::status(400, "Сначала добавьте ключ в приложение")) };
    let Some(step) = matching_step(&secret, &digits(&c.code), Utc::now().timestamp()) else {
        return Err(ApiError::status(400, "Неверный код"));
    };
    sqlx::query("UPDATE users SET totp_secret = totp_pending, totp_pending = NULL, totp_step = $2 WHERE id = $1 AND totp_pending = $3")
        .bind(me.id).bind(step).bind(&secret)
        .execute(&state.db)
        .await?;
    let codes = new_recovery_codes(&state.db, me.id).await?;
    tracing::info!(user = me.id, "two-factor sign-in on");
    Ok(Json(json!({ "ok": true, "recovery_codes": codes })))
}

/// The current code (or a recovery code) is asked again before anything is changed.
async fn confirm(state: &crate::AppState, user_id: i64, code: &str) -> ApiResult<()> {
    if !enabled(&state.db, user_id).await? {
        return Err(ApiError::status(400, "Двухэтапный вход выключен"));
    }
    check(&state.db, user_id, Some(code)).await.map_err(|_| ApiError::status(400, "Неверный код"))
}

pub async fn disable(State(state): AppRef, headers: HeaderMap, Json(c): Json<CodeIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    confirm(&state, me.id, &c.code).await?;
    turn_off(&state.db, me.id).await?;
    tracing::info!(user = me.id, "two-factor sign-in off");
    Ok(Json(json!({ "ok": true })))
}

pub async fn recovery_codes(State(state): AppRef, headers: HeaderMap, Json(c): Json<CodeIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    confirm(&state, me.id, &c.code).await?;
    Ok(Json(json!({ "recovery_codes": new_recovery_codes(&state.db, me.id).await? })))
}

async fn turn_off(db: &PgPool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_step = NULL WHERE id = $1").bind(user_id).execute(db).await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1").bind(user_id).execute(db).await?;
    Ok(())
}

/// `budget-server reset-2fa <login>`: for someone who lost both the phone and the codes.
pub async fn reset(db: &PgPool, login: &str) -> anyhow::Result<()> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE login = $1").bind(login.trim().to_lowercase()).fetch_optional(db).await?;
    let id = id.ok_or_else(|| anyhow::anyhow!("no user with the login {login}"))?;
    turn_off(db, id).await?;
    println!("Two-factor sign-in is off for {login}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 appendix B: the key "12345678901234567890", eight digits; the last six here.
        let key = b"12345678901234567890";
        assert_eq!(code_at(key, 59 / 30), 287_082);
        assert_eq!(code_at(key, 1_111_111_109 / 30), 81_804);
        assert_eq!(code_at(key, 2_000_000_000 / 30), 279_037);
    }

    #[test]
    fn base32_round_trip() {
        let key = b"12345678901234567890";
        assert_eq!(base32(key), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(unbase32("gezdgnbvgy3tqojqgezdgnbvgy3tqojq").unwrap(), key);
    }

    #[test]
    fn codes_one_step_either_side() {
        let secret = base32(b"12345678901234567890");
        let code = format!("{:06}", code_at(b"12345678901234567890", 1000));
        assert_eq!(matching_step(&secret, &code, 1000 * 30 + 29), Some(1000));
        assert_eq!(matching_step(&secret, &code, 1001 * 30), Some(1000));
        assert_eq!(matching_step(&secret, &code, 1002 * 30), None);
    }
}
