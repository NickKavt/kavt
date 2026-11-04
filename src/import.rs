//! CSV export and import.
//!
//! The export is every operation of the budget, one per line, in a form spreadsheets and other
//! budget apps read. The import takes rows the app has already brought to this form from any
//! CSV (its own export, Firefly III, Actual, ezBookkeeping, a bank): it finds categories and
//! subcategories by name, creating the missing ones, and skips rows imported before.

use std::collections::HashMap;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};

use crate::{ApiError, ApiResult, AppRef, check_amount, db, user};

pub const MAX_ROWS: usize = 50_000;
const COLUMNS: [&str; 8] = ["date", "type", "category", "subcategory", "amount", "currency", "note", "author"];

/// A text cell. Cells that a spreadsheet would run as a formula get a leading apostrophe.
fn cell(out: &mut String, text: &str) {
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{text}") } else { text.to_string() };
    if text.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(&text);
    }
}

/// `type` is expense, income, savings (a deposit), withdrawal (out of savings) or interest.
pub async fn export(State(state): AppRef, headers: HeaderMap) -> ApiResult<Response> {
    let me = user(&state, &headers).await?;
    let rows = sqlx::query(
        "SELECT t.transaction_date, t.type, t.source, COALESCE(p.name, c.name), CASE WHEN p.id IS NULL THEN '' ELSE c.name END,
                t.amount, t.currency, COALESCE(t.note, ''), COALESCE(u.first_name, u.login, u.username, ''), t.id
         FROM transactions t JOIN categories c ON c.id = t.category_id LEFT JOIN categories p ON p.id = c.parent_id
         LEFT JOIN users u ON u.id = t.user_id
         WHERE c.budget_id = $1
         UNION ALL
         -- Money put into or taken out of a savings account, from the everyday account's side.
         SELECT t.transaction_date, 'savings', CASE WHEN t.source = 'transfer_in' THEN 'withdrawal' END, 'Сбережения', sa.name,
                t.amount, t.currency, COALESCE(t.note, ''), COALESCE(u.first_name, u.login, u.username, ''), t.id
         FROM transactions t
         JOIN accounts a ON a.id = t.account_id AND a.kind = 'regular'
         JOIN transactions o ON o.id = t.pair_id
         JOIN accounts sa ON sa.id = o.account_id AND sa.kind = 'savings'
         LEFT JOIN users u ON u.id = t.user_id
         WHERE t.type = 'transfer' AND a.budget_id = $1
         ORDER BY 1, 10",
    )
    .bind(me.budget_owner_id)
    .fetch_all(&state.db)
    .await?;
    // A byte-order mark, so Excel reads the Cyrillic right.
    let mut out = String::from("\u{feff}");
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for r in rows {
        let kind: String = r.get(1);
        let source: Option<String> = r.get(2);
        let kind = match source.as_deref() {
            Some("withdrawal") => "withdrawal",
            Some("interest" | "interest_fixed") => "interest",
            _ => kind.as_str(),
        };
        let amount: f64 = r.get(5);
        out.push_str(&r.get::<String, _>(0));
        out.push(',');
        out.push_str(kind);
        for i in [3, 4] {
            out.push(',');
            cell(&mut out, &r.get::<String, _>(i));
        }
        out.push(',');
        out.push_str(&format!("{amount}"));
        for i in 6..=8 {
            out.push(',');
            cell(&mut out, &r.get::<String, _>(i));
        }
        out.push('\n');
    }
    let name = format!("attachment; filename=\"budget-{}.csv\"", Utc::now().date_naive());
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8")),
            (header::CONTENT_DISPOSITION, HeaderValue::from_str(&name).unwrap_or(HeaderValue::from_static("attachment"))),
        ],
        out,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct ImportRow {
    date: NaiveDate,
    /// expense, income, savings, withdrawal or interest (added as a deposit).
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    subcategory: String,
    amount: f64,
    currency: String,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize)]
pub struct ImportIn {
    rows: Vec<ImportRow>,
    /// Only count what would happen.
    #[serde(default)]
    dry_run: bool,
}

/// What the file calls a currency, as the app writes it.
fn currency(raw: &str) -> String {
    let c = raw.trim();
    match c.to_uppercase().as_str() {
        "TRY" | "TL" | "₺" => "₺".into(),
        "USD" | "$" => "$".into(),
        "EUR" | "€" => "€".into(),
        "RUB" | "RUR" | "₽" | "РУБ" => "₽".into(),
        _ => crate::rates::symbol(c),
    }
}

fn clean(text: &str, max: usize) -> String {
    let text = text.trim();
    let text = text.strip_prefix('\'').filter(|t| t.starts_with(['=', '+', '-', '@'])).unwrap_or(text);
    text.chars().take(max).collect()
}

/// Ids of categories found or created during one import, by (type, parent, lowercased name).
struct Categories<'a> {
    budget: i64,
    known: HashMap<(String, Option<i64>, String), i64>,
    /// The income and savings categories: (id, name) by type.
    system: HashMap<String, (i64, String)>,
    created: Vec<String>,
    tx: &'a mut Transaction<'static, Postgres>,
}

impl Categories<'_> {
    /// The category of a row: (id, name).
    async fn root(&mut self, kind: &str, name: &str) -> sqlx::Result<(i64, String)> {
        // Income and savings have one category each; what the file names becomes a subcategory.
        if kind != "expense" {
            if let Some(found) = self.system.get(kind) {
                return Ok(found.clone());
            }
            let row: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM categories WHERE parent_id IS NULL AND type = $1 AND budget_id = $2 ORDER BY id LIMIT 1")
                .bind(kind).bind(self.budget)
                .fetch_optional(&mut **self.tx)
                .await?;
            let row = match row {
                Some(row) => row,
                None => {
                    let (name, emoji, color) = if kind == "income" { ("Доход", "💰", "#9ece6a") } else { ("Сбережения", "🎯", "#2ac3de") };
                    let id = sqlx::query_scalar("INSERT INTO categories (name, emoji, color, type, budget_id) VALUES ($1, $2, $3, $4, $5) RETURNING id")
                        .bind(name).bind(emoji).bind(color).bind(kind).bind(self.budget)
                        .fetch_one(&mut **self.tx)
                        .await?;
                    (id, name.to_string())
                }
            };
            self.system.insert(kind.to_string(), row.clone());
            return Ok(row);
        }
        let name = if name.is_empty() { "Другое" } else { name };
        let key = (kind.to_string(), None, name.to_lowercase());
        if let Some(id) = self.known.get(&key) {
            return Ok((*id, name.to_string()));
        }
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM categories WHERE parent_id IS NULL AND type = 'expense' AND budget_id = $1 AND lower(name) = $2 ORDER BY id LIMIT 1",
        )
        .bind(self.budget).bind(&key.2)
        .fetch_optional(&mut **self.tx)
        .await?;
        let id = match found {
            Some(id) => id,
            None => {
                self.created.push(name.to_string());
                sqlx::query_scalar("INSERT INTO categories (name, emoji, color, type, budget_id) VALUES ($1, '📦', '#545c7e', 'expense', $2) RETURNING id")
                    .bind(name).bind(self.budget)
                    .fetch_one(&mut **self.tx)
                    .await?
            }
        };
        self.known.insert(key, id);
        Ok((id, name.to_string()))
    }

    /// A subcategory shows from its earliest month in the file, so imported history has a place.
    async fn sub(&mut self, kind: &str, parent: i64, parent_name: &str, name: &str, month: &str) -> sqlx::Result<i64> {
        let key = (kind.to_string(), Some(parent), name.to_lowercase());
        if let Some(id) = self.known.get(&key) {
            sqlx::query("UPDATE categories SET visible_from = LEAST(visible_from, $2) WHERE id = $1").bind(id).bind(month).execute(&mut **self.tx).await?;
            return Ok(*id);
        }
        let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE parent_id = $1 AND lower(name) = $2 ORDER BY id LIMIT 1")
            .bind(parent).bind(&key.2)
            .fetch_optional(&mut **self.tx)
            .await?;
        let id = match found {
            Some(id) => {
                sqlx::query("UPDATE categories SET visible_from = LEAST(visible_from, $2) WHERE id = $1").bind(id).bind(month).execute(&mut **self.tx).await?;
                id
            }
            None => {
                self.created.push(format!("{parent_name} / {name}"));
                sqlx::query_scalar(
                    "INSERT INTO categories (name, emoji, color, type, parent_id, visible_from, budget_id) VALUES ($1, '•', '#545c7e', $2, $3, $4, $5) RETURNING id",
                )
                .bind(name).bind(kind).bind(parent).bind(month).bind(self.budget)
                .fetch_one(&mut **self.tx)
                .await?
            }
        };
        self.known.insert(key, id);
        Ok(id)
    }
}

pub async fn import(State(state): AppRef, headers: HeaderMap, Json(body): Json<ImportIn>) -> ApiResult<Json<serde_json::Value>> {
    let me = user(&state, &headers).await?;
    if body.rows.len() > MAX_ROWS {
        return Err(ApiError::status(400, "Слишком много строк, разделите файл"));
    }
    let budget = me.budget_owner_id;
    let mut tx = state.db.begin().await?;
    let mut cats = Categories { budget, known: HashMap::new(), system: HashMap::new(), created: Vec::new(), tx: &mut tx };
    let (mut added, mut skipped) = (0u64, 0u64);
    // The same row twice in one file is two operations (two coffees on one day), so each copy
    // gets its own number in the fingerprint.
    let mut seen: HashMap<String, u32> = HashMap::new();
    for (line, r) in body.rows.iter().enumerate() {
        let bad = |why: &str| ApiError::status(400, &format!("Строка {}: {why}", line + 1));
        let (kind, source) = match r.kind.as_str() {
            "expense" => ("expense", None),
            "income" => ("income", None),
            "savings" | "interest" => ("savings", None),
            "withdrawal" => ("savings", Some("withdrawal")),
            _ => return Err(bad("неизвестный тип")),
        };
        let cur = currency(&r.currency);
        check_amount(r.amount, &cur).map_err(|_| bad("сумма или валюта"))?;
        let amount = (r.amount * 100.0).round() / 100.0;
        let category = clean(&r.category, 60);
        let mut subcategory = clean(&r.subcategory, 60);
        let note = clean(&r.note, 200);
        let date = r.date.to_string();
        let (root, root_name) = cats.root(kind, &category).await?;
        // For income and savings the file's category is the subcategory, unless it is the
        // category itself («Доход», «Сбережения», as the app's own export has them).
        if kind != "expense" && subcategory.is_empty() && category.to_lowercase() != root_name.to_lowercase() {
            subcategory = category.clone();
        }
        // Savings rows go to savings accounts below, not to a savings subcategory.
        let target = if subcategory.is_empty() || kind == "savings" {
            root
        } else {
            cats.sub(kind, root, &root_name, &subcategory, &db::month_bounds(r.date).0).await?
        };
        let base = format!("{budget}\u{1f}{date}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{amount}\u{1f}{cur}\u{1f}{note}", r.kind, category.to_lowercase(), subcategory.to_lowercase());
        let n = seen.entry(base.clone()).or_default();
        *n += 1;
        let key = hex::encode(Sha256::digest(format!("{base}\u{1f}{n}").as_bytes()));
        if kind == "savings" {
            // Savings are accounts: the file's subcategory (or its category, unless that is
            // «Сбережения» itself) names the savings account.
            let name = if !subcategory.is_empty() { subcategory.clone() } else if !category.is_empty() && category.to_lowercase() != root_name.to_lowercase() { category.clone() } else { root_name.clone() };
            let savings = savings_account(&mut cats, &name, &cur).await?;
            let fresh = if r.kind == "interest" {
                let pct = crate::savings::interest_category(cats.tx, budget).await?;
                sqlx::query(
                    "INSERT INTO transactions (user_id, category_id, account_id, type, amount, currency, note, transaction_date, source, import_key)
                     VALUES ($1, $2, $3, 'income', $4, $5, NULLIF($6, ''), $7, 'interest_fixed', $8) ON CONFLICT (import_key) WHERE import_key IS NOT NULL DO NOTHING",
                )
                .bind(me.id).bind(pct).bind(savings).bind(amount).bind(&cur).bind(&note).bind(&date).bind(&key)
                .execute(&mut **cats.tx)
                .await?
                .rows_affected() == 1
            } else {
                let regular = regular_account(&mut cats, me.id, &cur).await?;
                let (from, to) = if source == Some("withdrawal") { (savings, regular) } else { (regular, savings) };
                let out: Option<i64> = sqlx::query_scalar(
                    "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, import_key)
                     VALUES ($1, $2, 'transfer', $3, $4, NULLIF($5, ''), $6, 'transfer_out', $7) ON CONFLICT (import_key) WHERE import_key IS NOT NULL DO NOTHING RETURNING id",
                )
                .bind(me.id).bind(from).bind(amount).bind(&cur).bind(&note).bind(&date).bind(&key)
                .fetch_optional(&mut **cats.tx)
                .await?;
                if let Some(out) = out {
                    let into: i64 = sqlx::query_scalar(
                        "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, pair_id)
                         VALUES ($1, $2, 'transfer', $3, $4, NULLIF($5, ''), $6, 'transfer_in', $7) RETURNING id",
                    )
                    .bind(me.id).bind(to).bind(amount).bind(&cur).bind(&note).bind(&date).bind(out)
                    .fetch_one(&mut **cats.tx)
                    .await?;
                    sqlx::query("UPDATE transactions SET pair_id = $2 WHERE id = $1").bind(out).bind(into).execute(&mut **cats.tx).await?;
                }
                out.is_some()
            };
            if fresh { added += 1 } else { skipped += 1 }
            continue;
        }
        let done = sqlx::query(
            "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, transaction_date, source, import_key)
             VALUES ($1, $2, $3, $4, $5, NULLIF($6, ''), $7, $8, $9) ON CONFLICT (import_key) WHERE import_key IS NOT NULL DO NOTHING",
        )
        .bind(me.id).bind(target).bind(kind).bind(amount).bind(&cur).bind(&note).bind(&date).bind(source).bind(key)
        .execute(&mut **cats.tx)
        .await?;
        if done.rows_affected() == 1 { added += 1 } else { skipped += 1 }
    }
    let created = std::mem::take(&mut cats.created);
    if body.dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
        tracing::info!(budget, added, skipped, "operations imported");
        // Rates of the imported operations, each on its own day, in the background.
        let db = state.db.clone();
        tokio::spawn(async move {
            // A batch at a time; a source that doesn't answer is left for the hourly run.
            while let Ok(n) = crate::rates::fill(&db, Some(budget)).await {
                if n == 0 {
                    break;
                }
            }
        });
    }
    Ok(Json(json!({ "added": added, "skipped": skipped, "created": created, "dry_run": body.dry_run })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_quoted_and_defused() {
        let mut s = String::new();
        cell(&mut s, "a, \"b\"");
        assert_eq!(s, "\"a, \"\"b\"\"\"");
        let mut s = String::new();
        cell(&mut s, "=HYPERLINK(1)");
        assert_eq!(s, "'=HYPERLINK(1)");
        assert_eq!(clean("'=HYPERLINK(1)", 60), "=HYPERLINK(1)");
        assert_eq!(clean("'quoted", 60), "'quoted");
    }

    #[test]
    fn currency_codes() {
        assert_eq!(currency("usd"), "$");
        assert_eq!(currency("TRY"), "₺");
        assert_eq!(currency("GBP"), "GBP");
    }
}

/// The savings account of that name and currency, made when the file is the first to name it.
async fn savings_account(cats: &mut Categories<'_>, name: &str, currency: &str) -> sqlx::Result<i64> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE budget_id = $1 AND kind = 'savings' AND currency = $2 AND lower(name) = lower($3)
         ORDER BY archived, id LIMIT 1",
    )
    .bind(cats.budget).bind(currency).bind(name)
    .fetch_optional(&mut **cats.tx)
    .await?;
    if let Some(id) = found {
        return Ok(id);
    }
    sqlx::query_scalar(
        "INSERT INTO accounts (budget_id, name, emoji, currency, kind, position)
         VALUES ($1, $2, '🎯', $3, 'savings', (SELECT COALESCE(MAX(position), 0) + 1 FROM accounts WHERE budget_id = $1)) RETURNING id",
    )
    .bind(cats.budget).bind(name).bind(currency)
    .fetch_one(&mut **cats.tx)
    .await
}

/// The everyday account in that currency money for savings came from: the person's own, else shared, else a new one.
async fn regular_account(cats: &mut Categories<'_>, user: i64, currency: &str) -> sqlx::Result<i64> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE budget_id = $1 AND kind = 'regular' AND currency = $2 AND NOT archived
         ORDER BY (owner_id IS NOT DISTINCT FROM $3) DESC, (owner_id IS NULL) DESC, position, id LIMIT 1",
    )
    .bind(cats.budget).bind(currency).bind(user)
    .fetch_optional(&mut **cats.tx)
    .await?;
    if let Some(id) = found {
        return Ok(id);
    }
    sqlx::query_scalar(
        "INSERT INTO accounts (budget_id, name, emoji, currency, position)
         VALUES ($1, 'Основной ' || $2, '💳', $2, (SELECT COALESCE(MAX(position), 0) + 1 FROM accounts WHERE budget_id = $1)) RETURNING id",
    )
    .bind(cats.budget).bind(currency)
    .fetch_one(&mut **cats.tx)
    .await
}
