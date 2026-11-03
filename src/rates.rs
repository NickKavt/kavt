//! Exchange rates from a public source, kept for an hour. Only shown to people who turned them
//! on; nothing in the budget is converted on the server.
//!
//! Every source is brought to one shape: how many lira one unit of each currency costs, so
//! the app converts between any two of ₺, $, € and ₽ with the same numbers.

use std::{collections::HashMap, sync::Mutex, time::{Duration, Instant}};

use serde::Serialize;

const KEEP: Duration = Duration::from_secs(3600);
/// Currencies of the app and their codes at the sources.
const CODES: [(&str, &str); 4] = [("₺", "TRY"), ("$", "USD"), ("€", "EUR"), ("₽", "RUB")];

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
        _ => None,
    }
}

impl Cache {
    pub async fn get(&self, source: &'static str) -> Result<Rates, String> {
        if let Some((at, r)) = self.0.lock().unwrap().get(source) {
            if at.elapsed() < KEEP {
                return Ok(r.clone());
            }
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

async fn fetch(source: &'static str) -> Result<Rates, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("budget-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let url = match source {
        "tcmb" => "https://www.tcmb.gov.tr/kurlar/today.xml",
        "cbr" => "https://www.cbr.ru/scripts/XML_daily.asp",
        _ => "https://api.frankfurter.dev/v1/latest?from=EUR&to=TRY,USD",
    };
    let res = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("{url}: {}", res.status()));
    }
    // The Russian central bank answers in windows-1251; only ASCII parts are read here.
    let body = String::from_utf8_lossy(&res.bytes().await.map_err(|e| e.to_string())?).into_owned();
    let rates = match source {
        "tcmb" => parse_tcmb(&body),
        "cbr" => parse_cbr(&body),
        _ => parse_ecb(&body),
    }
    .ok_or_else(|| format!("{url}: unexpected answer"))?;
    Ok(Rates { source, stale: false, ..rates })
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

/// TCMB: lira per unit; the mid of the forex buying and selling rates (selling may be empty).
fn parse_tcmb(xml: &str) -> Option<Rates> {
    let date = dotted_date(attr(xml, "Tarih")?)?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0)]);
    for (sym, code) in &CODES[1..] {
        let Some(at) = xml.find(&format!("CurrencyCode=\"{code}\"")) else { continue };
        let block = &xml[at..at + xml[at..].find("</Currency>")?];
        let unit = tag(block, "Unit").and_then(number).unwrap_or(1.0);
        let buy = tag(block, "ForexBuying").and_then(number);
        let sell = tag(block, "ForexSelling").and_then(number);
        let price = match (buy, sell) {
            (Some(b), Some(s)) => (b + s) / 2.0,
            (Some(v), None) | (None, Some(v)) => v,
            _ => continue,
        };
        in_try.insert(sym.to_string(), price / unit);
    }
    (in_try.len() > 1).then_some(Rates { source: "tcmb", date, in_try, stale: false })
}

/// Bank of Russia: roubles per `Nominal` units; turned into lira per unit through TRY.
fn parse_cbr(xml: &str) -> Option<Rates> {
    let date = dotted_date(attr(xml, "Date")?)?;
    let rub_per = |code: &str| -> Option<f64> {
        let at = xml.find(&format!("<CharCode>{code}</CharCode>"))?;
        let block = &xml[at..at + xml[at..].find("</Valute>")?];
        Some(number(tag(block, "Value")?)? / number(tag(block, "Nominal")?)?)
    };
    let try_in_rub = rub_per("TRY")?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0), ("₽".to_string(), 1.0 / try_in_rub)]);
    for (sym, code) in [("$", "USD"), ("€", "EUR")] {
        if let Some(v) = rub_per(code) {
            in_try.insert(sym.to_string(), v / try_in_rub);
        }
    }
    Some(Rates { source: "cbr", date, in_try, stale: false })
}

/// ECB through Frankfurter: per one euro. It has no rouble.
fn parse_ecb(json: &str) -> Option<Rates> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let date = v["date"].as_str()?.to_string();
    let try_per_eur = v["rates"]["TRY"].as_f64()?;
    let mut in_try = HashMap::from([("₺".to_string(), 1.0), ("€".to_string(), try_per_eur)]);
    if let Some(usd_per_eur) = v["rates"]["USD"].as_f64() {
        in_try.insert("$".to_string(), try_per_eur / usd_per_eur);
    }
    Some(Rates { source: "ecb", date, in_try, stale: false })
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
