//! The public demo (`BUDGET_DEMO=1`): each visitor gets a budget of their own, filled with
//! three months of made-up money in their language, without a login or password. A demo budget
//! is deleted a day after it was made. Things that only make sense for real people (passwords,
//! two-step sign-in, invitations, Telegram, the owner of the install) are switched off.

use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use rand::RngExt;
use serde::Deserialize;

use crate::{ApiError, ApiResult, AppRef, AppState, accounts, db, import::ImportRow, local, savings};

/// Demo users are told apart by this prefix of `telegram_id`.
pub const PREFIX: &str = "demo:";
/// How many demo budgets may exist at once; each takes a few hundred rows.
const MAX_BUDGETS: i64 = 300;
const LIFETIME_HOURS: i64 = 24;
/// The largest import a demo budget accepts.
const MAX_IMPORT_BYTES: u64 = 1 << 20;

pub const OFF: &str = "В демо это недоступно";
const FULL: &str = "Демо сейчас занято, попробуйте позже";

#[derive(Deserialize)]
pub struct DemoIn {
    #[serde(default)]
    lang: Option<String>,
}

/// Names and money of a demo budget in one language.
struct Story {
    /// The budget's language, one of `db::LANGS`.
    lang: &'static str,
    guest: &'static str,
    main: &'static str,
    /// One unit of `main` in dollars' worth, so amounts look ordinary in every currency.
    scale: f64,
    /// Amounts in `main` are rounded to this.
    step: f64,
    rate_source: &'static str,
    second: &'static str,
    /// Units of `second` for one unit of `main`.
    second_rate: f64,
    card: &'static str,
    cash: &'static str,
    second_card: &'static str,
    trip: &'static str,
    deposit: &'static str,
    /// Annual rate and tax of the deposit, percent.
    deposit_rate: (f64, f64),
    salary: &'static str,
    side_job: &'static str,
    supermarket: &'static str,
    market: &'static str,
    pharmacy: &'static str,
    rent: &'static str,
    utilities: &'static str,
    cafe: &'static str,
    cinema: &'static str,
    transport: &'static str,
    subscription: &'static str,
}

/// In the order of `db::LANGS`.
const STORIES: [Story; 15] = [
    Story {
        lang: "ru", guest: "Гость", main: "₽", scale: 90.0, step: 10.0, rate_source: "cbr", second: "$", second_rate: 1.0 / 90.0,
        card: "Карта", cash: "Наличные", second_card: "Долларовая карта", trip: "Отпуск", deposit: "Вклад", deposit_rate: (18.0, 0.0),
        salary: "Зарплата", side_job: "Подработка", supermarket: "Супермаркет", market: "Рынок", pharmacy: "Аптека", rent: "Аренда", utilities: "Коммуналка", cafe: "Кафе", cinema: "Кино", transport: "Транспорт", subscription: "Подписки",
    },
    Story {
        lang: "en", guest: "Guest", main: "$", scale: 1.0, step: 0.01, rate_source: "ecb", second: "€", second_rate: 0.92,
        card: "Card", cash: "Cash", second_card: "Euro card", trip: "Vacation", deposit: "Term deposit", deposit_rate: (4.5, 0.0),
        salary: "Salary", side_job: "Freelance", supermarket: "Supermarket", market: "Farmers market", pharmacy: "Pharmacy", rent: "Rent", utilities: "Utilities", cafe: "Cafe", cinema: "Cinema", transport: "Transport", subscription: "Streaming",
    },
    Story {
        lang: "tr", guest: "Misafir", main: "₺", scale: 40.0, step: 5.0, rate_source: "tcmb", second: "€", second_rate: 1.0 / 45.0,
        card: "Kart", cash: "Nakit", second_card: "Euro hesabı", trip: "Tatil", deposit: "Vadeli mevduat", deposit_rate: (40.0, 17.5),
        salary: "Maaş", side_job: "Serbest iş", supermarket: "Süpermarket", market: "Pazar", pharmacy: "Eczane", rent: "Kira", utilities: "Faturalar", cafe: "Kafe", cinema: "Sinema", transport: "Ulaşım", subscription: "Abonelikler",
    },
    Story {
        lang: "es", guest: "Invitado", main: "€", scale: 0.92, step: 0.01, rate_source: "ecb", second: "$", second_rate: 1.0 / 0.92,
        card: "Tarjeta", cash: "Efectivo", second_card: "Tarjeta en dólares", trip: "Vacaciones", deposit: "Depósito a plazo", deposit_rate: (2.5, 19.0),
        salary: "Nómina", side_job: "Trabajos extra", supermarket: "Supermercado", market: "Mercado", pharmacy: "Farmacia", rent: "Alquiler", utilities: "Facturas", cafe: "Cafetería", cinema: "Cine", transport: "Transporte", subscription: "Suscripciones",
    },
    Story {
        lang: "pt", guest: "Visitante", main: "BRL", scale: 5.5, step: 1.0, rate_source: "ecb", second: "$", second_rate: 1.0 / 5.5,
        card: "Cartão", cash: "Dinheiro", second_card: "Cartão em dólar", trip: "Férias", deposit: "CDB", deposit_rate: (11.0, 22.5),
        salary: "Salário", side_job: "Freela", supermarket: "Supermercado", market: "Feira", pharmacy: "Farmácia", rent: "Aluguel", utilities: "Contas da casa", cafe: "Café", cinema: "Cinema", transport: "Transporte", subscription: "Assinaturas",
    },
    Story {
        lang: "fr", guest: "Invité", main: "€", scale: 0.92, step: 0.01, rate_source: "ecb", second: "$", second_rate: 1.0 / 0.92,
        card: "Carte", cash: "Espèces", second_card: "Carte en dollars", trip: "Vacances", deposit: "Compte à terme", deposit_rate: (3.0, 30.0),
        salary: "Salaire", side_job: "Missions freelance", supermarket: "Supermarché", market: "Marché", pharmacy: "Pharmacie", rent: "Loyer", utilities: "Charges", cafe: "Café", cinema: "Cinéma", transport: "Transports", subscription: "Abonnements",
    },
    Story {
        lang: "de", guest: "Gast", main: "€", scale: 0.92, step: 0.01, rate_source: "ecb", second: "$", second_rate: 1.0 / 0.92,
        card: "Girokarte", cash: "Bargeld", second_card: "Dollarkarte", trip: "Urlaub", deposit: "Festgeld", deposit_rate: (3.0, 26.375),
        salary: "Gehalt", side_job: "Nebenjob", supermarket: "Supermarkt", market: "Wochenmarkt", pharmacy: "Apotheke", rent: "Miete", utilities: "Nebenkosten", cafe: "Café", cinema: "Kino", transport: "ÖPNV", subscription: "Abos",
    },
    Story {
        lang: "it", guest: "Ospite", main: "€", scale: 0.92, step: 0.01, rate_source: "ecb", second: "$", second_rate: 1.0 / 0.92,
        card: "Carta", cash: "Contanti", second_card: "Carta in dollari", trip: "Vacanze", deposit: "Conto deposito", deposit_rate: (3.0, 26.0),
        salary: "Stipendio", side_job: "Lavoretti", supermarket: "Supermercato", market: "Mercato", pharmacy: "Farmacia", rent: "Affitto", utilities: "Bollette", cafe: "Bar", cinema: "Cinema", transport: "Trasporti", subscription: "Abbonamenti",
    },
    Story {
        lang: "pl", guest: "Gość", main: "PLN", scale: 4.0, step: 1.0, rate_source: "ecb", second: "€", second_rate: 1.0 / 4.3,
        card: "Karta", cash: "Gotówka", second_card: "Karta w euro", trip: "Wakacje", deposit: "Lokata", deposit_rate: (5.5, 19.0),
        salary: "Wynagrodzenie", side_job: "Zlecenia", supermarket: "Supermarket", market: "Targ", pharmacy: "Apteka", rent: "Czynsz", utilities: "Rachunki", cafe: "Kawiarnia", cinema: "Kino", transport: "Komunikacja", subscription: "Subskrypcje",
    },
    Story {
        lang: "uk", guest: "Гість", main: "UAH", scale: 41.0, step: 10.0, rate_source: "fawaz", second: "$", second_rate: 1.0 / 41.0,
        card: "Картка", cash: "Готівка", second_card: "Доларова картка", trip: "Відпустка", deposit: "Депозит", deposit_rate: (14.0, 23.0),
        salary: "Зарплата", side_job: "Підробіток", supermarket: "Супермаркет", market: "Ринок", pharmacy: "Аптека", rent: "Оренда", utilities: "Комуналка", cafe: "Кав’ярня", cinema: "Кіно", transport: "Транспорт", subscription: "Підписки",
    },
    Story {
        lang: "be", guest: "Госць", main: "BYN", scale: 3.3, step: 1.0, rate_source: "fawaz", second: "$", second_rate: 1.0 / 3.3,
        card: "Картка", cash: "Наяўныя", second_card: "Даляравая картка", trip: "Адпачынак", deposit: "Дэпазіт", deposit_rate: (10.0, 0.0),
        salary: "Заробак", side_job: "Падпрацоўка", supermarket: "Супермаркет", market: "Рынак", pharmacy: "Аптэка", rent: "Арэнда", utilities: "Камуналка", cafe: "Кавярня", cinema: "Кіно", transport: "Транспарт", subscription: "Падпіскі",
    },
    Story {
        lang: "zh", guest: "访客", main: "CNY", scale: 7.2, step: 1.0, rate_source: "ecb", second: "$", second_rate: 1.0 / 7.2,
        card: "银行卡", cash: "现金", second_card: "美元卡", trip: "旅行", deposit: "定期存款", deposit_rate: (1.5, 0.0),
        salary: "工资", side_job: "兼职", supermarket: "超市", market: "菜市场", pharmacy: "药店", rent: "房租", utilities: "水电燃气", cafe: "咖啡馆", cinema: "电影", transport: "交通", subscription: "会员订阅",
    },
    Story {
        lang: "ja", guest: "ゲスト", main: "JPY", scale: 150.0, step: 10.0, rate_source: "ecb", second: "$", second_rate: 1.0 / 150.0,
        card: "カード", cash: "現金", second_card: "ドル口座", trip: "旅行", deposit: "定期預金", deposit_rate: (0.3, 20.315),
        salary: "給料", side_job: "副業", supermarket: "スーパー", market: "市場", pharmacy: "薬局", rent: "家賃", utilities: "光熱費", cafe: "カフェ", cinema: "映画", transport: "交通費", subscription: "サブスク",
    },
    Story {
        lang: "hi", guest: "अतिथि", main: "INR", scale: 84.0, step: 10.0, rate_source: "ecb", second: "$", second_rate: 1.0 / 84.0,
        card: "कार्ड", cash: "नकद", second_card: "डॉलर कार्ड", trip: "छुट्टियाँ", deposit: "सावधि जमा", deposit_rate: (7.0, 10.0),
        salary: "वेतन", side_job: "फ्रीलांस", supermarket: "सुपरमार्केट", market: "सब्ज़ी मंडी", pharmacy: "दवाखाना", rent: "किराया", utilities: "बिजली-पानी", cafe: "कैफ़े", cinema: "सिनेमा", transport: "आना-जाना", subscription: "सब्सक्रिप्शन",
    },
    Story {
        lang: "id", guest: "Tamu", main: "IDR", scale: 16000.0, step: 1000.0, rate_source: "ecb", second: "$", second_rate: 1.0 / 16000.0,
        card: "Kartu debit", cash: "Tunai", second_card: "Kartu dolar", trip: "Liburan", deposit: "Deposito", deposit_rate: (4.0, 20.0),
        salary: "Gaji", side_job: "Kerja sampingan", supermarket: "Supermarket", market: "Pasar", pharmacy: "Apotek", rent: "Sewa", utilities: "Tagihan", cafe: "Kafe", cinema: "Bioskop", transport: "Transportasi", subscription: "Langganan",
    },
];

/// The English one for a language without a story.
fn story(lang: &str) -> &'static Story {
    STORIES.iter().find(|s| s.lang == lang).unwrap_or(&STORIES[1])
}

/// A small deterministic generator, so a budget's numbers vary but need no extra crate state.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
    fn chance(&mut self, p: f64) -> bool {
        self.next() < p
    }
}

/// The operations of the demo budget from `start` to `today`, in the import's form.
fn rows(s: &Story, start: NaiveDate, today: NaiveDate, dice: &mut Dice) -> Vec<ImportRow> {
    let n = names_of(s);
    let money = |dollars: f64| ((dollars * s.scale / s.step).round() * s.step * 100.0).round() / 100.0;
    let mut out = Vec::new();
    let opening = |account: &str, kind: &str, amount: f64, cur: &str| ImportRow {
        date: start, kind: "opening".into(), category: kind.into(), amount, currency: cur.into(), account: account.into(), ..Default::default()
    };
    out.push(opening(s.card, "regular", money(800.0), s.main));
    out.push(opening(s.cash, "regular", money(150.0), s.main));
    out.push(opening(s.second_card, "regular", (60.0 * s.scale * s.second_rate).round(), s.second));
    out.push(opening(s.trip, "savings", money(800.0), s.main));

    let spend = |date, category: &str, sub: &str, amount: f64, account: &str, cur: &str| ImportRow {
        date, kind: "expense".into(), category: category.into(), subcategory: sub.into(), amount, currency: cur.into(), account: account.into(), ..Default::default()
    };
    let mut day = start;
    while day <= today {
        let dom = day.day();
        let weekday = day.weekday().num_days_from_monday();
        let earn = |sub: &str, amount: f64| ImportRow {
            date: day, kind: "income".into(), category: n.income.into(), subcategory: sub.into(), amount, currency: s.main.into(), account: s.card.into(), ..Default::default()
        };
        if dom == 1 {
            out.push(spend(day, n.home, s.rent, money(1100.0), s.card, s.main));
        }
        if dom == 5 {
            out.push(earn(s.salary, money(2600.0)));
        }
        if dom == 15 && dice.chance(0.7) {
            out.push(earn(s.side_job, money(dice.between(250.0, 600.0))));
        }
        if dom == 10 {
            out.push(spend(day, n.home, s.utilities, money(dice.between(90.0, 150.0)), s.card, s.main));
        }
        if dom == 6 {
            // Part of the salary goes to savings and some is changed into the second currency.
            out.push(ImportRow {
                date: day, kind: "transfer".into(), amount: money(300.0), currency: s.main.into(), account: s.card.into(), to_account: s.trip.into(), ..Default::default()
            });
            let given = money(100.0);
            out.push(ImportRow {
                date: day, kind: "transfer".into(), amount: given, currency: s.main.into(), account: s.card.into(), to_account: s.second_card.into(),
                to_amount: Some((given * s.second_rate * 100.0).round() / 100.0), to_currency: s.second.into(), ..Default::default()
            });
        }
        if dom == 2 {
            // Cash for the market and the pharmacy.
            out.push(ImportRow {
                date: day, kind: "transfer".into(), amount: money(120.0), currency: s.main.into(), account: s.card.into(), to_account: s.cash.into(), ..Default::default()
            });
        }
        if dom == 12 {
            out.push(spend(day, n.fun, s.subscription, (15.0 * s.scale * s.second_rate * 100.0).round() / 100.0, s.second_card, s.second));
        }
        if weekday == 1 || weekday == 5 {
            out.push(spend(day, n.groceries, s.supermarket, money(dice.between(35.0, 95.0)), s.card, s.main));
        }
        if weekday == 6 {
            out.push(spend(day, n.groceries, s.market, money(dice.between(12.0, 30.0)), s.cash, s.main));
        }
        if weekday < 5 && dice.chance(0.5) {
            out.push(spend(day, n.other, s.transport, money(dice.between(2.0, 9.0)), s.card, s.main));
        }
        if dice.chance(0.25) {
            out.push(spend(day, n.fun, s.cafe, money(dice.between(6.0, 28.0)), s.card, s.main));
        }
        if dice.chance(0.07) {
            out.push(spend(day, n.health, s.pharmacy, money(dice.between(8.0, 45.0)), s.cash, s.main));
        }
        if dom == 20 {
            out.push(spend(day, n.fun, s.cinema, money(dice.between(18.0, 30.0)), s.card, s.main));
        }
        day += Duration::days(1);
    }
    out
}

/// The seeded category names of the story's language.
struct Seeded {
    income: &'static str,
    groceries: &'static str,
    health: &'static str,
    home: &'static str,
    fun: &'static str,
    other: &'static str,
}

fn names_of(s: &Story) -> Seeded {
    let n = db::names(s.lang);
    Seeded { income: n.income, groceries: n.expenses[0].0, health: n.expenses[1].0, home: n.expenses[2].0, fun: n.expenses[3].0, other: n.expenses[4].0 }
}

/// A new demo budget, signed in.
pub async fn start(State(state): AppRef, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(d): Json<DemoIn>) -> ApiResult<Response> {
    if !state.cfg.demo {
        return Err(ApiError::status(404, "Not found"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE telegram_id LIKE 'demo:%'").fetch_one(&state.db).await?;
    if count >= MAX_BUDGETS {
        return Err(ApiError::status(503, FULL));
    }
    let lang = db::budget_lang_of(d.lang.as_deref());
    let s = story(lang);
    let tag = hex::encode(rand::rng().random::<[u8; 12]>());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (telegram_id, first_name, language_code, role) VALUES ($1, $2, $3, 'admin') RETURNING id",
    )
    .bind(format!("{PREFIX}{tag}")).bind(s.guest).bind(lang)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("UPDATE users SET budget_owner_id = id WHERE id = $1").bind(id).execute(&state.db).await?;
    if let Err(e) = fill(&state, id, lang).await {
        remove(&state.db, id).await.ok();
        return Err(e);
    }
    tracing::info!(user = id, lang, "demo budget made");
    local::signed_in(&state, &headers, addr, id).await
}

async fn fill(state: &AppState, budget: i64, lang: &'static str) -> ApiResult<()> {
    let s = story(lang);
    db::seed_budget(&state.db, budget, lang).await?;
    crate::rates::save_settings(&state.db, budget, &crate::rates::Settings {
        main_currency: s.main.into(), rate_source: s.rate_source.into(), savings_goal: None,
    })
    .await?;
    let today = Utc::now().date_naive();
    let start = first_of_month(today, 2);
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15 ^ (budget as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
    crate::import::import_rows(&state.db, budget, budget, &rows(s, start, today, &mut dice), false).await?;

    let money = |dollars: f64| (dollars * s.scale / s.step).round() * s.step;
    crate::rates::save_goal(&state.db, budget, Some(money(5000.0)), None).await?;
    // The trip is saved for by a month eight months from now.
    let trip_month = (today.with_day(1).unwrap_or(today) + chrono::Months::new(8)).to_string();
    sqlx::query("UPDATE accounts SET target_amount = $3, target_date = $4, emoji = '🏖️' WHERE budget_id = $1 AND name = $2")
        .bind(budget).bind(s.trip).bind(money(3000.0)).bind(trip_month)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE accounts SET emoji = '💵' WHERE budget_id = $1 AND name = $2").bind(budget).bind(s.cash).execute(&state.db).await?;
    // Monthly limits: groceries usually fit, fun usually goes over, so both looks show.
    let n = names_of(s);
    for (name, dollars) in [(n.groceries, 700.0), (n.fun, 200.0), (n.other, 90.0)] {
        sqlx::query("UPDATE categories SET limit_amount = $3, limit_currency = $4 WHERE budget_id = $1 AND name = $2 AND parent_id IS NULL AND type = 'expense'")
            .bind(budget).bind(name).bind(money(dollars)).bind(s.main)
            .execute(&state.db)
            .await?;
    }
    let card: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE budget_id = $1 AND name = $2")
        .bind(budget).bind(s.card)
        .fetch_one(&state.db)
        .await?;

    // A term deposit opened three weeks ago that comes back to the card at its end.
    let opened = today - Duration::days(21);
    let deposit = accounts::AccountIn {
        name: s.deposit.into(),
        emoji: Some("🏦".into()),
        currency: s.main.into(),
        owner_id: None,
        initial_amount: money(5000.0),
        initial_date: Some(opened),
        archived: false,
        kind: "savings".into(),
        target_amount: None,
        target_date: None,
        interest: Some(savings::AccountIn {
            kind: savings::Kind::Term,
            rate: s.deposit_rate.0,
            tax: s.deposit_rate.1,
            term_days: Some(32),
            start: opened,
            rate_from: None,
            renew: false,
            payout_account_id: Some(card),
        }),
    };
    accounts::create(&state.db, budget, &deposit).await?.map_err(|_| ApiError::status(500, "demo deposit"))?;

    // Rent, salary and bills come again next month on their own.
    let n = names_of(s);
    for (sub, parent, dollars, dom) in [(s.rent, n.home, 1100.0, 1), (s.salary, n.income, 2600.0, 5), (s.utilities, n.home, 120.0, 10)] {
        let category: Option<(i64, String)> = sqlx::query_as(
            "SELECT c.id, c.type FROM categories c JOIN categories p ON p.id = c.parent_id WHERE p.budget_id = $1 AND p.name = $2 AND c.name = $3 LIMIT 1",
        )
        .bind(budget).bind(parent).bind(sub)
        .fetch_optional(&state.db)
        .await?;
        let Some((category, kind)) = category else { continue };
        let month = if today.day() < dom { first_of_month(today, 0) } else { first_of_month(today, -1) };
        let next = month.with_day(dom).unwrap_or(month).to_string();
        sqlx::query(
            "INSERT INTO recurring (budget_id, user_id, category_id, type, amount, currency, every, start_date, next_date, account_id)
             VALUES ($1, $1, $2, $3, $4, $5, 'month', $6, $6, $7)",
        )
        .bind(budget).bind(category).bind(kind).bind(money(dollars)).bind(s.main).bind(next).bind(card)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

/// The first day of the month `back` months before `day`'s (negative: after).
fn first_of_month(day: NaiveDate, back: i32) -> NaiveDate {
    let months = day.year() * 12 + day.month0() as i32 - back;
    NaiveDate::from_ymd_opt(months.div_euclid(12), months.rem_euclid(12) as u32 + 1, 1).unwrap_or(day)
}

/// Deletes a demo budget with everything in it.
async fn remove(db: &sqlx::PgPool, id: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    // Categories aren't tied to their budget by a key; operations go with their category,
    // transfers, accounts, sessions and the rest with the person.
    sqlx::query("DELETE FROM categories WHERE budget_id = $1 AND parent_id IS NULL").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM categories WHERE budget_id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM users WHERE id = $1 AND telegram_id LIKE 'demo:%'").bind(id).execute(&mut *tx).await?;
    tx.commit().await
}

/// Demo budgets older than a day; the background worker calls it.
pub async fn expire(db: &sqlx::PgPool) -> sqlx::Result<()> {
    let old: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE telegram_id LIKE 'demo:%' AND created_at::timestamptz < now() - make_interval(hours => $1::int)",
    )
    .bind(LIFETIME_HOURS as i32)
    .fetch_all(db)
    .await?;
    for id in &old {
        remove(db, *id).await?;
    }
    if !old.is_empty() {
        tracing::info!(removed = old.len(), "old demo budgets removed");
    }
    Ok(())
}

/// In the demo, the requests that only real people need are refused, and imports are small.
pub async fn guard(State(state): State<std::sync::Arc<AppState>>, req: Request, next: Next) -> Response {
    if state.cfg.demo {
        let path = req.uri().path();
        let post = req.method() == Method::POST;
        let off = post && (matches!(path, "/api/setup" | "/api/auth/register" | "/api/auth/login" | "/api/auth/password" | "/api/auth/telegram" | "/api/invites" | "/api/users")
                || path.starts_with("/api/invite/"))
            || path.starts_with("/api/2fa/")
            || path.starts_with("/api/users/") && req.method() != Method::GET;
        if off {
            return ApiError::status(403, OFF).into_response();
        }
        let big = req.headers().get(axum::http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        if path == "/api/import" && big.is_none_or(|n| n > MAX_IMPORT_BYTES) {
            return (StatusCode::PAYLOAD_TOO_LARGE, "Файл слишком большой для демо").into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_back_and_forward() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap();
        assert_eq!(first_of_month(d, 2), NaiveDate::from_ymd_opt(2025, 11, 1).unwrap());
        assert_eq!(first_of_month(d, -1), NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        assert_eq!(first_of_month(NaiveDate::from_ymd_opt(2026, 12, 5).unwrap(), -1), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
    }

    #[test]
    fn a_story_in_every_language_balances() {
        let start = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        for (s, lang) in STORIES.iter().zip(db::LANGS) {
            assert_eq!(s.lang, lang);
            assert!(rows(s, start, today, &mut Dice(42)).iter().all(|r| r.amount >= s.step.min(1.0)), "{lang}: an amount rounds to nothing");
            let rows = rows(s, start, today, &mut Dice(42));
            assert!(rows.len() > 60, "{} rows", rows.len());
            assert!(rows.iter().all(|r| r.amount > 0.0 && r.date <= today));
        }
    }
}
