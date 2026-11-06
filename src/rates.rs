//! Exchange rates from a public source, kept for an hour. Only shown to people who turned them
//! on; nothing in the budget is converted on the server.
//!
//! Every source is brought to one shape: how many lira one unit of each currency costs, so
//! the app converts between any two of ₺, $, € and ₽ with the same numbers.

use std::{collections::HashMap, sync::Mutex, time::{Duration, Instant}};

use chrono::{Datelike, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;

const KEEP: Duration = Duration::from_secs(3600);
/// The four currencies the app has always had keep their symbols as ids; every other currency
/// is its ISO code (`GBP`, `KZT`).
const CODES: [(&str, &str); 4] = [("₺", "TRY"), ("$", "USD"), ("€", "EUR"), ("₽", "RUB")];

/// The app's id of an ISO code: a symbol for the four, else the code in capitals.
pub fn app_id(code: &str) -> String {
    let c = code.trim().to_ascii_uppercase();
    CODES.iter().find(|(_, k)| *k == c).map_or(c, |(sym, _)| sym.to_string())
}

#[derive(Clone, Serialize)]
pub struct Rates {
    pub source: &'static str,
    /// The day the source published them, `YYYY-MM-DD`.
    pub date: String,
    /// Lira for one unit of each currency; a currency the source doesn't have is missing.
    pub in_try: HashMap<String, f64>,
    /// True when the source didn't answer and these are the last rates it gave.
    pub stale: bool,
}

#[derive(Default)]
pub struct Cache(Mutex<HashMap<&'static str, (Instant, Rates)>>);

pub fn source_name(s: &str) -> Option<&'static str> {
    match s {
        "tcmb" => Some("tcmb"),
        "cbr" => Some("cbr"),
        "ecb" => Some("ecb"),
        // currency-api (github.com/fawazahmed0/exchange-api): daily, 200+ currencies, no key.
        "fawaz" => Some("fawaz"),
        _ => None,
    }
}

/// The source asked when the chosen one has no rate for a currency.
pub const FALLBACK: &str = "fawaz";

impl Cache {
    pub async fn get(&self, source: &'static str) -> Result<Rates, String> {
        if let Some((at, r)) = self.0.lock().unwrap().get(source)
            && at.elapsed() < KEEP {
                return Ok(r.clone());
            }
        match fetch(source).await {
            Ok(r) => {
                self.0.lock().unwrap().insert(source, (Instant::now(), r.clone()));
                Ok(r)
            }
            Err(e) => {
                tracing::warn!(source, error = %e, "exchange rates unavailable");
                // Yesterday's rates are better than none; they are marked as such.
                match self.0.lock().unwrap().get(source) {
                    Some((_, r)) => Ok(Rates { stale: true, ..r.clone() }),
                    None => Err(e),
                }
            }
        }
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("budget-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Today's rates of a source.
async fn fetch(source: &'static str) -> Result<Rates, String> {
    let url = match source {
        "tcmb" => "https://www.tcmb.gov.tr/kurlar/today.xml".to_string(),
        "cbr" => "https://www.cbr.ru/scripts/XML_daily.asp".to_string(),
        "fawaz" => "https://cdn.jsdelivr.net/npm/@fawazahmed0/currency-api@latest/v1/currencies/try.min.json".to_string(),
        _ => "https://api.frankfurter.dev/v1/latest?from=EUR".to_string(),
    };
    get_parsed(&client()?, source, &url).await?.ok_or_else(|| format!("{url}: not published"))
}

/// One answer of a source, parsed. `Ok(None)`: the source has nothing for that address
/// (TCMB answers 404 for weekends and holidays).
async fn get_parsed(client: &reqwest::Client, source: &'static str, url: &str) -> Result<Option<Rates>, String> {
    let res = client.get(url).send().await.map_err(|e| e.to_string())?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(format!("{url}: {}", res.status()));
    }
    // The Russian central bank answers in windows-1251; only ASCII parts are read here.
    let body = String::from_utf8_lossy(&res.bytes().await.map_err(|e| e.to_string())?).into_owned();
    let rates = match source {
        "tcmb" => parse_tcmb(&body),
        "cbr" => parse_cbr(&body),
        "fawaz" => parse_fawaz(&body),
        _ => parse_ecb(&body),
    }
    .ok_or_else(|| format!("{url}: unexpected answer"))?;
    Ok(Some(Rates { source, stale: false, ..rates }))
}

/// Rates a source gave for `day`: the last ones published on or before it. A day in the
/// future is asked as today. Fetched once per day and kept in `daily_rates`.
pub async fn on_day(db: &PgPool, source: &'static str, day: NaiveDate) -> Result<Rates, String> {
    let day = day.min(Utc::now().date_naive());
    let key = day.to_string();
    let cached: Option<(String, String)> = sqlx::query_as("SELECT published, in_try::text FROM daily_rates WHERE source = $1 AND day = $2")
        .bind(source).bind(&key)
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;
    if let Some((published, in_try)) = cached {
        let in_try = serde_json::from_str(&in_try).map_err(|e| e.to_string())?;
        return Ok(Rates { source, date: published, in_try, stale: false });
    }
    let client = client()?;
    let mut found = None;
    match source {
        // An archive file per working day; step back over weekends and holidays.
        "tcmb" => {
            for back in 0..10 {
                let d = day - chrono::Days::new(back);
                let url = format!("https://www.tcmb.gov.tr/kurlar/{}/{}.xml", d.format("%Y%m"), d.format("%d%m%Y"));
                if let Some(r) = get_parsed(&client, source, &url).await? {
                    found = Some(r);
                    break;
                }
            }
        }
        // Both answer with the last rates published on or before the day asked.
        "cbr" => found = get_parsed(&client, source, &format!("https://www.cbr.ru/scripts/XML_daily.asp?date_req={}", day.format("%d/%m/%Y"))).await?,
        // A file per day; a day not published yet (today, early) falls back to the one before.
        "fawaz" => {
            for back in 0..4 {
                let d = day - chrono::Days::new(back);
                let url = format!("https://cdn.jsdelivr.net/npm/@fawazahmed0/currency-api@{d}/v1/currencies/try.min.json");
                if let Some(r) = get_parsed(&client, source, &url).await? {
                    found = Some(r);
                    break;
                }
            }
        }
        _ => found = get_parsed(&client, source, &format!("https://api.frankfurter.dev/v1/{day}?from=EUR")).await?,
    }
    let rates = found.ok_or_else(|| format!("{source}: no rates for {day}"))?;
    sqlx::query("INSERT INTO daily_rates (source, day, published, in_try) VALUES ($1, $2, $3, $4::jsonb) ON CONFLICT DO NOTHING")
        .bind(source).bind(&key).bind(&rates.date).bind(serde_json::to_string(&rates.in_try).map_err(|e| e.to_string())?)
        .execute(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rates)
}

/// The app's id of a currency as stored or sent: `TRY` → `₺` for the four, `gbp` → `GBP`.
pub fn symbol(currency: &str) -> String {
    let c = currency.trim();
    if let Some((sym, _)) = CODES.iter().find(|(_, code)| code.eq_ignore_ascii_case(c)) {
        return sym.to_string();
    }
    if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()) { c.to_ascii_uppercase() } else { c.to_string() }
}

/// Units of `to` for one unit of `from`, by one source's rates.
pub fn convert(r: &Rates, from: &str, to: &str) -> Option<f64> {
    Some(r.in_try.get(&symbol(from))? / r.in_try.get(&symbol(to))?)
}

#[derive(Clone, Serialize, serde::Deserialize)]
pub struct Settings {
    pub main_currency: String,
    pub rate_source: String,
    /// The budget's savings goal in its main currency (changed by its own request).
    #[serde(default)]
    pub savings_goal: Option<f64>,
}

pub async fn settings(db: &PgPool, budget: i64) -> sqlx::Result<Settings> {
    let row: Option<(String, String, Option<f64>)> = sqlx::query_as("SELECT main_currency, rate_source, savings_goal FROM budget_settings WHERE budget_id = $1")
        .bind(budget)
        .fetch_optional(db)
        .await?;
    let (main_currency, rate_source, savings_goal) = row.unwrap_or_else(|| ("₺".into(), "tcmb".into(), None));
    Ok(Settings { main_currency, rate_source, savings_goal })
}

/// Saves the settings; the rates of the budget's operations are looked up again for them.
pub async fn save_settings(db: &PgPool, budget: i64, s: &Settings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO budget_settings (budget_id, main_currency, rate_source) VALUES ($1, $2, $3)
         ON CONFLICT (budget_id) DO UPDATE SET main_currency = $2, rate_source = $3",
    )
    .bind(budget).bind(&s.main_currency).bind(&s.rate_source)
    .execute(db)
    .await?;
    sqlx::query(
        "UPDATE transactions t SET rate = NULL, rate_to = NULL, rate_day = NULL, rate_source = NULL
         FROM categories c WHERE c.id = t.category_id AND c.budget_id = $1",
    )
    .bind(budget)
    .execute(db)
    .await?;
    Ok(())
}

/// Forget an operation's rate (with its transfer half), after its currency or day changed.
pub async fn forget(db: &PgPool, id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE transactions SET rate = NULL, rate_to = NULL, rate_day = NULL, rate_source = NULL
         WHERE id = $1 OR id = (SELECT pair_id FROM transactions WHERE id = $1)",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Looks up the rate of operations that don't have one yet, on each operation's own day.
/// A source that doesn't answer is tried again on the next run; nothing else waits for it.
pub async fn fill(db: &PgPool, budget: Option<i64>) -> sqlx::Result<usize> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT t.id, t.currency, t.transaction_date, COALESCE(s.main_currency, '₺'), COALESCE(s.rate_source, 'tcmb')
         FROM transactions t
         JOIN categories c ON c.id = t.category_id
         LEFT JOIN budget_settings s ON s.budget_id = c.budget_id
         WHERE t.rate_source IS NULL AND COALESCE(s.rate_source, 'tcmb') <> 'off'
           AND ($1::BIGINT IS NULL OR c.budget_id = $1)
         ORDER BY t.id
         LIMIT 500",
    )
    .bind(budget)
    .fetch_all(db)
    .await?;
    let mut failed: std::collections::HashSet<(String, String)> = Default::default();
    let mut done = 0;
    for (id, currency, date, main, src) in rows {
        if symbol(&currency) == symbol(&main) {
            sqlx::query(
                "UPDATE transactions SET rate = 1, rate_to = $3, rate_day = transaction_date, rate_source = 'same'
                 WHERE id = $1 AND rate_source IS NULL AND currency = $2 AND EXISTS (SELECT 1 FROM categories c LEFT JOIN budget_settings s ON s.budget_id = c.budget_id
                       WHERE c.id = transactions.category_id AND COALESCE(s.main_currency, '₺') = $3 AND COALESCE(s.rate_source, 'tcmb') = $4)",
            )
            .bind(id).bind(&currency).bind(&main).bind(&src)
                .execute(db)
                .await?;
            done += 1;
            continue;
        }
        let (Some(source), Ok(day)) = (source_name(&src), date.parse::<NaiveDate>()) else { continue };
        if failed.contains(&(src.clone(), date.clone())) {
            continue;
        }
        match on_day(db, source, day).await {
            Ok(r) => {
                let mut rate = convert(&r, &currency, &main);
                let mut r = r;
                // The chosen source doesn't list this currency: ask the one that lists them all.
                if rate.is_none() && source != FALLBACK
                    && let Ok(f) = on_day(db, FALLBACK, day).await {
                        rate = convert(&f, &currency, &main);
                        if rate.is_some() {
                            r = f;
                        }
                    }
                let source = r.source;
                // Only if the operation still has the day and currency the rate was found for:
                // it may have been changed meanwhile, and then it waits for its own lookup.
                sqlx::query(
                    "UPDATE transactions SET rate = $2, rate_to = $3, rate_day = $4, rate_source = $5
                     WHERE id = $1 AND rate_source IS NULL AND transaction_date = $6 AND currency = $7 AND EXISTS (SELECT 1 FROM categories c LEFT JOIN budget_settings s ON s.budget_id = c.budget_id
                       WHERE c.id = transactions.category_id AND COALESCE(s.main_currency, '₺') = $3 AND COALESCE(s.rate_source, 'tcmb') = $8)",
                )
                .bind(id).bind(rate).bind(&main).bind(&r.date).bind(if rate.is_some() { source } else { "none" }).bind(&date).bind(&currency).bind(&src)
                    .execute(db)
                    .await?;
                done += 1;
            }
            Err(e) => {
                tracing::warn!(source, day = %day, error = %e, "rate for the day unavailable");
                failed.insert((src, date));
            }
        }
    }
    Ok(done)
}

/// Text between `<tag>` and `</tag>` after `from`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

fn number(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0)
}

/// `DD.MM.YYYY` → `YYYY-MM-DD`.
fn dotted_date(s: &str) -> Option<String> {
    let mut p = s.split('.');
    let (d, m, y) = (p.next()?, p.next()?, p.next()?);
    Some(format!("{y}-{m}-{d}"))
}

fn attr<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = text.find(&key)? + key.len();
    Some(&text[start..start + text[start..].find('"')?])
}

/// TCMB: lira per unit of every currency it lists; the mid of the forex buying and selling
/// rates (selling may be empty).
fn parse_tcmb(xml: &str) -> Option<Rates> {
    let date = dotted_date(attr(xml, "Tarih")?)?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0)]);
    let mut rest = xml;
    while let Some(at) = rest.find("<Currency ") {
        let block_end = rest[at..].find("</Currency>").map(|e| at + e)?;
        let block = &rest[at..block_end];
        rest = &rest[block_end..];
        let Some(code) = attr(block, "CurrencyCode") else { continue };
        let unit = tag(block, "Unit").and_then(number).unwrap_or(1.0);
        let buy = tag(block, "ForexBuying").and_then(number);
        let sell = tag(block, "ForexSelling").and_then(number);
        let price = match (buy, sell) {
            (Some(b), Some(s)) => (b + s) / 2.0,
            (Some(v), None) | (None, Some(v)) => v,
            _ => continue,
        };
        in_try.insert(app_id(code), price / unit);
    }
    (in_try.len() > 1).then_some(Rates { source: "tcmb", date, in_try, stale: false })
}

/// Bank of Russia: roubles per `Nominal` units of each currency; turned into lira per unit through TRY.
fn parse_cbr(xml: &str) -> Option<Rates> {
    let date = dotted_date(attr(xml, "Date")?)?;
    let mut rub_per: HashMap<String, f64> = HashMap::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<Valute") {
        let block_end = rest[at..].find("</Valute>").map(|e| at + e)?;
        let block = &rest[at..block_end];
        rest = &rest[block_end..];
        let (Some(code), Some(value), Some(nominal)) = (tag(block, "CharCode"), tag(block, "Value").and_then(number), tag(block, "Nominal").and_then(number)) else { continue };
        rub_per.insert(code.to_string(), value / nominal);
    }
    let try_in_rub = *rub_per.get("TRY")?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0), ("₽".to_string(), 1.0 / try_in_rub)]);
    for (code, v) in rub_per {
        if code != "TRY" {
            in_try.insert(app_id(&code), v / try_in_rub);
        }
    }
    Some(Rates { source: "cbr", date, in_try, stale: false })
}

/// ECB through Frankfurter: units of each currency per one euro. It has no rouble.
fn parse_ecb(json: &str) -> Option<Rates> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let date = v["date"].as_str()?.to_string();
    let rates = v["rates"].as_object()?;
    let try_per_eur = rates.get("TRY")?.as_f64()?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0), ("€".to_string(), try_per_eur)]);
    for (code, per_eur) in rates {
        if let Some(x) = per_eur.as_f64().filter(|x| *x > 0.0)
            && code != "TRY" {
                in_try.insert(app_id(code), try_per_eur / x);
            }
    }
    Some(Rates { source: "ecb", date, in_try, stale: false })
}

/// currency-api: units of each currency per one lira.
fn parse_fawaz(json: &str) -> Option<Rates> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let date = v["date"].as_str()?.to_string();
    let per_try = v["try"].as_object()?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0)]);
    for (code, x) in per_try {
        // ISO codes only: the file also lists crypto and metals under longer names.
        if code.len() == 3 && code.chars().all(|c| c.is_ascii_alphabetic()) && code != "try"
            && let Some(x) = x.as_f64().filter(|x| x.is_finite() && *x > 0.0) {
                in_try.insert(app_id(code), 1.0 / x);
            }
    }
    (in_try.len() > 1).then_some(Rates { source: "fawaz", date, in_try, stale: false })
}

/// The budget's savings goal; `None` or 0 removes it.
/// The goal, and its month when `date` is given (`Some(None)` removes it). No goal, no month.
pub async fn save_goal(db: &PgPool, budget: i64, goal: Option<f64>, date: Option<Option<NaiveDate>>) -> sqlx::Result<()> {
    let goal = goal.filter(|g| *g > 0.0);
    let date = if goal.is_none() { Some(None) } else { date };
    sqlx::query(
        "INSERT INTO budget_settings (budget_id, savings_goal, savings_goal_date) VALUES ($1, $2, $4)
         ON CONFLICT (budget_id) DO UPDATE SET savings_goal = $2,
             savings_goal_date = CASE WHEN $3 THEN $4 ELSE budget_settings.savings_goal_date END",
    )
    .bind(budget).bind(goal).bind(date.is_some()).bind(date.flatten().and_then(|d| d.with_day(1)).map(|d| d.to_string()))
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcmb() {
        let xml = r#"<Tarih_Date Tarih="03.11.2026" Date="11/03/2026">
          <Currency Kod="USD" CurrencyCode="USD"><Unit>1</Unit><ForexBuying>49.9094</ForexBuying><ForexSelling>49.9993</ForexSelling></Currency>
          <Currency Kod="RUB" CurrencyCode="RUB"><Unit>1</Unit><ForexBuying>0.60966</ForexBuying><ForexSelling></ForexSelling></Currency>
          <Currency Kod="JPY" CurrencyCode="JPY"><Unit>100</Unit><ForexBuying>32.5</ForexBuying><ForexSelling>32.7</ForexSelling></Currency></Tarih_Date>"#;
        let r = parse_tcmb(xml).unwrap();
        assert_eq!(r.date, "2026-11-03");
        assert!((r.in_try["$"] - 49.95435).abs() < 1e-9);
        assert!((r.in_try["₽"] - 0.60966).abs() < 1e-9);
        assert!(!r.in_try.contains_key("€"));
    }

    #[test]
    fn cbr() {
        let xml = r#"<ValCurs Date="04.11.2026" name="x"><Valute><CharCode>USD</CharCode><Nominal>1</Nominal><Value>81,5</Value></Valute>
          <Valute><CharCode>TRY</CharCode><Nominal>10</Nominal><Value>16,3</Value></Valute></ValCurs>"#;
        let r = parse_cbr(xml).unwrap();
        assert_eq!(r.date, "2026-11-04");
        // 1 ₺ = 1.63 ₽, 1 $ = 81.5 ₽ = 50 ₺.
        assert!((r.in_try["₽"] - 1.0 / 1.63).abs() < 1e-9);
        assert!((r.in_try["$"] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn ecb() {
        let r = parse_ecb(r#"{"amount":1.0,"base":"EUR","date":"2026-11-02","rates":{"TRY":56.0,"USD":1.12}}"#).unwrap();
        assert!((r.in_try["$"] - 50.0).abs() < 1e-9);
        assert!(!r.in_try.contains_key("₽"));
    }
}
