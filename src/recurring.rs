//! Recurring operations: rent, subscriptions, salary. Each one is added on its day by the
//! server, when the budget is opened and once an hour in the background, like interest.
//!
//! The n-th operation falls on `start_date` plus n periods, counted from the start each time,
//! so a payment on the 31st lands on the last day of a short month and comes back to the 31st.
//! An operation that was added and then deleted is not added again.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use chrono::{Days, Months, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};

use crate::{ApiError, ApiResult, AppRef, check_amount, ok, user};

/// At most this many are caught up at once, for a start date far in the past.
const CATCH_UP: i64 = 400;

fn occurrence(start: NaiveDate, every: &str, n: i64) -> Option<NaiveDate> {
    let n = u32::try_from(n).ok()?;
    match every {
        "week" => start.checked_add_days(Days::new(7 * u64::from(n))),
        "month" => start.checked_add_months(Months::new(n)),
        _ => start.checked_add_months(Months::new(n.checked_mul(12)?)),
    }
}

/// Adds what is due today or earlier in one budget, or in all of them.
pub async fn sync_budget(db: &PgPool, budget: i64) -> sqlx::Result<()> {
    sync(db, Some(budget)).await
}

pub async fn sync_all(db: &PgPool) -> sqlx::Result<()> {
    sync(db, None).await
}

async fn sync(db: &PgPool, budget: Option<i64>) -> sqlx::Result<()> {
    let today = Utc::now().date_naive();
    let due: Vec<i64> = sqlx::query_scalar("SELECT id FROM recurring WHERE next_date <= $1 AND ($2::BIGINT IS NULL OR budget_id = $2)")
        .bind(today.to_string()).bind(budget)
        .fetch_all(db)
        .await?;
    for id in due {
        catch_up(db, id, today).await?;
    }
    Ok(())
}

/// Whether a recurring row `r` is a transfer `catch_up` can't make (the same conditions).
macro_rules! STALLED {
    () => {
        "(r.type = 'transfer' AND (
            NOT EXISTS (SELECT 1 FROM accounts x WHERE x.id = r.to_account_id AND x.currency = r.currency AND NOT x.archived)
            OR (r.account_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM accounts x WHERE x.id = r.account_id AND x.currency = r.currency AND NOT x.archived))
            OR (r.account_id IS NULL AND NOT EXISTS (SELECT 1 FROM accounts x WHERE x.budget_id = r.budget_id AND x.kind = 'regular' AND x.currency = r.currency AND NOT x.archived))))"
    };
}

async fn catch_up(db: &PgPool, id: i64, today: NaiveDate) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    // Locked, so the hourly run and someone opening the app can't add the same day twice.
    let row: Option<(Option<i64>, i64, String, f64, String, Option<String>, String, String, i64, Option<i64>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT user_id, category_id, type, amount, currency, note, every, start_date, done, account_id, to_account_id, budget_id
         FROM recurring WHERE id = $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id, category, kind, amount, currency, note, every, start, mut done, account, to_account, budget)) = row else { return Ok(()) };
    // A recurring transfer (into savings): from its account, or the everyday one in its currency.
    let transfer_from: Option<i64> = if kind == "transfer" {
        match account {
            Some(a) => sqlx::query_scalar("SELECT id FROM accounts WHERE id = $1 AND currency = $2 AND NOT archived")
                .bind(a).bind(&currency)
                .fetch_optional(&mut *tx)
                .await?,
            None => sqlx::query_scalar(
                "SELECT id FROM accounts WHERE budget_id = $1 AND kind = 'regular' AND currency = $2 AND NOT archived ORDER BY position, id LIMIT 1",
            )
            .bind(budget).bind(&currency)
            .fetch_optional(&mut *tx)
            .await?,
        }
    } else {
        None
    };
    let to_currency: Option<String> = match to_account {
        Some(a) => sqlx::query_scalar("SELECT currency FROM accounts WHERE id = $1 AND NOT archived").bind(a).fetch_optional(&mut *tx).await?,
        None => None,
    };
    if kind == "transfer" && (transfer_from.is_none() || to_currency.as_deref() != Some(currency.as_str())) {
        // The list shows it as stalled (see `STALLED`) until its accounts are fixed.
        tracing::warn!(recurring = id, "recurring transfer without accounts in its currency; skipped");
        return Ok(());
    }
    let Ok(start) = NaiveDate::parse_from_str(&start, "%Y-%m-%d") else { return Ok(()) };
    let mut added = 0;
    while let Some(day) = occurrence(start, &every, done).filter(|d| *d <= today && added < CATCH_UP) {
        if kind == "transfer" {
            let insert = "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, recurring_id)
                          VALUES ($1, $2, 'transfer', $3, $4, $5, $6, $7, $8) RETURNING id";
            let out: i64 = sqlx::query_scalar(insert).bind(user_id).bind(transfer_from).bind(amount).bind(&currency).bind(&note).bind(day.to_string())
                .bind(crate::accounts::OUT).bind(id).fetch_one(&mut *tx).await?;
            let into: i64 = sqlx::query_scalar(insert).bind(user_id).bind(to_account).bind(amount).bind(&currency).bind(&note).bind(day.to_string())
                .bind(crate::accounts::IN).bind(id).fetch_one(&mut *tx).await?;
            sqlx::query("UPDATE transactions SET pair_id = CASE id WHEN $1 THEN $2 ELSE $1 END WHERE id IN ($1, $2)").bind(out).bind(into).execute(&mut *tx).await?;
            done += 1;
            added += 1;
            continue;
        }
        sqlx::query(
            "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, transaction_date, recurring_id, account_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(user_id).bind(category).bind(&kind).bind(amount).bind(&currency).bind(&note).bind(day.to_string()).bind(id).bind(account)
        .execute(&mut *tx)
        .await?;
        done += 1;
        added += 1;
    }
    let next = occurrence(start, &every, done).map_or_else(|| "9999-12-31".to_string(), |d| d.to_string());
    sqlx::query("UPDATE recurring SET done = $2, next_date = $3 WHERE id = $1").bind(id).bind(done).bind(next).execute(&mut *tx).await?;
    tx.commit().await
}

#[derive(Serialize)]
pub struct RecurringOut {
    id: i64,
    category_id: i64,
    /// The category on the home screen, and the subcategory when it is booked on one.
    root_id: i64,
    category_name: String,
    emoji: String,
    subcategory_name: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    amount: f64,
    currency: String,
    note: Option<String>,
    every: String,
    start_date: String,
    next_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<i64>,
    /// A transfer that can't be made: an account it moves between is gone, archived or in
    /// another currency. It waits, adding nothing, until that is fixed.
    stalled: bool,
}

pub async fn list(State(state): AppRef, headers: HeaderMap) -> ApiResult<Json<Vec<RecurringOut>>> {
    let me = user(&state, &headers).await?;
    sync_budget(&state.db, me.budget_owner_id).await?;
    const SQL: &str = concat!(
        "SELECT r.id, r.category_id, COALESCE(p.id, c.id) AS root_id, COALESCE(p.name, c.name) AS category_name,
                COALESCE(p.emoji, c.emoji) AS emoji, CASE WHEN p.id IS NULL THEN NULL ELSE c.name END AS subcategory_name,
                r.type, r.amount, r.currency, r.note, r.every, r.start_date, r.next_date, r.account_id, ", STALLED!(), "
         FROM recurring r JOIN categories c ON c.id = r.category_id LEFT JOIN categories p ON p.id = c.parent_id
         WHERE r.budget_id = $1 ORDER BY r.next_date, r.id"
    );
    let rows = sqlx::query(SQL)
    .bind(me.budget_owner_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.iter()
            .map(|r| RecurringOut {
                id: r.get(0), category_id: r.get(1), root_id: r.get(2), category_name: r.get(3), emoji: r.get(4),
                subcategory_name: r.get(5), kind: r.get(6), amount: r.get(7), currency: r.get(8), note: r.get(9),
                every: r.get(10), start_date: r.get(11), next_date: r.get(12), account_id: r.get(13), stalled: r.get(14),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct RecurringIn {
    /// A category or subcategory of the budget; the operation's type is that of its category.
    category_id: i64,
    amount: f64,
    currency: String,
    note: Option<String>,
    every: String,
    start_date: NaiveDate,
    account_id: Option<i64>,
}

fn check_every(every: &str) -> ApiResult<()> {
    if matches!(every, "week" | "month" | "year") { Ok(()) } else { Err(ApiError::status(400, "Неизвестный период")) }
}

fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty())
}

/// A new recurring operation. If it starts today or earlier, what is due is added at once.
pub async fn create(State(state): AppRef, headers: HeaderMap, Json(r): Json<RecurringIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let budget = me.budget_owner_id;
    check_amount(r.amount, &r.currency)?;
    check_every(&r.every)?;
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(p.type, c.type) FROM categories c LEFT JOIN categories p ON p.id = c.parent_id WHERE c.id = $1 AND c.budget_id = $2",
    )
    .bind(r.category_id).bind(budget)
    .fetch_optional(&state.db)
    .await?;
    let Some(kind) = kind else { return Err(ApiError::status(404, "Категория не найдена")) };
    if kind == "savings" {
        return Err(ApiError::status(400, "Сбережения пополняются переводом на накопительный счёт"));
    }
    check_account(&state, budget, r.account_id, &r.currency).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO recurring (budget_id, user_id, category_id, type, amount, currency, note, every, start_date, next_date, account_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, $10) RETURNING id",
    )
    .bind(budget).bind(me.id).bind(r.category_id).bind(kind).bind(r.amount).bind(crate::rates::symbol(&r.currency)).bind(clean_note(r.note))
    .bind(&r.every).bind(r.start_date.to_string()).bind(r.account_id)
    .fetch_one(&state.db)
    .await?;
    catch_up(&state.db, id, Utc::now().date_naive()).await?;
    Ok(Json(json!({ "id": id, "ok": true })))
}

#[derive(Deserialize)]
pub struct RecurringUpdate {
    amount: Option<f64>,
    currency: Option<String>,
    note: Option<String>,
    account_id: Option<i64>,
}

/// An account named for it must be the budget's, in its currency.
async fn check_account(state: &crate::AppState, budget: i64, account: Option<i64>, currency: &str) -> ApiResult<()> {
    let Some(id) = account else { return Ok(()) };
    match crate::accounts::currency_of(&state.db, budget, id).await? {
        None => Err(ApiError::status(404, "Счёт не найден")),
        Some(c) if c != crate::rates::symbol(currency) => Err(ApiError::status(400, "У счёта другая валюта")),
        Some(_) => Ok(()),
    }
}

/// A new amount, currency or description applies from the next operation; added ones stay.
pub async fn update(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>, Json(u): Json<RecurringUpdate>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let current: Option<(f64, String)> = sqlx::query_as("SELECT amount, currency FROM recurring WHERE id = $1 AND budget_id = $2")
        .bind(id).bind(me.budget_owner_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((amount, currency)) = current else { return Err(not_found()) };
    if let Some(c) = &u.currency {
        // A transfer moves money between two accounts of one currency: its currency is theirs.
        let to: Option<String> = sqlx::query_scalar(
            "SELECT a.currency FROM recurring r JOIN accounts a ON a.id = r.to_account_id WHERE r.id = $1 AND r.type = 'transfer'",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
        if to.is_some_and(|to| to != crate::rates::symbol(c)) {
            return Err(ApiError::status(400, "Валюта регулярного перевода — валюта счёта, на который он идёт"));
        }
    }
    check_amount(u.amount.unwrap_or(amount), u.currency.as_deref().unwrap_or(&currency))?;
    check_account(&state, me.budget_owner_id, u.account_id, u.currency.as_deref().unwrap_or(&currency)).await?;
    sqlx::query(
        "UPDATE recurring SET amount = COALESCE($2, amount), currency = COALESCE($3, currency),
            note = CASE WHEN $4::BOOLEAN THEN $5 ELSE note END,
            -- A new currency without an account of its own: the trigger picks one for each operation.
            account_id = CASE WHEN $6::BIGINT IS NOT NULL THEN $6 WHEN $3::TEXT IS NOT NULL THEN NULL ELSE account_id END
         WHERE id = $1",
    )
    .bind(id).bind(u.amount).bind(u.currency.map(|c| crate::rates::symbol(&c))).bind(u.note.is_some()).bind(clean_note(u.note)).bind(u.account_id)
    .execute(&state.db)
    .await?;
    Ok(ok())
}

/// Stops it; the operations already added stay in the budget.
pub async fn stop(State(state): AppRef, headers: HeaderMap, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    let done = sqlx::query("DELETE FROM recurring WHERE id = $1 AND budget_id = $2").bind(id).bind(me.budget_owner_id).execute(&state.db).await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(ok())
}

fn not_found() -> ApiError {
    ApiError::status(404, "Регулярная операция не найдена")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn month_end_does_not_drift() {
        let start = d("2026-01-31");
        assert_eq!(occurrence(start, "month", 1), Some(d("2026-02-28")));
        assert_eq!(occurrence(start, "month", 2), Some(d("2026-03-31")));
        assert_eq!(occurrence(start, "month", 3), Some(d("2026-04-30")));
    }

    #[test]
    fn weeks_and_years() {
        assert_eq!(occurrence(d("2026-11-03"), "week", 2), Some(d("2026-11-17")));
        assert_eq!(occurrence(d("2024-02-29"), "year", 1), Some(d("2025-02-28")));
        assert_eq!(occurrence(d("2024-02-29"), "year", 4), Some(d("2028-02-29")));
    }
}
