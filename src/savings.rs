//! Savings accounts with interest. An account is a subcategory of the savings category with
//! interest settings. Deposits are ordinary savings operations in it; the server adds the
//! interest as operations «Проценты» marked `source = 'interest'`.
//!
//! Interest is counted day by day at `rate / 365` of the balance and paid after tax:
//! - a flexible account (like a daily-interest account) pays on the last day of every month,
//!   and what it has earned but not yet paid earns interest too;
//! - a term deposit earns on its balance at the start of the term, at the rate of that day,
//!   and pays when the term ends; the next term starts with the interest included. Money
//!   added during a term starts earning in the next one; money taken out stops at once.
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

pub const INTEREST_NOTE: &str = "Проценты";
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

/// Settings of an account as the app sends them.
#[derive(Deserialize)]
pub struct AccountIn {
    pub name: Option<String>,
    pub kind: Kind,
    /// Annual rate, percent.
    pub rate: f64,
    /// Tax withheld from interest, percent.
    #[serde(default)]
    pub tax: f64,
    pub term_days: Option<i64>,
    pub currency: String,
    pub start: NaiveDate,
    /// The day a changed rate applies from; today if not given.
    pub rate_from: Option<NaiveDate>,
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

#[derive(Serialize)]
pub struct AccountOut {
    pub id: i64,
    pub name: String,
    pub kind: Kind,
    /// The rate and tax in force today.
    pub rate: f64,
    pub tax: f64,
    pub term_days: Option<i64>,
    pub currency: String,
    pub start: String,
    /// Start of the current term of a term deposit.
    pub term_start: String,
    /// Deposits less withdrawals, plus interest paid, up to today.
    pub balance: f64,
    /// Earned since the last payment, after tax.
    pub accrued: f64,
    pub interest_paid: f64,
    pub next_payout_date: String,
    /// Expected next payment after tax, if nothing is added.
    pub next_payout: f64,
    /// Expected interest over the next 365 days after tax, at today's rate.
    pub year_forecast: f64,
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
                let (rate, tax) = term_rate;
                accrued += principal.max(0.0) * rate / 36_500.0 * (1.0 - tax / 100.0);
                let maturity = term_start + Duration::days(plan.term_days);
                if day + Duration::days(1) >= maturity {
                    if !fixed.contains(&maturity) && round2(accrued) > 0.0 {
                        payouts.push((maturity, round2(accrued)));
                    }
                    accrued = 0.0;
                    term_start = maturity;
                }
            }
        }
        day += Duration::days(1);
    }
    Simulation { payouts, accrued, term_start }
}

struct Account {
    id: i64,
    name: String,
    currency: String,
    plan: Plan,
    deposits: Vec<(NaiveDate, f64)>,
    fixed: BTreeSet<NaiveDate>,
    fixed_total: f64,
    /// The server's own payments now in the database: (operation id, day, amount).
    paid: Vec<(i64, NaiveDate, f64)>,
}

/// Locks an account and reads it; `None` when the subcategory has no interest settings.
async fn load(tx: &mut Transaction<'_, Postgres>, id: i64) -> sqlx::Result<Option<Account>> {
    let Some(r) = sqlx::query(
        "SELECT id, name, acc_kind, acc_term_days, acc_currency, acc_start FROM categories
         WHERE id = $1 AND acc_kind IS NOT NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let currency: String = r.get::<Option<String>, _>(4).unwrap_or_default();
    let start = r.get::<Option<String>, _>(5).as_deref().and_then(parse_day).unwrap_or_else(today);
    let rates = sqlx::query("SELECT from_day, rate, tax FROM savings_rates WHERE category_id = $1 ORDER BY from_day")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .filter_map(|r| Some((parse_day(&r.get::<String, _>(0))?, r.get(1), r.get(2))))
        .collect();
    let rows = sqlx::query(
        "SELECT id, transaction_date, amount, source FROM transactions
         WHERE category_id = $1 AND type = 'savings' AND currency = $2 ORDER BY transaction_date, id",
    )
    .bind(id).bind(&currency)
    .fetch_all(&mut **tx)
    .await?;
    let mut acc = Account {
        id: r.get(0),
        name: r.get(1),
        currency,
        plan: Plan { kind: Kind::parse(&r.get::<String, _>(2)), term_days: r.get::<Option<i64>, _>(3).unwrap_or(30), start, rates },
        deposits: Vec::new(),
        fixed: BTreeSet::new(),
        fixed_total: 0.0,
        paid: Vec::new(),
    };
    for row in rows {
        let Some(day) = parse_day(&row.get::<String, _>(1)) else { continue };
        let amount: f64 = row.get(2);
        match row.get::<Option<String>, _>(3).as_deref() {
            Some(INTEREST_SOURCE) => acc.paid.push((row.get(0), day, amount)),
            Some(crate::db::WITHDRAWAL) => acc.deposits.push((day, -amount)),
            Some(FIXED_SOURCE) => {
                acc.fixed.insert(day);
                acc.fixed_total += amount;
                acc.deposits.push((day, amount));
            }
            _ => acc.deposits.push((day, amount)),
        }
    }
    Ok(Some(acc))
}

/// Brings the server's interest payments of an account in line with what is due up to today.
async fn sync(db: &PgPool, id: i64, budget: i64) -> sqlx::Result<Option<(Account, Simulation)>> {
    let mut tx = db.begin().await?;
    let Some(mut acc) = load(&mut tx, id).await? else { return Ok(None) };
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
    for &(day, amount) in &due {
        if kept.iter().any(|(_, d, _)| *d == day) {
            continue;
        }
        let op: i64 = sqlx::query_scalar(
            "INSERT INTO transactions (user_id, category_id, type, amount, currency, note, transaction_date, source)
             VALUES ($1, $2, 'savings', $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(budget).bind(acc.id).bind(amount).bind(&acc.currency).bind(INTEREST_NOTE).bind(day.to_string()).bind(INTEREST_SOURCE)
        .fetch_one(&mut *tx)
        .await?;
        kept.push((op, day, amount));
        changed += 1;
    }
    tx.commit().await?;
    if changed > 0 {
        tracing::info!(account = acc.id, changed, "interest payments updated");
    }
    acc.paid = kept;
    Ok(Some((acc, sim)))
}

fn describe(acc: &Account, sim: &Simulation) -> AccountOut {
    let today = today();
    let (rate, tax) = acc.plan.rate_on(today);
    // What comes next if nothing is added: the same calculation a year ahead.
    let ahead = simulate(&acc.plan, &acc.deposits, &acc.fixed, today + Duration::days(366));
    let future: Vec<_> = ahead.payouts.iter().filter(|(d, _)| *d > today).collect();
    let (next_date, next_amount) = future.first().map(|&&(d, a)| (d, a)).unwrap_or((today, 0.0));
    let year: f64 = future.iter().filter(|(d, _)| *d <= today + Duration::days(365)).map(|(_, a)| a).sum();
    let paid: f64 = acc.paid.iter().map(|(_, _, a)| a).sum::<f64>() + acc.fixed_total;
    let balance = acc.deposits.iter().filter(|(d, _)| *d <= today).map(|(_, a)| a).sum::<f64>()
        + acc.paid.iter().filter(|(_, d, _)| *d <= today).map(|(_, _, a)| a).sum::<f64>();
    AccountOut {
        id: acc.id,
        name: acc.name.clone(),
        kind: acc.plan.kind,
        rate,
        tax,
        term_days: (acc.plan.kind == Kind::Term).then_some(acc.plan.term_days),
        currency: acc.currency.clone(),
        start: acc.plan.start.to_string(),
        term_start: sim.term_start.to_string(),
        balance: round2(balance),
        accrued: round2(sim.accrued),
        interest_paid: round2(paid),
        next_payout_date: next_date.to_string(),
        next_payout: next_amount,
        year_forecast: round2(year + if future.is_empty() { ahead.accrued } else { 0.0 }),
    }
}

/// Accounts of a budget, their interest brought up to today.
pub async fn list(db: &PgPool, budget: i64) -> sqlx::Result<Vec<AccountOut>> {
    let mut out = Vec::new();
    for id in account_ids(db, Some(budget)).await? {
        if let Some((acc, sim)) = sync(db, id, budget).await? {
            out.push(describe(&acc, &sim));
        }
    }
    Ok(out)
}

pub async fn one(db: &PgPool, id: i64, budget: i64) -> sqlx::Result<Option<AccountOut>> {
    Ok(sync(db, id, budget).await?.map(|(acc, sim)| describe(&acc, &sim)))
}

/// Brings the interest of a budget's accounts up to date, e.g. before showing its month.
pub async fn sync_budget(db: &PgPool, budget: i64) -> sqlx::Result<()> {
    for id in account_ids(db, Some(budget)).await? {
        sync(db, id, budget).await?;
    }
    Ok(())
}

/// Every account of every budget; the background worker calls it.
pub async fn sync_all(db: &PgPool) -> sqlx::Result<()> {
    let rows: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, budget_id FROM categories WHERE acc_kind IS NOT NULL AND archived_from IS NULL AND type = 'savings' ORDER BY id",
    )
    .fetch_all(db)
    .await?;
    for (id, budget) in rows {
        sync(db, id, budget.unwrap_or(0)).await?;
    }
    Ok(())
}

async fn account_ids(db: &PgPool, budget: Option<i64>) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM categories WHERE acc_kind IS NOT NULL AND archived_from IS NULL AND type = 'savings'
           AND ($1::BIGINT IS NULL OR budget_id = $1) ORDER BY id",
    )
    .bind(budget)
    .fetch_all(db)
    .await
}

/// Gives a savings subcategory interest settings, or changes them. A changed rate or tax
/// applies from `rate_from` (today by default); everything else is worked out again.
pub async fn configure(db: &PgPool, id: i64, budget: i64, a: &AccountIn) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE categories SET acc_kind = $1, acc_term_days = $2, acc_currency = $3, acc_start = $4 WHERE id = $5")
        .bind(a.kind.as_str()).bind(a.term_days).bind(&a.currency).bind(a.start.to_string()).bind(id)
        .execute(&mut *tx)
        .await?;
    let start = a.start.to_string();
    // Rates that began before the start: only the one in force on the start day matters, and
    // it now begins on the start.
    let in_force: Option<(String, f64, f64)> = sqlx::query_as(
        "SELECT from_day, rate, tax FROM savings_rates WHERE category_id = $1 AND from_day <= $2 ORDER BY from_day DESC LIMIT 1",
    )
    .bind(id).bind(&start)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM savings_rates WHERE category_id = $1 AND from_day <= $2").bind(id).bind(&start).execute(&mut *tx).await?;
    if let Some((_, rate, tax)) = in_force {
        sqlx::query("INSERT INTO savings_rates (category_id, from_day, rate, tax) VALUES ($1, $2, $3, $4)")
            .bind(id).bind(&start).bind(rate).bind(tax)
            .execute(&mut *tx)
            .await?;
    }
    let rates: Vec<(String, f64, f64)> = sqlx::query_as("SELECT from_day, rate, tax FROM savings_rates WHERE category_id = $1 ORDER BY from_day")
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
    let today = today().to_string();
    let current = rates.iter().rev().find(|(from, _, _)| *from <= today).or(rates.first());
    if current.is_none_or(|&(_, r, t)| (r - a.rate).abs() > 1e-9 || (t - a.tax).abs() > 1e-9) {
        let from = if rates.is_empty() { start.clone() } else { a.rate_from.unwrap_or_else(self::today).max(a.start).to_string() };
        // A rate from a day replaces what was set for that day and after.
        sqlx::query("DELETE FROM savings_rates WHERE category_id = $1 AND from_day >= $2").bind(id).bind(&from).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO savings_rates (category_id, from_day, rate, tax) VALUES ($1, $2, $3, $4)")
            .bind(id).bind(&from).bind(a.rate).bind(a.tax)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    sync(db, id, budget).await?;
    Ok(())
}

/// Stops interest on an account: what was paid stays, as payments nothing recalculates.
pub async fn stop(db: &PgPool, id: i64, budget: i64) -> sqlx::Result<()> {
    sync(db, id, budget).await?;
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE categories SET acc_kind = NULL, acc_term_days = NULL, acc_currency = NULL, acc_start = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM savings_rates WHERE category_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE transactions SET source = $2 WHERE category_id = $1 AND source = $3")
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
        Plan { kind, term_days, start: d(start), rates: vec![(d(start), rate, tax)] }
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
