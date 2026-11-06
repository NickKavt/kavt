//! Accounts: where the money is, a card or cash, each in one currency (migrations/0005).
//! Income and expenses are paid from an account, a savings deposit leaves one and a
//! withdrawal comes back to one; a database trigger picks an account for operations that
//! don't name one. A transfer is two linked operations of type `transfer`, one per account,
//! each in its own account's currency, so money can move between currencies.
//!
//! An account's balance is its starting amount plus everything dated from its starting day
//! up to today. Savings accounts with interest live in src/savings.rs; the app lists both.

use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

pub const OUT: &str = "transfer_out";
pub const IN: &str = "transfer_in";

#[derive(Serialize)]
pub struct AccountOut {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub currency: String,
    /// Whose account it is; `None` is shared.
    pub owner_id: Option<i64>,
    pub owner_name: Option<String>,
    pub initial_amount: f64,
    pub initial_date: Option<String>,
    pub archived: bool,
    pub balance: f64,
    /// Operations on it; an account without any can be deleted rather than archived.
    pub used: i64,
    /// `regular` (a card, cash) or `savings`: money put aside, counted out of the month.
    pub kind: String,
    /// A savings account's own goal, in its currency.
    pub target_amount: Option<f64>,
    /// The month the goal is meant to be reached in, `YYYY-MM-01`.
    pub target_date: Option<String>,
    /// Interest of a savings account that earns it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interest: Option<crate::savings::InterestOut>,
}

#[derive(Deserialize)]
pub struct AccountIn {
    pub name: String,
    pub emoji: Option<String>,
    pub currency: String,
    pub owner_id: Option<i64>,
    #[serde(default)]
    pub initial_amount: f64,
    pub initial_date: Option<NaiveDate>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default = "regular")]
    pub kind: String,
    pub target_amount: Option<f64>,
    /// Absent keeps the date (a page that doesn't know dates); null removes it.
    #[serde(default, deserialize_with = "present")]
    pub target_date: Option<Option<NaiveDate>>,
    /// Interest settings of a savings account; `None` means it doesn't earn.
    pub interest: Option<crate::savings::AccountIn>,
}

/// A field that may be absent (outer `None`), null (`Some(None)`) or set.
pub fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// The first day of the goal's month, kept only with a goal.
fn goal_month(a: &AccountIn) -> Option<Option<String>> {
    if !a.target_amount.is_some_and(|t| t > 0.0) {
        return Some(None);
    }
    a.target_date.map(|d| d.and_then(|d| d.with_day(1)).map(|d| d.to_string()))
}

fn regular() -> String {
    "regular".into()
}

/// How an operation moves an account's money, in SQL over `transactions t`. A macro, so the
/// queries using it stay compile-time strings.
macro_rules! signed {
    () => {
        "CASE
            WHEN t.type = 'income' THEN t.amount
            WHEN t.type = 'expense' THEN -t.amount
            WHEN t.type = 'savings' AND t.source = 'withdrawal' THEN t.amount
            WHEN t.type = 'savings' THEN -t.amount
            WHEN t.source = 'transfer_in' THEN t.amount
            ELSE -t.amount END"
    };
}

/// Accounts of a budget with their balances today. A budget gets its first account, in its
/// main currency, the first time it asks.
pub async fn list(db: &PgPool, budget: i64, today: Option<NaiveDate>) -> sqlx::Result<Vec<AccountOut>> {
    // The device's day, within a day of the server's (a wrong clock can't hide or add much).
    let utc = Utc::now().date_naive();
    let today = today.filter(|d| (*d - utc).num_days().abs() <= 1).unwrap_or(utc);
    let any: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE budget_id = $1)").bind(budget).fetch_one(db).await?;
    if !any {
        let main = crate::rates::settings(db, budget).await?.main_currency;
        let name = crate::db::names(crate::db::budget_lang(db, budget).await?).main;
        // Two requests at once must not both make it; the lock is the one pick_account() takes.
        let mut tx = db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(7001, $1::int)").bind(budget).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO accounts (budget_id, name, emoji, currency)
             SELECT $1, $3, '💳', $2 WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE budget_id = $1)",
        )
        .bind(budget).bind(main).bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    const SQL: &str = concat!(
        "SELECT a.id, a.name, a.emoji, a.currency, a.owner_id,
                COALESCE(NULLIF(TRIM(CONCAT_WS(' ', u.first_name, u.last_name)), ''), u.username, u.login),
                a.initial_amount, a.initial_date, a.archived,
                a.initial_amount + COALESCE((SELECT SUM(", signed!(), ") FROM transactions t
                    WHERE t.account_id = a.id AND t.transaction_date <= $2
                      AND (a.initial_date IS NULL OR t.transaction_date >= a.initial_date)), 0),
                (SELECT COUNT(*) FROM transactions t WHERE t.account_id = a.id),
                a.kind, a.target_amount, a.interest_kind IS NOT NULL, a.target_date
         FROM accounts a LEFT JOIN users u ON u.id = a.owner_id
         WHERE a.budget_id = $1
         ORDER BY a.archived, a.position, a.id"
    );
    // Interest first: bringing it up to date can add payments and return an ended deposit.
    let interest = crate::savings::summaries(db, budget).await?;
    let rows = sqlx::query(SQL).bind(budget).bind(today.to_string()).fetch_all(db).await?;
    Ok(rows
        .iter()
        .map(|r| AccountOut {
            id: r.get(0),
            name: r.get(1),
            emoji: r.get(2),
            currency: r.get(3),
            owner_id: r.get(4),
            owner_name: r.get(5),
            initial_amount: r.get(6),
            initial_date: r.get(7),
            archived: r.get(8),
            balance: (r.get::<f64, _>(9) * 100.0).round() / 100.0,
            used: r.get(10),
            kind: r.get(11),
            target_amount: r.get(12),
            target_date: r.get(14),
            interest: interest.get(&r.get::<i64, _>(0)).cloned(),
        })
        .collect())
}

/// The currency of an account of this budget, or `None` when it isn't one.
pub async fn currency_of(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT currency FROM accounts WHERE id = $1 AND budget_id = $2").bind(id).bind(budget).fetch_optional(db).await
}

/// The owner must be someone in this budget.
async fn owner_fits(db: &PgPool, budget: i64, owner: Option<i64>) -> sqlx::Result<bool> {
    let Some(owner) = owner else { return Ok(true) };
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND COALESCE(budget_owner_id, id) = $2)")
        .bind(owner).bind(budget)
        .fetch_one(db)
        .await
}

pub enum Problem {
    Name,
    Currency,
    Owner,
    NotFound,
    /// The currency of an account with operations can't change: they are in that currency.
    CurrencyInUse,
    Kind,
    Interest(&'static str),
}

fn check(a: &AccountIn) -> Result<(String, String, String), Problem> {
    let name: String = a.name.trim().chars().take(40).collect();
    if name.is_empty() {
        return Err(Problem::Name);
    }
    let currency = crate::rates::symbol(&a.currency).to_string();
    if currency.is_empty() || currency.chars().count() > 8 {
        return Err(Problem::Currency);
    }
    let emoji = a.emoji.as_deref().map(str::trim).filter(|e| !e.is_empty()).unwrap_or("💳").chars().take(8).collect();
    if a.kind != "regular" && a.kind != "savings" {
        return Err(Problem::Kind);
    }
    if a.target_amount.is_some_and(|t| !t.is_finite() || t < 0.0) {
        return Err(Problem::Kind);
    }
    if a.kind == "regular" && a.interest.is_some() {
        return Err(Problem::Interest("Проценты бывают только у накопительного счёта"));
    }
    if let Some(i) = &a.interest {
        i.check().map_err(Problem::Interest)?;
    }
    Ok((name, emoji, currency))
}

pub async fn create(db: &PgPool, budget: i64, a: &AccountIn) -> sqlx::Result<Result<i64, Problem>> {
    let (name, emoji, currency) = match check(a) {
        Ok(v) => v,
        Err(p) => return Ok(Err(p)),
    };
    if !owner_fits(db, budget, a.owner_id).await? {
        return Ok(Err(Problem::Owner));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO accounts (budget_id, name, emoji, currency, owner_id, initial_amount, initial_date, kind, target_amount, position, target_date)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, (SELECT COALESCE(MAX(position), 0) + 1 FROM accounts WHERE budget_id = $1), $10)
         RETURNING id",
    )
    .bind(budget).bind(name).bind(emoji).bind(currency).bind(a.owner_id).bind(a.initial_amount).bind(a.initial_date.map(|d| d.to_string()))
    .bind(&a.kind).bind(a.target_amount.filter(|t| *t > 0.0)).bind(goal_month(a).flatten())
    .fetch_one(db)
    .await?;
    if let Some(i) = &a.interest {
        crate::savings::configure(db, id, i).await?;
    }
    Ok(Ok(id))
}

pub async fn update(db: &PgPool, budget: i64, id: i64, a: &AccountIn) -> sqlx::Result<Result<(), Problem>> {
    let (name, emoji, currency) = match check(a) {
        Ok(v) => v,
        Err(p) => return Ok(Err(p)),
    };
    let Some(old) = currency_of(db, budget, id).await? else { return Ok(Err(Problem::NotFound)) };
    if !owner_fits(db, budget, a.owner_id).await? {
        return Ok(Err(Problem::Owner));
    }
    if old != currency {
        // Its operations, and recurring ones that book on it, are in the old currency.
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM transactions WHERE account_id = $1)
                 OR EXISTS (SELECT 1 FROM recurring WHERE account_id = $1 OR to_account_id = $1)",
        )
        .bind(id)
        .fetch_one(db)
        .await?;
        if used {
            return Ok(Err(Problem::CurrencyInUse));
        }
    }
    sqlx::query(
        "UPDATE accounts SET name = $3, emoji = $4, currency = $5, owner_id = $6, initial_amount = $7, initial_date = $8, archived = $9,
                kind = $10, target_amount = $11, target_date = CASE WHEN $12 THEN $13 ELSE target_date END
         WHERE id = $1 AND budget_id = $2",
    )
    .bind(id).bind(budget).bind(name).bind(emoji).bind(currency).bind(a.owner_id).bind(a.initial_amount)
    .bind(a.initial_date.map(|d| d.to_string())).bind(a.archived).bind(&a.kind).bind(a.target_amount.filter(|t| *t > 0.0))
    .bind(goal_month(a).is_some()).bind(goal_month(a).flatten())
    .execute(db)
    .await?;
    let earning: bool = sqlx::query_scalar("SELECT interest_kind IS NOT NULL FROM accounts WHERE id = $1").bind(id).fetch_one(db).await?;
    match &a.interest {
        Some(i) => crate::savings::configure(db, id, i).await?,
        None if earning => crate::savings::stop(db, id).await?,
        None => {}
    }
    Ok(Ok(()))
}

/// An account without operations is deleted; one with them is archived, so its history stays.
pub async fn remove(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<&'static str>> {
    // One a recurring transfer goes to is kept (archived), so the transfer shows it is stalled
    // instead of losing where it went.
    let used: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM transactions WHERE account_id = a.id) OR EXISTS (SELECT 1 FROM recurring WHERE to_account_id = a.id)
         FROM accounts a WHERE a.id = $1 AND a.budget_id = $2",
    )
        .bind(id).bind(budget)
        .fetch_optional(db)
        .await?;
    let Some(used) = used else { return Ok(None) };
    if used {
        sqlx::query("UPDATE accounts SET archived = TRUE WHERE id = $1").bind(id).execute(db).await?;
        Ok(Some("archived"))
    } else {
        sqlx::query("DELETE FROM accounts WHERE id = $1").bind(id).execute(db).await?;
        Ok(Some("deleted"))
    }
}

#[derive(Deserialize)]
pub struct TransferIn {
    pub from_id: i64,
    pub to_id: i64,
    /// Leaves the first account, in its currency.
    pub amount: f64,
    /// Arrives in the second, in its currency; the same as `amount` when both are one currency.
    pub to_amount: Option<f64>,
    pub note: Option<String>,
    pub transaction_date: Option<NaiveDate>,
}

/// Both halves of a transfer: (out id, in id).
pub async fn transfer(db: &PgPool, budget: i64, user: i64, t: &TransferIn) -> sqlx::Result<Result<(i64, i64), &'static str>> {
    if t.from_id == t.to_id {
        return Ok(Err("Выберите два разных счёта"));
    }
    let (Some(from), Some(to)) = (currency_of(db, budget, t.from_id).await?, currency_of(db, budget, t.to_id).await?) else {
        return Ok(Err("Счёт не найден"));
    };
    let to_amount = if from == to { t.amount } else { t.to_amount.unwrap_or(0.0) };
    if !(t.amount.is_finite() && t.amount > 0.0 && t.amount <= 1e12 && to_amount.is_finite() && to_amount > 0.0 && to_amount <= 1e12) {
        return Ok(Err("Сумма должна быть больше нуля"));
    }
    let day = t.transaction_date.unwrap_or_else(|| Utc::now().date_naive()).to_string();
    let note = t.note.as_deref().map(str::trim).unwrap_or("").chars().take(200).collect::<String>();
    let mut tx = db.begin().await?;
    let insert = "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source)
                  VALUES ($1, $2, 'transfer', $3, $4, $5, $6, $7) RETURNING id";
    let out: i64 = sqlx::query_scalar(insert).bind(user).bind(t.from_id).bind(t.amount).bind(&from).bind(&note).bind(&day).bind(OUT).fetch_one(&mut *tx).await?;
    let into: i64 = sqlx::query_scalar(insert).bind(user).bind(t.to_id).bind(to_amount).bind(&to).bind(&note).bind(&day).bind(IN).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE transactions SET pair_id = CASE id WHEN $1 THEN $2 ELSE $1 END WHERE id IN ($1, $2)").bind(out).bind(into).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Ok((out, into)))
}

/// The halves of a transfer of this budget, (out id, in id), from either half.
async fn halves(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<(i64, i64)>> {
    let row: Option<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, t.source, t.pair_id FROM transactions t JOIN accounts a ON a.id = t.account_id
         WHERE t.id = $1 AND t.type = 'transfer' AND a.budget_id = $2",
    )
    .bind(id).bind(budget)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(id, source, pair)| pair.map(|p| if source == OUT { (id, p) } else { (p, id) })))
}

/// Changes a transfer: both accounts, both amounts, the day and the description.
pub async fn update_transfer(db: &PgPool, budget: i64, id: i64, t: &TransferIn) -> sqlx::Result<Result<(), &'static str>> {
    let Some((out, into)) = halves(db, budget, id).await? else { return Ok(Err("Перевод не найден")) };
    if t.from_id == t.to_id {
        return Ok(Err("Выберите два разных счёта"));
    }
    let (Some(from), Some(to)) = (currency_of(db, budget, t.from_id).await?, currency_of(db, budget, t.to_id).await?) else {
        return Ok(Err("Счёт не найден"));
    };
    let to_amount = if from == to { t.amount } else { t.to_amount.unwrap_or(0.0) };
    if !(t.amount.is_finite() && t.amount > 0.0 && t.amount <= 1e12 && to_amount.is_finite() && to_amount > 0.0 && to_amount <= 1e12) {
        return Ok(Err("Сумма должна быть больше нуля"));
    }
    let note = t.note.as_deref().map(str::trim).unwrap_or("").chars().take(200).collect::<String>();
    let day = t.transaction_date.map(|d| d.to_string());
    let mut tx = db.begin().await?;
    let set = "UPDATE transactions SET account_id = $2, amount = $3, currency = $4, note = $5, transaction_date = COALESCE($6, transaction_date) WHERE id = $1";
    sqlx::query(set).bind(out).bind(t.from_id).bind(t.amount).bind(&from).bind(&note).bind(&day).execute(&mut *tx).await?;
    sqlx::query(set).bind(into).bind(t.to_id).bind(to_amount).bind(&to).bind(&note).bind(&day).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn delete_transfer(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<bool> {
    let Some((out, into)) = halves(db, budget, id).await? else { return Ok(false) };
    sqlx::query("DELETE FROM transactions WHERE id IN ($1, $2)").bind(out).bind(into).execute(db).await?;
    Ok(true)
}

#[derive(Serialize)]
pub struct AccountOperation {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub amount: f64,
    pub currency: String,
    pub note: Option<String>,
    pub transaction_date: String,
    /// How it moved this account's money: negative is out.
    pub signed: f64,
    pub category_name: Option<String>,
    pub category_emoji: Option<String>,
    pub subcategory_name: Option<String>,
    pub actor_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<i64>,
    /// For a transfer: the other account, its amount and currency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_account_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_currency: Option<String>,
    /// The top-level category of an income or expense, to open it for editing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<i64>,
}

/// The latest operations of one account, newest first.
pub async fn operations(db: &PgPool, budget: i64, id: i64, limit: i64) -> sqlx::Result<Option<Vec<AccountOperation>>> {
    if currency_of(db, budget, id).await?.is_none() {
        return Ok(None);
    }
    const SQL: &str = concat!(
        "SELECT t.id, t.type, t.amount, t.currency, t.note, t.transaction_date, ", signed!(), ",
                r.name, r.emoji, CASE WHEN c.parent_id IS NULL THEN NULL ELSE c.name END,
                COALESCE(u.first_name, u.username, u.login), t.source, t.pair_id,
                p.account_id, pa.emoji || ' ' || pa.name, p.amount, p.currency, r.id
         FROM transactions t
         LEFT JOIN categories c ON c.id = t.category_id
         LEFT JOIN categories r ON r.id = COALESCE(c.parent_id, c.id)
         LEFT JOIN users u ON u.id = t.user_id
         LEFT JOIN transactions p ON p.id = t.pair_id AND t.type = 'transfer'
         LEFT JOIN accounts pa ON pa.id = p.account_id
         WHERE t.account_id = $1 AND NOT (t.source = 'interest_fixed' AND t.amount = 0)
         ORDER BY t.transaction_date DESC, t.created_at DESC, t.id DESC
         LIMIT $2"
    );
    let rows = sqlx::query(SQL).bind(id).bind(limit).fetch_all(db).await?;
    Ok(Some(
        rows.iter()
            .map(|r| AccountOperation {
                id: r.get(0),
                kind: r.get(1),
                amount: r.get(2),
                currency: r.get(3),
                note: r.get(4),
                transaction_date: r.get(5),
                signed: r.get(6),
                category_name: r.get(7),
                category_emoji: r.get(8),
                subcategory_name: r.get(9),
                actor_name: r.get(10),
                source: r.get(11),
                pair_id: r.get(12),
                other_account_id: r.get(13),
                other_account: r.get(14),
                other_amount: r.get(15),
                other_currency: r.get(16),
                category_id: r.get(17),
            })
            .collect(),
    ))
}

/// Money put aside in the month (by currency): what moved from everyday accounts into savings
/// ones, less what came back, plus income on savings accounts (interest) less their expenses.
/// Bounds are `YYYY-MM-DD`, `from` inclusive (None: from the start), `to` exclusive.
pub async fn saved(db: &PgPool, budget: i64, from: Option<&str>, to: &str) -> sqlx::Result<crate::db::Totals> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT t.currency, COALESCE(SUM(CASE
            WHEN a.kind = 'regular' AND t.type = 'transfer' AND pa.kind = 'savings' AND t.source = 'transfer_out' THEN t.amount
            WHEN a.kind = 'regular' AND t.type = 'transfer' AND pa.kind = 'savings' AND t.source = 'transfer_in' THEN -t.amount
            WHEN a.kind = 'savings' AND t.type = 'income' THEN t.amount
            WHEN a.kind = 'savings' AND t.type = 'expense' THEN -t.amount
            ELSE 0 END), 0.0)
         FROM transactions t
         JOIN accounts a ON a.id = t.account_id
         LEFT JOIN transactions p ON p.id = t.pair_id
         LEFT JOIN accounts pa ON pa.id = p.account_id
         WHERE a.budget_id = $1 AND ($2::TEXT IS NULL OR t.transaction_date >= $2) AND t.transaction_date < $3
         GROUP BY t.currency",
    )
    .bind(budget).bind(from).bind(to)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Money on savings accounts by currency up to `until` (exclusive), each from its starting balance.
pub async fn savings_totals(db: &PgPool, budget: i64, until: &str) -> sqlx::Result<crate::db::Totals> {
    totals_of_kind(db, budget, "savings", until).await
}

/// Money on everyday accounts by currency up to `until` (exclusive): what the account list
/// shows, added up by currency.
pub async fn everyday_totals(db: &PgPool, budget: i64, until: &str) -> sqlx::Result<crate::db::Totals> {
    totals_of_kind(db, budget, "regular", until).await
}

async fn totals_of_kind(db: &PgPool, budget: i64, kind: &str, until: &str) -> sqlx::Result<crate::db::Totals> {
    const SQL: &str = concat!(
        "SELECT a.currency, SUM(a.initial_amount + COALESCE((SELECT SUM(", signed!(), ") FROM transactions t
            WHERE t.account_id = a.id AND t.transaction_date < $2
              AND (a.initial_date IS NULL OR t.transaction_date >= a.initial_date)), 0))
         FROM accounts a
         WHERE a.budget_id = $1 AND a.kind = $3 AND (a.initial_date IS NULL OR a.initial_date < $2)
         GROUP BY a.currency"
    );
    let rows: Vec<(String, f64)> = sqlx::query_as(SQL).bind(budget).bind(until).bind(kind).fetch_all(db).await?;
    Ok(rows.into_iter().map(|(c, v)| (c, (v * 100.0).round() / 100.0)).collect())
}

/// Money exchanged between everyday accounts of different currencies in `[from, to)`, by
/// currency: what arrived in it minus what left it.
pub async fn exchanged(db: &PgPool, budget: i64, from: &str, to: &str) -> sqlx::Result<crate::db::Totals> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT t.currency, SUM(CASE WHEN t.source = 'transfer_in' THEN t.amount ELSE -t.amount END)
         FROM transactions t
         JOIN accounts a ON a.id = t.account_id
         JOIN transactions p ON p.id = t.pair_id
         JOIN accounts pa ON pa.id = p.account_id
         WHERE a.budget_id = $1 AND t.type = 'transfer' AND a.kind = 'regular' AND pa.kind = 'regular'
           AND t.currency <> p.currency AND t.transaction_date >= $2 AND t.transaction_date < $3
           AND (a.initial_date IS NULL OR t.transaction_date >= a.initial_date)
         GROUP BY t.currency",
    )
    .bind(budget).bind(from).bind(to)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Starting balances of everyday accounts dated in `[from, to)`, by currency.
pub async fn opened(db: &PgPool, budget: i64, from: &str, to: &str) -> sqlx::Result<crate::db::Totals> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT currency, SUM(initial_amount) FROM accounts
         WHERE budget_id = $1 AND kind = 'regular' AND initial_amount <> 0 AND initial_date >= $2 AND initial_date < $3
         GROUP BY currency",
    )
    .bind(budget).bind(from).bind(to)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}
