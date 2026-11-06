//! The forecast for the end of the current month, by currency: the everyday money now, plus
//! what recurring operations will still add or take this month, minus the usual daily spending
//! for the days left. The usual spending is that of the last 90 days without recurring
//! operations and without single big ones (rent paid by hand, a new laptop), which don't
//! happen every day.

use std::collections::{BTreeMap, HashMap};

use chrono::{Days, NaiveDate};
use serde::Serialize;
use sqlx::PgPool;

use crate::db::Totals;

/// How far back the usual spending is looked for.
const WINDOW_DAYS: u64 = 90;
/// Less history than this, and there is no usual spending to speak of.
const MIN_DAYS: i64 = 7;

#[derive(Debug, Serialize, Default, PartialEq)]
pub struct Line {
    /// Everyday money with what is already booked this month.
    pub now: f64,
    /// Recurring operations still to come this month: income in, expenses and transfers out.
    pub scheduled: f64,
    /// Usual spending a day.
    pub per_day: f64,
    pub end: f64,
}

#[derive(Debug, Serialize)]
pub struct Forecast {
    /// Days after today until the month ends.
    pub days_left: i64,
    /// Too little history: the usual spending is left out.
    pub short_history: bool,
    pub by_currency: BTreeMap<String, Line>,
}

/// One expense of the window: its root category, currency, amount, and whether it is on an
/// everyday account (or none), so it moves the balance.
pub struct Spent {
    pub root: i64,
    pub currency: String,
    pub amount: f64,
    pub everyday: bool,
}

/// Expenses that are not usual: more than a tenth of the window's spending in their currency
/// and more than five times its median expense. Both, so a few small expenses all stay in.
fn usual(spent: &[Spent]) -> Vec<&Spent> {
    let mut by_cur: HashMap<&str, Vec<f64>> = HashMap::new();
    for s in spent {
        by_cur.entry(&s.currency).or_default().push(s.amount);
    }
    let limits: HashMap<&str, (f64, f64)> = by_cur
        .into_iter()
        .map(|(c, mut v)| {
            v.sort_by(f64::total_cmp);
            let median = v[v.len() / 2];
            (c, (v.iter().sum::<f64>() * 0.1, median * 5.0))
        })
        .collect();
    spent
        .iter()
        .filter(|s| {
            let (tenth, five) = limits[s.currency.as_str()];
            !(s.amount > tenth && s.amount > five)
        })
        .collect()
}

/// Usual spending a day: by currency on everyday accounts, and by root category and currency.
pub fn pace(spent: &[Spent], days: i64) -> (Totals, HashMap<(i64, String), f64>) {
    let mut total = Totals::new();
    let mut by_cat: HashMap<(i64, String), f64> = HashMap::new();
    if days < MIN_DAYS {
        return (total, by_cat);
    }
    for s in usual(spent) {
        if s.everyday {
            *total.entry(s.currency.clone()).or_insert(0.0) += s.amount;
        }
        *by_cat.entry((s.root, s.currency.clone())).or_insert(0.0) += s.amount;
    }
    total.values_mut().for_each(|v| *v /= days as f64);
    by_cat.values_mut().for_each(|v| *v /= days as f64);
    (total, by_cat)
}

/// The n-th date of a recurring operation, as `recurring.rs` counts them.
fn occurrence(start: NaiveDate, every: &str, n: i64) -> Option<NaiveDate> {
    let n = u32::try_from(n).ok()?;
    match every {
        "week" => start.checked_add_days(Days::new(7 * u64::from(n))),
        "month" => start.checked_add_months(chrono::Months::new(n)),
        _ => start.checked_add_months(chrono::Months::new(n.checked_mul(12)?)),
    }
}

/// A recurring operation still to come: its dates after `today` and before `end`.
pub struct Planned {
    pub root: i64,
    pub kind: String,
    pub amount: f64,
    pub currency: String,
    pub every: String,
    pub start: NaiveDate,
    pub done: i64,
    /// Booked on an everyday account (or none), so it moves the balance.
    pub everyday: bool,
}

/// How many dates of a planned operation are not added yet and fall before `end`. Those due
/// by the server's day are already added (`done` counts them), so all the rest are to come,
/// also one due today on a device that is a day ahead.
pub fn times(p: &Planned, end: NaiveDate) -> i64 {
    let mut count = 0;
    while let Some(d) = occurrence(p.start, &p.every, p.done + count) {
        if d >= end || count > 60 {
            break;
        }
        count += 1;
    }
    count
}

/// The month's end by currency, from the balance now, the planned operations and the pace.
pub fn build(balance: &Totals, planned: &[Planned], per_day: &Totals, today: NaiveDate, end: NaiveDate, short_history: bool) -> Forecast {
    let days_left = (end - today).num_days() - 1;
    let mut by_currency: BTreeMap<String, Line> = BTreeMap::new();
    for (c, v) in balance {
        by_currency.entry(c.clone()).or_default().now = *v;
    }
    for p in planned.iter().filter(|p| p.everyday) {
        let sign = if p.kind == "income" { 1.0 } else { -1.0 };
        by_currency.entry(p.currency.clone()).or_default().scheduled += sign * p.amount * times(p, end) as f64;
    }
    for (c, v) in per_day {
        by_currency.entry(c.clone()).or_default().per_day = *v;
    }
    let r = |v: f64| (v * 100.0).round() / 100.0;
    by_currency.retain(|_, l| l.now.abs() >= 0.005 || l.scheduled.abs() >= 0.005 || l.per_day >= 0.005);
    for l in by_currency.values_mut() {
        l.scheduled = r(l.scheduled);
        l.per_day = r(l.per_day);
        l.end = r(l.now + l.scheduled - l.per_day * days_left as f64);
    }
    Forecast { days_left, short_history, by_currency }
}

/// What the home screen needs for the month of `today`: the forecast, and each root category's
/// expected spending by currency for the rest of the month (pace and planned expenses).
pub async fn load(db: &PgPool, budget: i64, today: NaiveDate, balance: &Totals) -> sqlx::Result<(Forecast, HashMap<(i64, String), f64>)> {
    let (_, end) = crate::db::month_bounds(today);
    let end = NaiveDate::parse_from_str(&end, "%Y-%m-%d").unwrap_or(today);
    let window_start = today - Days::new(WINDOW_DAYS);
    let first: Option<String> = sqlx::query_scalar(
        "SELECT MIN(t.transaction_date) FROM transactions t JOIN categories c ON c.id = t.category_id
         WHERE c.budget_id = $1 AND t.type = 'expense'",
    )
    .bind(budget)
    .fetch_one(db)
    .await?;
    let first = first.and_then(|f| NaiveDate::parse_from_str(&f, "%Y-%m-%d").ok()).unwrap_or(today);
    let from = first.max(window_start);
    let days = (today - from).num_days();

    let rows: Vec<(i64, String, f64, bool)> = sqlx::query_as(
        "SELECT COALESCE(c.parent_id, c.id), t.currency, t.amount, (a.id IS NULL OR a.kind = 'regular')
         FROM transactions t
         JOIN categories c ON c.id = t.category_id
         LEFT JOIN accounts a ON a.id = t.account_id
         WHERE c.budget_id = $1 AND t.type = 'expense' AND t.recurring_id IS NULL
           AND t.transaction_date >= $2 AND t.transaction_date < $3",
    )
    .bind(budget).bind(from.to_string()).bind(today.to_string())
    .fetch_all(db)
    .await?;
    let spent: Vec<Spent> = rows.into_iter().map(|(root, currency, amount, everyday)| Spent { root, currency, amount, everyday }).collect();
    let (per_day, by_cat) = pace(&spent, days);

    let rows: Vec<(i64, String, f64, String, String, String, i64, bool)> = sqlx::query_as(
        "SELECT COALESCE(c.parent_id, c.id), r.type, r.amount, r.currency, r.every, r.start_date, r.done, (a.id IS NULL OR a.kind = 'regular')
         FROM recurring r
         JOIN categories c ON c.id = r.category_id
         LEFT JOIN accounts a ON a.id = r.account_id
         WHERE r.budget_id = $1 AND r.next_date < $2",
    )
    .bind(budget).bind(end.to_string())
    .fetch_all(db)
    .await?;
    let planned: Vec<Planned> = rows
        .into_iter()
        .filter_map(|(root, kind, amount, currency, every, start, done, everyday)| {
            let start = NaiveDate::parse_from_str(&start, "%Y-%m-%d").ok()?;
            Some(Planned { root, kind, amount, currency, every, start, done, everyday })
        })
        .collect();

    let forecast = build(balance, &planned, &per_day, today, end, days < MIN_DAYS);
    // Per category: the pace for the days left plus its planned expenses.
    let mut expected: HashMap<(i64, String), f64> = by_cat.into_iter().map(|(k, v)| (k, v * forecast.days_left as f64)).collect();
    for p in planned.iter().filter(|p| p.kind == "expense") {
        *expected.entry((p.root, p.currency.clone())).or_insert(0.0) += p.amount * times(p, end) as f64;
    }
    Ok((forecast, expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn spent(amount: f64) -> Spent {
        Spent { root: 1, currency: "₽".into(), amount, everyday: true }
    }

    #[test]
    fn big_one_off_expenses_are_not_usual() {
        // 30 days of 100 a day, and rent of 30 000 once.
        let mut list: Vec<Spent> = (0..30).map(|_| spent(100.0)).collect();
        list.push(spent(30000.0));
        let (total, by_cat) = pace(&list, 30);
        assert_eq!(total["₽"], 100.0);
        assert_eq!(by_cat[&(1, "₽".to_string())], 100.0);
        // A few expenses of about the same size all count.
        let (total, _) = pace(&[spent(700.0), spent(800.0), spent(900.0)], 10);
        assert_eq!(total["₽"], 240.0);
        // Too little history: no pace.
        assert!(pace(&[spent(100.0)], 3).0.is_empty());
    }

    #[test]
    fn planned_operations_count_until_the_month_ends() {
        let p = |kind: &str, every: &str, start: &str, done: i64| Planned {
            root: 1, kind: kind.into(), amount: 100.0, currency: "$".into(), every: every.into(), start: d(start), done, everyday: true,
        };
        let (today, end) = (d("2026-11-06"), d("2026-12-01"));
        // Weekly from Nov 2: Nov 2 is done; 9, 16, 23 and 30 are still to come.
        assert_eq!(times(&p("expense", "week", "2026-11-02", 1), end), 4);
        // Monthly on the 25th, not yet this month.
        assert_eq!(times(&p("income", "month", "2026-01-25", 10), end), 1);
        // Monthly on the 1st: already added, the next is in December.
        assert_eq!(times(&p("expense", "month", "2026-01-01", 11), end), 0);

        let balance: Totals = [("$".to_string(), 1000.0)].into();
        let per_day: Totals = [("$".to_string(), 10.0), ("€".to_string(), 1.0)].into();
        let f = build(&balance, &[p("income", "month", "2026-01-25", 10), p("expense", "week", "2026-11-02", 1)], &per_day, today, end, false);
        assert_eq!(f.days_left, 24);
        assert_eq!(f.by_currency["$"], Line { now: 1000.0, scheduled: -300.0, per_day: 10.0, end: 460.0 });
        assert_eq!(f.by_currency["€"].end, -24.0);
    }
}
