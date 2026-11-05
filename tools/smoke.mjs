// A walk through a fresh install, run by CI after it starts the app: node tools/smoke.mjs [base URL]
// It makes the owner with an English interface, then checks that the budget is named in English
// and that the home card's money matches the accounts: a starting balance, an exchange from
// dollars to euros, spending from the euro card and money put into savings.
const base = process.argv[2] || "http://127.0.0.1:8009";
let cookie = "";
let failed = false;

async function api(method, path, body) {
  const r = await fetch(base + path, {
    method,
    headers: { "Content-Type": "application/json", Origin: base, ...(cookie && { Cookie: cookie }) },
    body: body && JSON.stringify(body),
  });
  const set = r.headers.getSetCookie?.() || [];
  if (set.length) cookie = set.map(c => c.split(";")[0]).join("; ");
  const text = await r.text();
  // Writes are rate-limited; a person doesn't add five things in a second, this script does.
  if (r.status === 429) {
    await new Promise(done => setTimeout(done, 1000 * (Number(/(\d+)s/.exec(text)?.[1]) || 2)));
    return api(method, path, body);
  }
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${text}`);
  return text ? JSON.parse(text) : null;
}

function check(what, ok, got) {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${ok ? "" : ` (got ${JSON.stringify(got)})`}`);
  if (!ok) failed = true;
}
const near = (a, b) => Math.abs((a || 0) - (b || 0)) < 0.005;

const setup = await api("GET", "/api/setup");
check("a fresh install asks for its owner", setup.needs_setup === true, setup);
await api("POST", "/api/setup", { login: "ci-owner", password: "ci password", name: "CI", lang: "en" });

// The budget is in the language it was made in.
const month = new Date().toISOString().slice(0, 7) + "-01";
let home = await api("GET", `/api/home?month=${month}`);
const names = home.categories.map(c => c.name);
check("categories are in English", ["Groceries", "Income", "Savings"].every(n => names.includes(n)), names);
const accounts = await api("GET", "/api/accounts");
check("the first account is in English", accounts[0]?.name === "Main", accounts.map(a => a.name));

// Money: a dollar card with a starting balance, a euro card topped up from it, a savings account.
await api("PUT", "/api/settings", { main_currency: "$", rate_source: "off" });
const day = new Date().toISOString().slice(0, 10);
const usd = (await api("POST", "/api/accounts", { name: "Card", currency: "$", initial_amount: 1000 })).id;
const eur = (await api("POST", "/api/accounts", { name: "Euro card", currency: "€" })).id;
const box = (await api("POST", "/api/accounts", { name: "Box", currency: "$", kind: "savings" })).id;
await api("POST", "/api/accounts/transfers", { from_id: usd, to_id: eur, amount: 110, to_amount: 100, transaction_date: day });
await api("POST", "/api/accounts/transfers", { from_id: usd, to_id: box, amount: 200, transaction_date: day });
const groceries = home.categories.find(c => c.name === "Groceries").id;
await api("POST", `/api/categories/${groceries}/expenses`, { amount: 30, currency: "€", account_id: eur, transaction_date: day });
await api("POST", `/api/categories/${groceries}/expenses`, { amount: 50, currency: "$", account_id: usd, transaction_date: day });
await api("POST", "/api/income", { amount: 500, currency: "$", account_id: usd, transaction_date: day });

home = await api("GET", `/api/home?month=${month}`);
const sum = home.month_summary;
const onAccounts = {};
for (const a of await api("GET", "/api/accounts")) if (a.kind === "regular") onAccounts[a.currency] = (onAccounts[a.currency] || 0) + a.balance;
for (const cur of ["$", "€"]) {
  check(`left in ${cur} is the money on everyday accounts`, near(sum.balance[cur], onAccounts[cur]), { card: sum.balance[cur], accounts: onAccounts[cur] });
  const parts = (sum.carry_in[cur] || 0) + (sum.income[cur] || 0) - (sum.expense[cur] || 0) - (sum.saved[cur] || 0) + (sum.exchanged[cur] || 0) + (sum.opened[cur] || 0);
  check(`the card's lines add up in ${cur}`, near(parts, sum.balance[cur]), { parts, balance: sum.balance[cur] });
}
check("dollars: 1000 + 500 − 50 − 110 − 200", near(sum.balance["$"], 1140), sum.balance["$"]);
check("euros: 100 − 30", near(sum.balance["€"], 70), sum.balance["€"]);
check("the exchange is shown", near(sum.exchanged["$"], -110) && near(sum.exchanged["€"], 100), sum.exchanged);

process.exit(failed ? 1 : 0);
