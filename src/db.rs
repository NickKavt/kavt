//! Database access. The SQL follows the original budgetbot-rust-v4 (recovered from its
//! binary), with one change: every budget has its own categories (`categories.budget_id`),
//! and an operation belongs to the budget of its category. A budget is identified by the id
//! of the user who owns it.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row, postgres::PgRow};

/// Currencies every summary lists even when nothing was spent in them.
pub const DEFAULT_CURRENCIES: [&str; 4] = ["₺", "$", "€", "₽"];

pub type Totals = BTreeMap<String, f64>;

pub fn zero_filled() -> Totals {
    DEFAULT_CURRENCIES.iter().map(|c| (c.to_string(), 0.0)).collect()
}

/// First day of the month containing `day`, and the first day of the next month, as the
/// `YYYY-MM-DD` strings stored in `transactions.transaction_date`.
pub fn month_bounds(day: NaiveDate) -> (String, String) {
    let start = NaiveDate::from_ymd_opt(day.year(), day.month(), 1).unwrap();
    let next = if day.month() == 12 {
        NaiveDate::from_ymd_opt(day.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(day.year(), day.month() + 1, 1).unwrap()
    };
    (start.to_string(), next.to_string())
}

pub fn current_month_start() -> String {
    month_bounds(Utc::now().date_naive()).0
}

/// `2026-10-27 19:29:01.925814+00` (Postgres text) -> `2026-10-27T19:29:01.925814Z`.
pub fn db_datetime_to_rfc3339(raw: &str) -> String {
    let parsed = DateTime::parse_from_str(&format!("{raw}00"), "%Y-%m-%d %H:%M:%S%.f%z")
        .or_else(|_| DateTime::parse_from_rfc3339(raw));
    match parsed {
        Ok(dt) => dt.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        Err(_) => raw.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Seed data
// ---------------------------------------------------------------------------
const SEED_EXPENSES: [(&str, &str, &str); 5] = [
    ("Продукты", "🛒", "#e0af68"),
    ("Здоровье", "🏥", "#9ece6a"),
    ("Квартира", "🏠", "#6183bb"),
    ("Развлечения", "🎉", "#bb9af7"),
    ("Другое", "📦", "#545c7e"),
];

/// A budget without categories (a new person, or one who left a shared budget and never had
/// their own) gets the standard set. Existing budgets are left exactly as their users made them.
pub async fn seed_budget(db: &PgPool, budget: i64) -> sqlx::Result<()> {
    let has_any: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM categories WHERE budget_id = $1)")
        .bind(budget)
        .fetch_one(db)
        .await?;
    if has_any {
        return Ok(());
    }
    ensure_system_root(db, budget, "income").await?;
    ensure_system_root(db, budget, "savings").await?;
    for (name, emoji, color) in SEED_EXPENSES {
        sqlx::query("INSERT INTO categories (name, emoji, color, type, budget_id) VALUES ($1, $2, $3, 'expense', $4)")
            .bind(name).bind(emoji).bind(color).bind(budget)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// The single root category of a system type (income or savings) in a budget, created if
/// missing; extra roots of that type are removed. Returns its id.
pub async fn ensure_system_root(db: &PgPool, budget: i64, kind: &str) -> sqlx::Result<i64> {
    let (name, emoji, color, target) = match kind {
        "income" => ("Доход", "💰", "#9ece6a", None),
        _ => ("Сбережения", "🎯", "#2ac3de", Some(0.0)),
    };
    let keep: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM categories WHERE parent_id IS NULL AND type = $1 AND budget_id = $2 ORDER BY id ASC LIMIT 1",
    )
    .bind(kind).bind(budget)
    .fetch_optional(db)
    .await?;
    match keep {
        Some(id) => {
            sqlx::query("UPDATE categories SET color = COALESCE(NULLIF(color, ''), $1), target_amount = COALESCE(target_amount, $2) WHERE id = $3")
                .bind(color).bind(target).bind(id)
                .execute(db)
                .await?;
            sqlx::query("DELETE FROM categories WHERE parent_id IS NULL AND type = $1 AND budget_id = $2 AND id != $3")
                .bind(kind).bind(budget).bind(id)
                .execute(db)
                .await?;
            Ok(id)
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO categories (name, emoji, color, type, target_amount, budget_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            )
            .bind(name).bind(emoji).bind(color).bind(kind).bind(target).bind(budget)
            .fetch_one(db)
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Users and budgets
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct UserOut {
    pub id: i64,
    pub telegram_id: String,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language_code: Option<String>,
    pub role: String,
}

fn user_from_row(r: &PgRow) -> UserOut {
    UserOut {
        id: r.get("id"),
        telegram_id: r.get("telegram_id"),
        username: r.get("username"),
        first_name: r.get("first_name"),
        last_name: r.get("last_name"),
        language_code: r.get("language_code"),
        role: r.get("role"),
    }
}

pub async fn user_by_id(db: &PgPool, id: i64) -> sqlx::Result<Option<UserOut>> {
    let row = sqlx::query("SELECT id, telegram_id, username, first_name, last_name, language_code, role FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn user_by_telegram_id(db: &PgPool, telegram_id: &str) -> sqlx::Result<Option<UserOut>> {
    let row = sqlx::query("SELECT id, telegram_id, username, first_name, last_name, language_code, role FROM users WHERE telegram_id = $1")
        .bind(telegram_id)
        .fetch_optional(db)
        .await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn list_budget_users(db: &PgPool, owner_id: i64) -> sqlx::Result<Vec<UserOut>> {
    let rows = sqlx::query(
        "SELECT id, telegram_id, username, first_name, last_name, language_code, role
         FROM users
         WHERE id = $1 OR COALESCE(budget_owner_id, id) = $1
         ORDER BY created_at ASC, id ASC",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(user_from_row).collect())
}

pub async fn owner_exists(db: &PgPool) -> sqlx::Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'owner'").fetch_one(db).await?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct CategoryRow {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub color: String,
    pub kind: String,
    pub target_amount: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct CategoryOut {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub color: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub total: f64,
    pub totals_by_currency: Totals,
    pub target_amount: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct SubcategoryOut {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub color: String,
}

fn category_from_row(r: &PgRow) -> CategoryRow {
    CategoryRow {
        id: r.get("id"),
        name: r.get("name"),
        emoji: r.get("emoji"),
        color: r.get("color"),
        kind: r.get("type"),
        target_amount: r.get("target_amount"),
    }
}

/// Any category (root or subcategory) of the budget.
pub async fn category_row(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<CategoryRow>> {
    let row = sqlx::query("SELECT id, name, emoji, color, type, target_amount FROM categories WHERE id = $1 AND budget_id = $2")
        .bind(id)
        .bind(budget)
        .fetch_optional(db)
        .await?;
    Ok(row.as_ref().map(category_from_row))
}

pub async fn root_category_row(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<CategoryRow>> {
    let row = sqlx::query("SELECT id, name, emoji, color, type, target_amount FROM categories WHERE id = $1 AND parent_id IS NULL AND budget_id = $2")
        .bind(id)
        .bind(budget)
        .fetch_optional(db)
        .await?;
    Ok(row.as_ref().map(category_from_row))
}

pub async fn root_categories(db: &PgPool, budget: i64) -> sqlx::Result<Vec<CategoryRow>> {
    let rows = sqlx::query("SELECT id, name, emoji, color, type, target_amount FROM categories WHERE parent_id IS NULL AND budget_id = $1 ORDER BY type ASC, id ASC")
        .bind(budget)
        .fetch_all(db)
        .await?;
    Ok(rows.iter().map(category_from_row).collect())
}

/// Per root category and currency: the sum of its own and its subcategories' operations of
/// the category's type, optionally limited to one month. A category without operations gets
/// one row with a NULL currency and 0.
async fn root_totals(db: &PgPool, budget: i64, month: Option<&(String, String)>) -> sqlx::Result<Vec<(i64, Option<String>, f64)>> {
    let rows = match month {
        Some((from, to)) => sqlx::query(
            "WITH RECURSIVE category_tree(root_id, id) AS (SELECT id, id FROM categories WHERE parent_id IS NULL UNION ALL SELECT ct.root_id, c.id FROM categories c JOIN category_tree ct ON c.parent_id = ct.id)
             SELECT r.id, t.currency, COALESCE(SUM(CASE WHEN t.source = 'withdrawal' THEN -t.amount ELSE t.amount END), 0.0) AS total
             FROM categories r
             LEFT JOIN category_tree ct ON ct.root_id = r.id
             LEFT JOIN transactions t ON t.category_id = ct.id AND t.type = r.type
                  AND t.transaction_date >= $2 AND t.transaction_date < $3
             WHERE r.parent_id IS NULL AND r.budget_id = $1
             GROUP BY r.id, t.currency ORDER BY r.id ASC, t.currency ASC",
        )
        .bind(budget).bind(from).bind(to)
        .fetch_all(db)
        .await?,
        None => sqlx::query(
            "WITH RECURSIVE category_tree(root_id, id) AS (SELECT id, id FROM categories WHERE parent_id IS NULL UNION ALL SELECT ct.root_id, c.id FROM categories c JOIN category_tree ct ON c.parent_id = ct.id)
             SELECT r.id, t.currency, COALESCE(SUM(CASE WHEN t.source = 'withdrawal' THEN -t.amount ELSE t.amount END), 0.0) AS total
             FROM categories r
             LEFT JOIN category_tree ct ON ct.root_id = r.id
             LEFT JOIN transactions t ON t.category_id = ct.id AND t.type = r.type
             WHERE r.parent_id IS NULL AND r.budget_id = $1
             GROUP BY r.id, t.currency ORDER BY r.id ASC, t.currency ASC",
        )
        .bind(budget)
        .fetch_all(db)
        .await?,
    };
    Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect())
}

fn category_out(c: &CategoryRow, rows: &[(i64, Option<String>, f64)], zero_fill: bool) -> CategoryOut {
    let mut totals = if zero_fill { zero_filled() } else { Totals::new() };
    let mut total = -0.0;
    for (_, currency, amount) in rows.iter().filter(|(id, _, _)| *id == c.id) {
        total += amount;
        if let Some(cur) = currency {
            *totals.entry(cur.clone()).or_insert(0.0) += amount;
        }
    }
    CategoryOut {
        id: c.id,
        name: c.name.clone(),
        emoji: c.emoji.clone(),
        color: c.color.clone(),
        kind: c.kind.clone(),
        total,
        totals_by_currency: totals,
        target_amount: c.target_amount,
    }
}

/// Root categories with their totals; `month` = None means all time.
pub async fn list_categories(db: &PgPool, budget: i64, month: Option<&(String, String)>) -> sqlx::Result<Vec<CategoryOut>> {
    let cats = root_categories(db, budget).await?;
    let rows = root_totals(db, budget, month).await?;
    Ok(cats.iter().map(|c| category_out(c, &rows, true)).collect())
}

/// One root category with its all-time totals (only the currencies actually used).
pub async fn category_out_all_time(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<CategoryOut>> {
    let Some(c) = category_row(db, budget, id).await? else { return Ok(None) };
    let rows = root_totals(db, budget, None).await?;
    Ok(Some(category_out(&c, &rows, false)))
}

#[derive(Debug, Serialize)]
pub struct ChartRow {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub color: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub currency: Option<String>,
    pub total: f64,
}

pub async fn stats_by_category(db: &PgPool, budget: i64, month: &(String, String)) -> sqlx::Result<Vec<ChartRow>> {
    let rows = sqlx::query(
        "WITH RECURSIVE category_tree(root_id, id) AS (SELECT id, id FROM categories WHERE parent_id IS NULL UNION ALL SELECT ct.root_id, c.id FROM categories c JOIN category_tree ct ON c.parent_id = ct.id)
         SELECT r.id, r.name, r.emoji, r.color, r.type, t.currency, COALESCE(SUM(CASE WHEN t.source = 'withdrawal' THEN -t.amount ELSE t.amount END), 0.0) AS total
         FROM categories r
         LEFT JOIN category_tree ct ON ct.root_id = r.id
         LEFT JOIN transactions t ON t.category_id = ct.id AND t.type = r.type
              AND t.transaction_date >= $2 AND t.transaction_date < $3
         WHERE r.parent_id IS NULL AND r.budget_id = $1
         GROUP BY r.id, r.name, r.emoji, r.color, r.type, t.currency ORDER BY r.id ASC, t.currency ASC",
    )
    .bind(budget).bind(&month.0).bind(&month.1)
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .map(|r| ChartRow {
            id: r.get(0),
            name: r.get(1),
            emoji: r.get(2),
            color: r.get(3),
            kind: r.get(4),
            currency: r.get(5),
            total: r.get(6),
        })
        .collect())
}

pub async fn child_ids(db: &PgPool, id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT id FROM categories WHERE parent_id = $1").bind(id).fetch_all(db).await
}

pub async fn visible_subcategories(db: &PgPool, parent: i64, month_start: &str) -> sqlx::Result<Vec<SubcategoryOut>> {
    let rows = sqlx::query(
        "SELECT id, name, emoji, color
         FROM categories
         WHERE parent_id = $1
           AND COALESCE(visible_from, substr(created_at, 1, 7) || '-01') <= $2
           AND (archived_from IS NULL OR archived_from = '' OR archived_from > $2)
         ORDER BY id ASC",
    )
    .bind(parent).bind(month_start)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(|r| SubcategoryOut { id: r.get(0), name: r.get(1), emoji: r.get(2), color: r.get(3) }).collect())
}

pub async fn subcategory(db: &PgPool, budget: i64, id: i64) -> sqlx::Result<Option<SubcategoryOut>> {
    let row = sqlx::query("SELECT id, name, emoji, color FROM categories WHERE id = $1 AND parent_id IS NOT NULL AND budget_id = $2")
        .bind(id)
        .bind(budget)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| SubcategoryOut { id: r.get(0), name: r.get(1), emoji: r.get(2), color: r.get(3) }))
}


// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------
#[derive(Debug, Serialize)]
pub struct TransactionOut {
    pub id: i64,
    pub amount: f64,
    pub currency: String,
    pub note: Option<String>,
    pub is_debt: bool,
    pub created_at: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub transaction_date: String,
    pub subcategory_name: Option<String>,
    pub actor_name: Option<String>,
    /// `withdrawal` or interest (`interest`, `interest_fixed`); left out for ordinary operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The other half of a transfer between savings accounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<i64>,
}

/// Money taken out of savings: a savings operation that counts negative.
pub const WITHDRAWAL: &str = "withdrawal";

pub async fn category_transactions(db: &PgPool, kind: &str, ids: &[i64], month: &(String, String)) -> sqlx::Result<Vec<TransactionOut>> {
    let rows = sqlx::query(
        "SELECT t.id, t.amount, t.currency, t.note, t.is_debt, t.created_at, t.type, t.transaction_date, c.name,
                COALESCE(u.first_name, u.username, u.telegram_id), t.source, t.pair_id
         FROM transactions t
         LEFT JOIN categories c ON c.id = t.category_id
         LEFT JOIN users u ON u.id = t.user_id
         WHERE t.type = $1 AND t.category_id = ANY($2)
           AND t.transaction_date >= $3 AND t.transaction_date < $4
         ORDER BY t.created_at DESC, t.id DESC",
    )
    .bind(kind).bind(ids).bind(&month.0).bind(&month.1)
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .map(|r| TransactionOut {
            id: r.get(0),
            amount: r.get(1),
            currency: r.get(2),
            note: r.get(3),
            is_debt: r.get::<i64, _>(4) != 0,
            created_at: db_datetime_to_rfc3339(&r.get::<String, _>(5)),
            kind: r.get(6),
            transaction_date: r.get(7),
            subcategory_name: r.get(8),
            actor_name: r.get(9),
            source: r.get(10),
            pair_id: r.get(11),
        })
        .collect())
}

pub async fn category_currency_totals(db: &PgPool, kind: &str, ids: &[i64], month: &(String, String)) -> sqlx::Result<Totals> {
    let rows = sqlx::query(
        "SELECT currency, COALESCE(SUM(CASE WHEN source = 'withdrawal' THEN -amount ELSE amount END), 0.0) FROM transactions
         WHERE type = $1 AND category_id = ANY($2) AND transaction_date >= $3 AND transaction_date < $4
         GROUP BY currency",
    )
    .bind(kind).bind(ids).bind(&month.0).bind(&month.1)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(|r| (r.get::<String, _>(0), r.get::<f64, _>(1))).collect())
}

/// Sums by currency of one operation type in a month, starting from the default currencies.
async fn month_sums(db: &PgPool, kind: &str, budget: i64, month: &(String, String)) -> sqlx::Result<Totals> {
    let rows = sqlx::query(
        "SELECT t.currency, COALESCE(SUM(CASE WHEN t.source = 'withdrawal' THEN -t.amount ELSE t.amount END), 0.0) FROM transactions t JOIN categories c ON c.id = t.category_id
         WHERE t.type = $1 AND t.transaction_date >= $2 AND t.transaction_date < $3 AND c.budget_id = $4
         GROUP BY t.currency",
    )
    .bind(kind).bind(&month.0).bind(&month.1).bind(budget)
    .fetch_all(db)
    .await?;
    let mut out = zero_filled();
    for r in rows {
        out.insert(r.get(0), r.get(1));
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct MonthSummary {
    pub income: Totals,
    pub expense: Totals,
    pub carry_in: Totals,
    pub balance: Totals,
}

pub async fn month_summary(db: &PgPool, budget: i64, month: &(String, String)) -> sqlx::Result<MonthSummary> {
    let income = month_sums(db, "income", budget, month).await?;
    let expense = month_sums(db, "expense", budget, month).await?;
    // Savings move money out of the month, except the starting balance of the savings and
    // interest paid on savings accounts, which were never part of any month's money.
    // A withdrawal brings money back into the month.
    let saved_rows = sqlx::query(
        "SELECT t.currency, COALESCE(SUM(CASE WHEN t.source = 'withdrawal' THEN -t.amount ELSE t.amount END), 0.0) FROM transactions t JOIN categories c ON c.id = t.category_id
         WHERE t.type = 'savings' AND COALESCE(t.note, '') NOT LIKE 'Первоначальная сумма%' AND (t.source IS NULL OR t.source = 'withdrawal')
           AND t.transaction_date >= $1 AND t.transaction_date < $2 AND c.budget_id = $3
         GROUP BY t.currency",
    )
    .bind(&month.0).bind(&month.1).bind(budget)
    .fetch_all(db)
    .await?;
    let saved: Totals = saved_rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    let carry_rows = sqlx::query(
        "SELECT t.currency, COALESCE(SUM(CASE
                WHEN t.type = 'income' THEN t.amount
                WHEN t.type = 'expense' THEN -t.amount
                WHEN t.type = 'savings' AND t.source = 'withdrawal' THEN t.amount
                WHEN t.type = 'savings' AND COALESCE(t.note, '') NOT LIKE 'Первоначальная сумма%' AND t.source IS NULL THEN -t.amount
                ELSE 0 END), 0.0)
         FROM transactions t JOIN categories c ON c.id = t.category_id
         WHERE t.type IN ('income', 'expense', 'savings') AND t.transaction_date < $1 AND c.budget_id = $2
         GROUP BY t.currency",
    )
    .bind(&month.0).bind(budget)
    .fetch_all(db)
    .await?;
    let mut carry_in = zero_filled();
    for r in carry_rows {
        carry_in.insert(r.get(0), r.get(1));
    }
    let balance = DEFAULT_CURRENCIES
        .iter()
        .map(|c| {
            let get = |m: &Totals| m.get(*c).copied().unwrap_or(0.0);
            (c.to_string(), get(&carry_in) + get(&income) - get(&expense) - get(&saved))
        })
        .collect();
    Ok(MonthSummary { income, expense, carry_in, balance })
}

#[derive(Debug, Serialize)]
pub struct SavingsSummary {
    pub category_id: i64,
    pub name: String,
    pub currency: String,
    pub target_amount: f64,
    pub saved_amount: f64,
    pub remaining_amount: f64,
    pub progress_percent: f64,
    /// The starting balance («Первоначальная сумма…»), whatever month it is dated in.
    pub initial: Option<SavingsInitial>,
}

#[derive(Debug, Serialize)]
pub struct SavingsInitial {
    pub id: i64,
    pub amount: f64,
    pub currency: String,
    pub transaction_date: String,
}

/// Savings saved up to `until` (exclusive, `YYYY-MM-DD`): the end of the month on screen.
pub async fn savings_summary(db: &PgPool, budget: i64, until: &str) -> sqlx::Result<Option<SavingsSummary>> {
    let id = ensure_system_root(db, budget, "savings").await?;
    let row = sqlx::query("SELECT id, name, emoji, color, type, target_amount FROM categories WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    let Some(cat) = row.as_ref().map(category_from_row) else { return Ok(None) };
    let mut ids = vec![cat.id];
    ids.extend(child_ids(db, cat.id).await?);
    let rows = sqlx::query(
        "SELECT currency, COALESCE(SUM(CASE WHEN source = 'withdrawal' THEN -amount ELSE amount END), 0.0) FROM transactions
         WHERE type = 'savings' AND category_id = ANY($1) AND transaction_date < $2
         GROUP BY currency",
    )
    .bind(&ids).bind(until)
    .fetch_all(db)
    .await?;
    let by_currency: Totals = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    let (currency, saved) = by_currency.iter().next().map(|(c, v)| (c.clone(), *v)).unwrap_or(("₽".to_string(), 0.0));
    let target = cat.target_amount.unwrap_or(0.0);
    let remaining = (target - saved).max(0.0);
    let progress = if target > 0.0 { ((saved / target * 100.0) * 10.0).round() / 10.0 } else { 0.0 };
    let initial = sqlx::query(
        "SELECT id, amount, currency, transaction_date FROM transactions
         WHERE type = 'savings' AND category_id = ANY($1) AND COALESCE(note, '') LIKE 'Первоначальная сумма%'
         ORDER BY transaction_date ASC, id ASC LIMIT 1",
    )
    .bind(&ids)
    .fetch_optional(db)
    .await?
    .map(|r| SavingsInitial { id: r.get(0), amount: r.get(1), currency: r.get(2), transaction_date: r.get(3) });
    Ok(Some(SavingsSummary {
        category_id: cat.id,
        name: cat.name,
        currency,
        target_amount: target,
        saved_amount: saved,
        remaining_amount: remaining,
        progress_percent: progress.min(100.0),
        initial,
    }))
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------
#[derive(Debug, Serialize)]
pub struct ReportCategory {
    pub id: i64,
    pub name: String,
    pub emoji: String,
    pub color: String,
    pub totals: Totals,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct ReportDay {
    pub date: String,
    pub expense: Totals,
}

#[derive(Debug, Serialize)]
pub struct ReportTransaction {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub amount: f64,
    pub currency: String,
    pub note: Option<String>,
    pub transaction_date: String,
    pub category_name: String,
    pub category_emoji: String,
    pub subcategory_name: Option<String>,
    pub actor_name: Option<String>,
    /// `withdrawal` or interest (`interest`, `interest_fixed`); left out for ordinary operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The other half of a transfer between savings accounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub from: String,
    pub to: String,
    pub expense: Totals,
    pub income: Totals,
    pub savings: Totals,
    /// Expense roots of the budget with operations in the period, largest first.
    pub categories: Vec<ReportCategory>,
    /// Expenses per day, only days that have any.
    pub days: Vec<ReportDay>,
    pub transactions: Vec<ReportTransaction>,
}

/// Operations of a budget dated `from..=to`. The starting balance of savings is left out,
/// as in the month summary: it is not money of the period.
pub async fn report(db: &PgPool, budget: i64, from: NaiveDate, to: NaiveDate) -> sqlx::Result<Report> {
    let end = (to + chrono::Days::new(1)).to_string();
    let rows = sqlx::query(
        "SELECT t.id, t.type, t.amount, t.currency, t.note, t.transaction_date,
                r.id, r.name, r.emoji, r.color, CASE WHEN c.parent_id IS NULL THEN NULL ELSE c.name END,
                COALESCE(u.first_name, u.username, u.telegram_id), t.source, t.pair_id
         FROM transactions t
         JOIN categories c ON c.id = t.category_id
         JOIN categories r ON r.id = COALESCE(c.parent_id, c.id)
         LEFT JOIN users u ON u.id = t.user_id
         WHERE c.budget_id = $1 AND t.transaction_date >= $2 AND t.transaction_date < $3
           AND NOT (t.type = 'savings' AND (COALESCE(t.note, '') LIKE 'Первоначальная сумма%' OR COALESCE(t.source, 'withdrawal') <> 'withdrawal'))
         ORDER BY t.transaction_date DESC, t.created_at DESC, t.id DESC",
    )
    .bind(budget).bind(from.to_string()).bind(&end)
    .fetch_all(db)
    .await?;

    let mut report = Report {
        from: from.to_string(),
        to: to.to_string(),
        expense: Totals::new(),
        income: Totals::new(),
        savings: Totals::new(),
        categories: Vec::new(),
        days: Vec::new(),
        transactions: Vec::with_capacity(rows.len()),
    };
    let mut categories: BTreeMap<i64, ReportCategory> = BTreeMap::new();
    let mut days: BTreeMap<String, Totals> = BTreeMap::new();
    for r in &rows {
        let t = ReportTransaction {
            id: r.get(0),
            kind: r.get(1),
            amount: r.get(2),
            currency: r.get(3),
            note: r.get(4),
            transaction_date: r.get(5),
            category_name: r.get(7),
            category_emoji: r.get(8),
            subcategory_name: r.get(10),
            actor_name: r.get(11),
            source: r.get(12),
            pair_id: r.get(13),
        };
        let totals = match t.kind.as_str() {
            "income" => &mut report.income,
            "savings" => &mut report.savings,
            _ => &mut report.expense,
        };
        let signed = if t.source.as_deref() == Some(WITHDRAWAL) { -t.amount } else { t.amount };
        *totals.entry(t.currency.clone()).or_default() += signed;
        if t.kind == "expense" {
            let cat = categories.entry(r.get(6)).or_insert_with(|| ReportCategory {
                id: r.get(6),
                name: t.category_name.clone(),
                emoji: t.category_emoji.clone(),
                color: r.get(9),
                totals: Totals::new(),
                count: 0,
            });
            *cat.totals.entry(t.currency.clone()).or_default() += t.amount;
            cat.count += 1;
            *days.entry(t.transaction_date.clone()).or_default().entry(t.currency.clone()).or_default() += t.amount;
        }
        report.transactions.push(t);
    }
    report.categories = categories.into_values().collect();
    report.categories.sort_by(|a, b| b.totals.values().sum::<f64>().total_cmp(&a.totals.values().sum::<f64>()));
    report.days = days.into_iter().map(|(date, expense)| ReportDay { date, expense }).collect();
    Ok(report)
}
