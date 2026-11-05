//! Interest on savings accounts (src/accounts.rs, kind 'savings' with `interest_kind` set). The
//! account's money is its starting balance and everything booked on it: transfers in and out,
//! income and expenses. Interest is paid as income «Проценты» (in the budget's language) on
//! the account, marked `source = 'interest'`; it is income of the month like any other.
//!
//! Interest is counted day by day at `rate / 365` of the balance and paid after tax:
//! - a flexible account (like a daily-interest account) pays on the last day of every month,
//!   and what it has earned but not yet paid earns interest too;
//! - a term deposit earns on its balance at the start of the term, at the rate of that day,
//!   and pays when the term ends; the next term starts with the interest included. Money
//!   added during a term starts earning in the next one; money taken out stops at once.
//!   As banks do, the tax is worked out once on the term's interest, rounded to kuruş.
//!   A deposit that doesn't renew ends with its term: the money and interest go back to the
//!   payout account by a transfer (made here), and a new term is opened by hand (`new_term`).
//!
//! A withdrawal (`source = 'withdrawal'`) lowers the balance from its day.
//!
//! The interest is always worked out again from the account's start, so deposits entered
//! later for past days count. Rates are kept with the day they apply from, so a new rate
//! doesn't change past months. A payment the user corrected to match the bank becomes
//! `source = 'interest_fixed'` and is never recalculated.
//! Like the starting balance, interest never reduces a month's balance: it was not taken
//! from any month's money.

use std::collections::BTreeSet;

use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub const INTEREST_SOURCE: &str = "interest";
pub const FIXED_SOURCE: &str = "interest_fixed";
/// Accounts can start at most this many days back.
const MAX_DAYS: i64 = 3700;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Flexible,
    Term,
}

impl Kind {
    fn parse(s: &str) -> Self {
        if s == "term" { Kind::Term } else { Kind::Flexible }
    }
    fn as_str(self) -> &'static str {
        match self {
            Kind::Flexible => "flexible",
            Kind::Term => "term",
        }
    }
}

/// Interest settings of a savings account as the app sends them.
#[derive(Deserialize, Clone)]
pub struct AccountIn {
    pub kind: Kind,
    /// Annual rate, percent.
    pub rate: f64,
    /// Tax withheld from interest, percent.
    #[serde(default)]
    pub tax: f64,
    pub term_days: Option<i64>,
    pub start: NaiveDate,
    /// The day a changed rate applies from; today if not given.
    pub rate_from: Option<NaiveDate>,
    /// A term deposit renews with its interest; otherwise it ends and the money goes back to
    /// `payout_account_id` (an account of the same currency), or stays when there is none.
    #[serde(default = "yes")]
    pub renew: bool,
    pub payout_account_id: Option<i64>,
}

fn yes() -> bool {
    true
}

impl AccountIn {
    pub fn check(&self) -> Result<(), &'static str> {
        if !self.rate.is_finite() || !(0.0..=1000.0).contains(&self.rate) {
            return Err("Ставка должна быть от 0 до 1000%");
        }
        if !self.tax.is_finite() || !(0.0..100.0).contains(&self.tax) {
            return Err("Налог должен быть от 0 до 100%");
        }
        if self.kind == Kind::Term && !matches!(self.term_days, Some(1..=3660)) {
            return Err("Срок вклада — от 1 до 3660 дней");
        }
        let today = today();
        if self.start > today || self.rate_from.is_some_and(|d| d > today) {
            return Err("Дата не может быть в будущем");
        }
        if (today - self.start).num_days() > MAX_DAYS {
            return Err("Дата начала — не раньше чем 10 лет назад");
        }
        Ok(())
    }
}

/// How a savings account earns, for the app.
#[derive(Serialize, Clone)]
pub struct InterestOut {
    pub kind: Kind,
    /// The rate and tax in force today.
    pub rate: f64,
    pub tax: f64,
    pub term_days: Option<i64>,
    pub start: String,
    /// Start of the current term of a term deposit.
    pub term_start: String,
    /// Earned since the last payment, after tax.
    pub accrued: f64,
    pub interest_paid: f64,
    pub next_payout_date: String,
    /// Expected next payment after tax, if nothing is added.
    pub next_payout: f64,
    /// Expected interest over the next 365 days after tax, at today's rate.
    pub year_forecast: f64,
    /// The next payment before tax and the tax taken from it, as the bank's statement shows them.
    pub next_gross: f64,
    pub next_tax: f64,
    pub renew: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payout_account_id: Option<i64>,
    /// The day a deposit that doesn't renew ended (paid out and returned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn is_month_end(d: NaiveDate) -> bool {
    d.succ_opt().is_some_and(|n| n.month() != d.month())
}

/// How an account earns.
#[derive(Clone)]
struct Plan {
    kind: Kind,
    term_days: i64,
    start: NaiveDate,
    /// A term deposit starts a new term at maturity; else it ends there.
    renew: bool,
    /// (from day, rate %, tax %), by day.
    rates: Vec<(NaiveDate, f64, f64)>,
}

impl Plan {
    fn rate_on(&self, d: NaiveDate) -> (f64, f64) {
        self.rates.iter().rev().find(|(from, _, _)| *from <= d).or(self.rates.first()).map(|&(_, r, t)| (r, t)).unwrap_or((0.0, 0.0))
    }
}

struct Simulation {
    /// Payments the server makes (corrected ones are among the deposits instead).
    payouts: Vec<(NaiveDate, f64)>,
    /// Earned since the last payment, after tax.
    accrued: f64,
    term_start: NaiveDate,
    /// The maturity of a deposit that doesn't renew, once reached.
    ended: Option<NaiveDate>,
}

/// Interest after tax, as a bank pays it: the tax is taken once from the rounded interest.
fn after_tax(gross: f64, tax: f64) -> f64 {
    let gross = round2(gross);
    round2(gross - round2(gross * tax / 100.0))
}

/// Works an account out from its start until `until` (exclusive). `deposits` are by date and
/// include payments the user corrected; on their days no payment is generated.
fn simulate(plan: &Plan, deposits: &[(NaiveDate, f64)], fixed: &BTreeSet<NaiveDate>, until: NaiveDate) -> Simulation {
    let mut payouts: Vec<(NaiveDate, f64)> = Vec::new();
    let balance_through = |payouts: &[(NaiveDate, f64)], d: NaiveDate| {
        deposits.iter().chain(payouts).filter(|(day, _)| *day <= d).map(|(_, a)| a).sum::<f64>()
    };
    let mut accrued = 0.0;
    let mut term_start = plan.start;
    let mut principal = 0.0;
    let mut term_rate = (0.0, 0.0);
    let mut ended = None;
    let mut day = plan.start;
    let end = until.min(plan.start + Duration::days(MAX_DAYS + 400));
    while day < end {
        match plan.kind {
            Kind::Flexible => {
                let (rate, tax) = plan.rate_on(day);
                let base = balance_through(&payouts, day) + accrued;
                accrued += base.max(0.0) * rate / 36_500.0 * (1.0 - tax / 100.0);
                if is_month_end(day) {
                    if !fixed.contains(&day) && round2(accrued) > 0.0 {
                        payouts.push((day, round2(accrued)));
                    }
                    accrued = 0.0;
                }
            }
            Kind::Term => {
                if day == term_start {
                    principal = balance_through(&payouts, day);
                    term_rate = plan.rate_on(day);
                } else {
                    // Money added waits for the next term, but money taken out stops earning now.
                    principal = principal.min(balance_through(&payouts, day));
                }
                // Before tax here; the tax is taken from the whole term's interest at maturity.
                let (rate, _) = term_rate;
                accrued += principal.max(0.0) * rate / 36_500.0;
                let maturity = term_start + Duration::days(plan.term_days);
                if day + Duration::days(1) >= maturity {
                    let paid = after_tax(accrued, term_rate.1);
                    if !fixed.contains(&maturity) && paid > 0.0 {
                        payouts.push((maturity, paid));
                    }
                    accrued = 0.0;
                    if !plan.renew {
                        ended = Some(maturity);
                        break;
                    }
                    term_start = maturity;
                }
            }
        }
        day += Duration::days(1);
    }
    // Shown as earned so far: after tax, like the payments.
    if plan.kind == Kind::Term {
        accrued *= 1.0 - term_rate.1 / 100.0;
    }
    Simulation { payouts, accrued, term_start, ended }
}

struct Account {
    id: i64,
    budget: i64,
    currency: String,
    payout_account: Option<i64>,
    plan: Plan,
    deposits: Vec<(NaiveDate, f64)>,
    fixed: BTreeSet<NaiveDate>,
    fixed_total: f64,
    /// The server's own payments now in the database: (operation id, day, amount).
    paid: Vec<(i64, NaiveDate, f64)>,
}

/// The income subcategory interest is booked on, made the first time it is needed.
pub async fn interest_category(tx: &mut Transaction<'_, Postgres>, budget: i64) -> sqlx::Result<i64> {
    let n = crate::db::names(crate::db::budget_lang(&mut **tx, budget).await?);
    let root: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE budget_id = $1 AND type = 'income' AND parent_id IS NULL ORDER BY id LIMIT 1")
        .bind(budget)
        .fetch_optional(&mut **tx)
        .await?;
    let root = match root {
        Some(id) => id,
        None => {
            sqlx::query_scalar("INSERT INTO categories (name, emoji, color, type, budget_id) VALUES ($2, '💰', '#9ece6a', 'income', $1) RETURNING id")
                .bind(budget).bind(n.income)
                .fetch_one(&mut **tx)
                .await?
        }
    };
    // Its name in any language: the budget's may have been named before languages were kept.
    let sub: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE parent_id = $1 AND name = ANY($2) ORDER BY id LIMIT 1")
        .bind(root).bind(&crate::db::INTEREST_NAMES[..])
        .fetch_optional(&mut **tx)
        .await?;
    match sub {
        Some(id) => Ok(id),
        None => {
            sqlx::query_scalar(
                "INSERT INTO categories (name, emoji, color, type, parent_id, budget_id, visible_from)
                 VALUES ($1, '•', '#545c7e', 'income', $2, $3, '2000-01-01') RETURNING id",
            )
            .bind(n.interest).bind(root).bind(budget)
            .fetch_one(&mut **tx)
            .await
        }
    }
}

/// Locks an account and reads it; `None` when it doesn't earn interest.
async fn load(tx: &mut Transaction<'_, Postgres>, id: i64) -> sqlx::Result<Option<Account>> {
    let Some(r) = sqlx::query(
        "SELECT id, budget_id, interest_kind, term_days, currency, interest_start, initial_amount, initial_date, term_renew, payout_account_id
         FROM accounts WHERE id = $1 AND interest_kind IS NOT NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let currency: String = r.get(4);
    let start = r.get::<Option<String>, _>(5).as_deref().and_then(parse_day).unwrap_or_else(today);
    let rates = sqlx::query("SELECT from_day, rate, tax FROM account_rates WHERE account_id = $1 ORDER BY from_day")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .filter_map(|r| Some((parse_day(&r.get::<String, _>(0))?, r.get(1), r.get(2))))
        .collect();
    let initial: f64 = r.get(6);
    let from = r.get::<Option<String>, _>(7).as_deref().and_then(parse_day);
    let rows = sqlx::query(
        "SELECT id, transaction_date, amount, type, source FROM transactions
         WHERE account_id = $1 AND currency = $2 ORDER BY transaction_date, id",
    )
    .bind(id).bind(&currency)
    .fetch_all(&mut **tx)
    .await?;
    let mut acc = Account {
        id: r.get(0),
        budget: r.get(1),
        currency,
        payout_account: r.get(9),
        plan: Plan { kind: Kind::parse(&r.get::<String, _>(2)), term_days: r.get::<Option<i64>, _>(3).unwrap_or(30), start, renew: r.get(8), rates },
        deposits: Vec::new(),
        fixed: BTreeSet::new(),
        fixed_total: 0.0,
        paid: Vec::new(),
    };
    // The starting balance is there on its day, or from the start of interest when it has none.
    if initial != 0.0 {
        acc.deposits.push((from.unwrap_or(start), initial));
    }
    for row in rows {
        let Some(day) = parse_day(&row.get::<String, _>(1)) else { continue };
        // Before the starting balance's day: already in it.
        if from.is_some_and(|f| day < f) {
            continue;
        }
        let amount: f64 = row.get(2);
        let kind: String = row.get(3);
        match (kind.as_str(), row.get::<Option<String>, _>(4).as_deref()) {
            ("income", Some(INTEREST_SOURCE)) => acc.paid.push((row.get(0), day, amount)),
            ("income", Some(FIXED_SOURCE)) => {
                acc.fixed.insert(day);
                acc.fixed_total += amount;
                acc.deposits.push((day, amount));
            }
            ("income", _) | ("transfer", Some(crate::accounts::IN)) => acc.deposits.push((day, amount)),
            ("expense", _) | ("transfer", _) => acc.deposits.push((day, -amount)),
            _ => {}
        }
    }
    acc.deposits.sort_by_key(|(d, _)| *d);
    Ok(Some(acc))
}

/// Brings the server's interest payments of an account in line with what is due up to today.
async fn sync(db: &PgPool, id: i64) -> sqlx::Result<Option<(Account, Simulation)>> {
    let mut tx = db.begin().await?;
    let Some(mut acc) = load(&mut tx, id).await? else { return Ok(None) };
    // Notes of the payments it adds, in the budget's language.
    let names = crate::db::names(crate::db::budget_lang(&mut *tx, acc.budget).await?);
    // Days up to yesterday are complete; a payment falls due once its last day is over.
    let sim = simulate(&acc.plan, &acc.deposits, &acc.fixed, today());
    let due = sim.payouts.clone();
    let mut kept = Vec::new();
    let mut changed = 0;
    for &(op, day, amount) in &acc.paid {
        match due.iter().find(|(d, _)| *d == day) {
            Some(_) if kept.iter().any(|(_, d, _)| *d == day) => {
                // A second payment for the same day (two syncs raced before the lock): drop it.
                sqlx::query("DELETE FROM transactions WHERE id = $1").bind(op).execute(&mut *tx).await?;
                changed += 1;
            }
            Some(&(_, want)) if (want - amount).abs() < 0.005 => kept.push((op, day, amount)),
            Some(&(_, want)) => {
                sqlx::query("UPDATE transactions SET amount = $1 WHERE id = $2").bind(want).bind(op).execute(&mut *tx).await?;
                kept.push((op, day, want));
                changed += 1;
            }
            None => {
                sqlx::query("DELETE FROM transactions WHERE id = $1").bind(op).execute(&mut *tx).await?;
                changed += 1;
            }
        }
    }
    let mut category = None;
    for &(day, amount) in &due {
        if kept.iter().any(|(_, d, _)| *d == day) {
            continue;
        }
        let cat = match category {
            Some(c) => c,
            None => {
                let c = interest_category(&mut tx, acc.budget).await?;
                category = Some(c);
                c
            }
        };
        let op: i64 = sqlx::query_scalar(
            "INSERT INTO transactions (user_id, category_id, account_id, type, amount, currency, note, transaction_date, source)
             VALUES ($1, $2, $3, 'income', $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(acc.budget).bind(cat).bind(acc.id).bind(amount).bind(&acc.currency).bind(names.interest).bind(day.to_string()).bind(INTEREST_SOURCE)
        .fetch_one(&mut *tx)
        .await?;
        kept.push((op, day, amount));
        changed += 1;
    }
    // A deposit that doesn't renew has ended: its money and interest go back, once.
    if let (Some(end), Some(to)) = (sim.ended, acc.payout_account) {
        let fits: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND budget_id = $2 AND currency = $3 AND id <> $4)")
            .bind(to).bind(acc.budget).bind(&acc.currency).bind(acc.id)
            .fetch_one(&mut *tx)
            .await?;
        let balance = acc.deposits.iter().filter(|(d, _)| *d <= end).map(|(_, a)| a).sum::<f64>()
            + kept.iter().filter(|(_, d, _)| *d <= end).map(|(_, _, a)| a).sum::<f64>();
        let balance = round2(balance);
        if fits && balance > 0.0 {
            let key = format!("deposit-return:{}:{end}", acc.id);
            let out: Option<i64> = sqlx::query_scalar(
                "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, import_key)
                 VALUES ($1, $2, 'transfer', $3, $4, $5, $6, $7, $8) ON CONFLICT (import_key) WHERE import_key IS NOT NULL DO NOTHING RETURNING id",
            )
            .bind(acc.budget).bind(acc.id).bind(balance).bind(&acc.currency).bind(names.deposit_return).bind(end.to_string()).bind(crate::accounts::OUT).bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(out) = out {
                let into: i64 = sqlx::query_scalar(
                    "INSERT INTO transactions (user_id, account_id, type, amount, currency, note, transaction_date, source, pair_id)
                     VALUES ($1, $2, 'transfer', $3, $4, $5, $6, $7, $8) RETURNING id",
                )
                .bind(acc.budget).bind(to).bind(balance).bind(&acc.currency).bind(names.deposit_return).bind(end.to_string()).bind(crate::accounts::IN).bind(out)
                .fetch_one(&mut *tx)
                .await?;
                sqlx::query("UPDATE transactions SET pair_id = $2 WHERE id = $1").bind(out).bind(into).execute(&mut *tx).await?;
                acc.deposits.push((end, -balance));
                changed += 1;
            }
        }
    }
    tx.commit().await?;
    if changed > 0 {
        tracing::info!(account = acc.id, changed, "interest payments updated");
    }
    acc.paid = kept;
    Ok(Some((acc, sim)))
}

fn describe(acc: &Account, sim: &Simulation) -> InterestOut {
    let today = today();
    let (rate, tax) = acc.plan.rate_on(today);
    // What comes next if nothing is added: the same calculation a year ahead.
    let ahead = simulate(&acc.plan, &acc.deposits, &acc.fixed, today + Duration::days(366));
    let future: Vec<_> = ahead.payouts.iter().filter(|(d, _)| *d > today).collect();
    let (next_date, next_amount) = future.first().map(|&&(d, a)| (d, a)).unwrap_or((today, 0.0));
    let year: f64 = future.iter().filter(|(d, _)| *d <= today + Duration::days(365)).map(|(_, a)| a).sum();
    let paid: f64 = acc.paid.iter().map(|(_, _, a)| a).sum::<f64>() + acc.fixed_total;
    let (_, next_tax_rate) = acc.plan.rate_on(next_date);
    let next_gross = if next_tax_rate < 100.0 { round2(next_amount / (1.0 - next_tax_rate / 100.0)) } else { next_amount };
    InterestOut {
        kind: acc.plan.kind,
        rate,
        tax,
        term_days: (acc.plan.kind == Kind::Term).then_some(acc.plan.term_days),
        start: acc.plan.start.to_string(),
        term_start: sim.term_start.to_string(),
        accrued: round2(sim.accrued),
        interest_paid: round2(paid),
        next_payout_date: next_date.to_string(),
        next_payout: next_amount,
        year_forecast: round2(year + if future.is_empty() { ahead.accrued } else { 0.0 }),
        next_gross,
        next_tax: round2(next_gross - next_amount),
        renew: acc.plan.renew,
        payout_account_id: acc.payout_account,
        ended: sim.ended.map(|d| d.to_string()),
    }
}

/// Interest of every earning account of a budget, brought up to today, by account id.
pub async fn summaries(db: &PgPool, budget: i64) -> sqlx::Result<std::collections::HashMap<i64, InterestOut>> {
    let mut out = std::collections::HashMap::new();
    for id in account_ids(db, Some(budget)).await? {
        if let Some((acc, sim)) = sync(db, id).await? {
            out.insert(id, describe(&acc, &sim));
        }
    }
    Ok(out)
}

/// Every earning account of every budget; the background worker calls it.
pub async fn sync_all(db: &PgPool) -> sqlx::Result<()> {
    for id in account_ids(db, None).await? {
        sync(db, id).await?;
    }
    Ok(())
}

async fn account_ids(db: &PgPool, budget: Option<i64>) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM accounts WHERE interest_kind IS NOT NULL AND NOT archived AND kind = 'savings'
           AND ($1::BIGINT IS NULL OR budget_id = $1) ORDER BY id",
    )
    .bind(budget)
    .fetch_all(db)
    .await
}

/// Gives a savings account interest, or changes it. A changed rate or tax applies from
/// `rate_from` (today by default); everything else is worked out again.
pub async fn configure(db: &PgPool, id: i64, a: &AccountIn) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let start = a.start.to_string();
    // Interest paid before a later start was real (an earlier term): it stays as it is.
    sqlx::query(
        "UPDATE transactions t SET source = $3 FROM accounts a
         WHERE a.id = $1 AND t.account_id = a.id AND t.source = $4 AND a.interest_start < $2 AND t.transaction_date <= $2",
    )
    .bind(id).bind(&start).bind(FIXED_SOURCE).bind(INTEREST_SOURCE)
    .execute(&mut *tx)
    .await?;
    // The payout account: another account of the budget in the same currency, or none.
    sqlx::query(
        "UPDATE accounts a SET interest_kind = $1, term_days = $2, interest_start = $3, term_renew = $5,
            payout_account_id = (SELECT p.id FROM accounts p WHERE p.id = $6 AND p.budget_id = a.budget_id AND p.currency = a.currency AND p.id <> a.id)
         WHERE a.id = $4",
    )
    .bind(a.kind.as_str()).bind(a.term_days).bind(&start).bind(id).bind(a.kind != Kind::Term || a.renew).bind(a.payout_account_id)
    .execute(&mut *tx)
    .await?;
    // Rates that began before the start: only the one in force on the start day matters, and
    // it now begins on the start.
    let in_force: Option<(String, f64, f64)> = sqlx::query_as(
        "SELECT from_day, rate, tax FROM account_rates WHERE account_id = $1 AND from_day <= $2 ORDER BY from_day DESC LIMIT 1",
    )
    .bind(id).bind(&start)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM account_rates WHERE account_id = $1 AND from_day <= $2").bind(id).bind(&start).execute(&mut *tx).await?;
    if let Some((_, rate, tax)) = in_force {
        sqlx::query("INSERT INTO account_rates (account_id, from_day, rate, tax) VALUES ($1, $2, $3, $4)")
            .bind(id).bind(&start).bind(rate).bind(tax)
            .execute(&mut *tx)
            .await?;
    }
    let rates: Vec<(String, f64, f64)> = sqlx::query_as("SELECT from_day, rate, tax FROM account_rates WHERE account_id = $1 ORDER BY from_day")
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    let today = today().to_string();
    let current = rates.iter().rev().find(|(from, _, _)| *from <= today).or(rates.first());
    if current.is_none_or(|&(_, r, t)| (r - a.rate).abs() > 1e-9 || (t - a.tax).abs() > 1e-9) {
        let from = if rates.is_empty() { start.clone() } else { a.rate_from.unwrap_or_else(self::today).max(a.start).to_string() };
        // A rate from a day replaces what was set for that day and after.
        sqlx::query("DELETE FROM account_rates WHERE account_id = $1 AND from_day >= $2").bind(id).bind(&from).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO account_rates (account_id, from_day, rate, tax) VALUES ($1, $2, $3, $4)")
            .bind(id).bind(&from).bind(a.rate).bind(a.tax)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    sync(db, id).await?;
    Ok(())
}

/// A new term of a deposit that ended (or one cut short): it starts on `a.start` with its own
/// days, rate and tax; what was paid before stays. The earlier term is settled first, so its
/// money is back on the payout account before the new one counts what is on the account.
pub async fn new_term(db: &PgPool, id: i64, a: &AccountIn) -> sqlx::Result<()> {
    sync(db, id).await?;
    configure(db, id, &AccountIn { rate_from: Some(a.start), ..a.clone() }).await
}

/// Stops interest on an account: what was paid stays, as payments nothing recalculates.
pub async fn stop(db: &PgPool, id: i64) -> sqlx::Result<()> {
    sync(db, id).await?;
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE accounts SET interest_kind = NULL, term_days = NULL, interest_start = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM account_rates WHERE account_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE transactions SET source = $2 WHERE account_id = $1 AND source = $3")
        .bind(id).bind(FIXED_SOURCE).bind(INTEREST_SOURCE)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn plan(kind: Kind, rate: f64, tax: f64, term_days: i64, start: &str) -> Plan {
        Plan { kind, term_days, start: d(start), renew: true, rates: vec![(d(start), rate, tax)] }
    }

    fn none() -> BTreeSet<NaiveDate> {
        BTreeSet::new()
    }

    #[test]
    fn flexible_pays_month_end() {
        // 100 000 at 36.5% a year: 100 a day; 31 days, plus about 46 earned on what was earned.
        let p = plan(Kind::Flexible, 36.5, 0.0, 0, "2026-01-01");
        let sim = simulate(&p, &[(d("2026-01-01"), 100_000.0)], &none(), d("2026-02-01"));
        assert_eq!(sim.payouts.len(), 1);
        assert_eq!(sim.payouts[0].0, d("2026-01-31"));
        assert!(sim.payouts[0].1 > 3_140.0 && sim.payouts[0].1 < 3_150.0, "{:?}", sim.payouts);
    }

    #[test]
    fn tax_is_taken_from_interest() {
        let p = plan(Kind::Flexible, 36.5, 17.5, 0, "2026-01-01");
        let paid = simulate(&p, &[(d("2026-01-01"), 100_000.0)], &none(), d("2026-02-01")).payouts[0].1;
        assert!(paid > 3_100.0 * 0.825 && paid < 3_146.0 * 0.825, "{paid}");
    }

    #[test]
    fn money_added_later_earns_from_its_day() {
        let p = plan(Kind::Flexible, 36.5, 0.0, 0, "2026-01-01");
        let paid = simulate(&p, &[(d("2026-01-21"), 100_000.0)], &none(), d("2026-02-01")).payouts[0].1;
        assert!(paid > 1_100.0 && paid < 1_110.0, "{paid}");
    }

    #[test]
    fn a_new_rate_applies_from_its_day() {
        let mut p = plan(Kind::Flexible, 36.5, 0.0, 0, "2026-01-01");
        p.rates.push((d("2026-02-01"), 0.0, 0.0));
        let sim = simulate(&p, &[(d("2026-01-01"), 100_000.0)], &none(), d("2026-03-01"));
        // January earns, February doesn't.
        assert_eq!(sim.payouts.len(), 1);
        assert_eq!(sim.payouts[0].0, d("2026-01-31"));
    }

    #[test]
    fn term_pays_at_maturity_and_renews() {
        // 32 days at 45.625%: 100 000 × 0.45625 × 32 / 365 = 4 000 before tax.
        let p = plan(Kind::Term, 45.625, 15.0, 32, "2026-03-01");
        let sim = simulate(&p, &[(d("2026-03-01"), 100_000.0)], &none(), d("2026-05-05"));
        assert_eq!(sim.payouts[0], (d("2026-04-02"), 3_400.0));
        assert_eq!(sim.payouts[1].0, d("2026-05-04"));
        assert!((sim.payouts[1].1 - 103_400.0 * 0.45625 * 32.0 / 365.0 * 0.85).abs() < 0.02, "{:?}", sim.payouts);
        assert_eq!(sim.term_start, d("2026-05-04"));
    }

    #[test]
    fn matches_a_bank_statement() {
        // A Turkish bank's dekont, amounts changed: 100 000 ₺ for 32 days at 34.25%: interest
        // 3 002.74, stopaj 17.5% 525.48, paid 2 477.26.
        let mut p = plan(Kind::Term, 34.25, 17.5, 32, "2026-10-01");
        p.renew = false;
        let sim = simulate(&p, &[(d("2026-10-01"), 100_000.0)], &none(), d("2027-01-01"));
        assert_eq!(sim.payouts, vec![(d("2026-11-02"), 2_477.26)]);
        assert_eq!(sim.ended, Some(d("2026-11-02")));
        // 150 000 ₺ for 31 days: interest 4 363.36, stopaj 763.59, «Net Faiz Tutarı» 3 599.77.
        let mut p = plan(Kind::Term, 34.25, 17.5, 31, "2026-10-30");
        p.renew = false;
        let sim = simulate(&p, &[(d("2026-10-30"), 150_000.0)], &none(), d("2027-01-01"));
        assert_eq!(sim.payouts, vec![(d("2026-11-30"), 3_599.77)]);
    }

    #[test]
    fn a_deposit_that_does_not_renew_earns_nothing_after_its_term() {
        let mut p = plan(Kind::Term, 36.5, 0.0, 10, "2026-03-01");
        p.renew = false;
        let sim = simulate(&p, &[(d("2026-03-01"), 100_000.0)], &none(), d("2026-06-01"));
        assert_eq!(sim.payouts, vec![(d("2026-03-11"), 1_000.0)]);
        assert_eq!(sim.accrued, 0.0);
    }

    #[test]
    fn tax_is_rounded_once_like_a_bank() {
        assert_eq!(after_tax(6_485.92, 17.5), 5_350.88);
        assert_eq!(after_tax(6_981.369_863, 17.5), 5_759.63);
    }

    #[test]
    fn term_deposit_added_mid_term_waits_for_next_term() {
        let p = plan(Kind::Term, 36.5, 0.0, 10, "2026-03-01");
        let sim = simulate(&p, &[(d("2026-03-01"), 10_000.0), (d("2026-03-05"), 90_000.0)], &none(), d("2026-03-12"));
        assert_eq!(sim.payouts, vec![(d("2026-03-11"), 100.0)]);
    }

    #[test]
    fn a_withdrawal_earns_no_more_from_its_day() {
        // 100 000 for 15 days, then 50 000 for 16: 1 500 + 800, plus about 40 earned on what was earned.
        let p = plan(Kind::Flexible, 36.5, 0.0, 0, "2026-01-01");
        let deposits = [(d("2026-01-01"), 100_000.0), (d("2026-01-16"), -50_000.0)];
        let paid = simulate(&p, &deposits, &none(), d("2026-02-01")).payouts[0].1;
        assert!(paid > 2_330.0 && paid < 2_350.0, "{paid}");
    }

    #[test]
    fn a_withdrawal_from_a_term_deposit_counts_at_once() {
        // 100 000 for 10 days at 36.5%: 5 days on 100 000, 5 on 40 000 → 500 + 200.
        let p = plan(Kind::Term, 36.5, 0.0, 10, "2026-03-01");
        let deposits = [(d("2026-03-01"), 100_000.0), (d("2026-03-06"), -60_000.0)];
        let sim = simulate(&p, &deposits, &none(), d("2026-03-12"));
        assert_eq!(sim.payouts, vec![(d("2026-03-11"), 700.0)]);
    }

    #[test]
    fn a_corrected_payment_is_kept_and_counts() {
        let p = plan(Kind::Flexible, 36.5, 0.0, 0, "2026-01-01");
        // The bank paid 3 000 for January; the user entered that instead of ours.
        let deposits = [(d("2026-01-01"), 100_000.0), (d("2026-01-31"), 3_000.0)];
        let fixed = BTreeSet::from([d("2026-01-31")]);
        let sim = simulate(&p, &deposits, &fixed, d("2026-03-01"));
        assert_eq!(sim.payouts.len(), 1, "January is not paid again: {:?}", sim.payouts);
        // February earns on 103 000: 28 days of about 103.
        assert!(sim.payouts[0].1 > 103_000.0 * 0.001 * 28.0, "{:?}", sim.payouts);
    }
}
