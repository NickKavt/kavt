// A category: add an operation, see the month's operations by day, tap one to change it.

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText, locale } from "../i18n.js";
import { money, usedCurrencies, ymd, parseYmd, today, sameMonth, dayTitle, weekday, monthStart, addMonths } from "../format.js";
import { useStore, openSheet, toast, state } from "../store.js";
import { loadDetails, addOperation, refresh, setMonth, withdraw, createRecurring } from "../data.js";
import { evalAmount } from "../calc.js";
import { haptic, tell, tg } from "../tg.js";
import { back } from "../router.js";
import { MoneyField, DateField, Chips, Seg, Empty, lastCurrency, rememberCurrency } from "../ui.js";
import { MonthBar } from "./home.js";
import { AccountPicker, everydayAccounts, defaultAccount, rememberAccount, accountLabel, BeforeStart } from "./accounts.js";

/** Marks the savings starting balance; the server leaves it out of month balances. */
export const INITIAL_NOTE = "Первоначальная сумма";
import { useSwipe } from "./swipe.js";
import { EVERY } from "./tools.js";

/** Date a new operation gets: today in the current month, else the month's first day. */
export function defaultDate(month) {
  return sameMonth(month, today()) ? ymd(today()) : ymd(monthStart(month));
}

const subKey = id => `budget:v2:sub:${id}`;
function lastSub(id) {
  try {
    const v = localStorage.getItem(subKey(id));
    return v === null || v === "" ? null : Number(v);
  } catch {
    return null;
  }
}

/** Form for a new operation; also used by the quick-add sheet on the home screen. */
export function AddForm({ category, details, onSaved, autofocus }) {
  const s = useStore();
  const [amount, setAmount] = useState("");
  const accounts = everydayAccounts(s);
  const [account, setAccount] = useState(() => defaultAccount(accounts, null, s.me?.id)?.id ?? null);
  const [currency, setCurrency] = useState(() => accounts.find(a => a.id === account)?.currency || lastCurrency());
  const [note, setNote] = useState("");
  const [date, setDate] = useState(defaultDate(s.month));
  // The account sets the currency; a currency picked by hand moves to an account in it.
  const pickAccount = id => {
    setAccount(id);
    const a = accounts.find(x => x.id === id);
    if (a) setCurrency(a.currency);
  };
  const pickCurrency = c => {
    setCurrency(c);
    if (accounts.find(x => x.id === account)?.currency !== c) setAccount(defaultAccount(accounts, c, s.me?.id)?.id ?? null);
  };
  const subs = details?.subcategories || [];
  const remembered = lastSub(category.id);
  const [sub, setSub] = useState(subs.some(x => x.id === remembered) ? remembered : null);
  const [busy, setBusy] = useState(false);
  // Savings can be added to or taken out of.
  const [mode, setMode] = useState("add");
  const out = category.type === "savings" && mode === "out";
  // "" or week, month, year: the operation then repeats from its date (src/recurring.rs).
  const [repeat, setRepeat] = useState("");
  // The month on screen changed: a new operation goes to that month by default.
  useEffect(() => setDate(defaultDate(s.month)), [ymd(s.month)]);
  useEffect(() => {
    if (sub !== null && !subs.some(x => x.id === sub)) setSub(null);
  }, [subs.map(x => x.id).join()]);
  // Another category picked in the same form (quick add): its own last subcategory.
  useEffect(() => {
    const r = lastSub(category.id);
    setSub(subs.some(x => x.id === r) ? r : null);
  }, [category.id, subs.length > 0]);

  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) {
      haptic("error");
      return tell(t("form.enterAmount"));
    }
    setBusy(true);
    try {
      const op = { category, subcategoryId: sub, amount: value, currency, note: note.trim(), date, accountId: account };
      if (repeat && !out) await createRecurring({ ...op, categoryId: sub ?? category.id, every: repeat, start: date });
      else await (out ? withdraw : addOperation)(op);
      haptic("success");
      rememberCurrency(currency);
      rememberAccount(account);
      try {
        localStorage.setItem(subKey(category.id), sub ?? "");
      } catch {}
      setAmount("");
      setNote("");
      const d = parseYmd(date);
      const saved = sameMonth(d, s.month) ? t("form.saved") : t("form.savedOn", { date: dayTitle(d) });
      toast(repeat && !out ? `${saved} · ${t("every." + repeat)}` : saved);
      setRepeat("");
      await refresh();
      onSaved?.();
    } catch (e) {
      haptic("error");
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const label = category.type === "income" ? t("form.addIncome") : out ? t("form.withdraw") : category.type === "savings" ? t("form.addSavings") : t("form.addExpense");
  return html`<div class="card form">
    ${category.type === "savings" && html`<${Seg} options=${[["add", t("form.deposit")], ["out", t("form.withdrawShort")]]} value=${mode} onChange=${setMode} />`}
    ${subs.length > 0 && html`<${Chips}
      items=${[[null, t("form.noSubcategory")], ...subs.map(x => [x.id, x.name])]} value=${sub} onChange=${setSub} />`}
    <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${pickCurrency} autofocus=${autofocus} onEnter=${save} />
    <${AccountPicker} accounts=${accounts} value=${account} onChange=${pickAccount}
      label=${category.type === "income" ? t("accounts.toAccount") : out ? t("accounts.toAccount") : category.type === "savings" ? t("accounts.fromAccount") : ""} />
    <input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />
    <${DateField} value=${date} onChange=${setDate} />
    <${BeforeStart} accounts=${[accounts.find(a => a.id === account)]} date=${date} />
    ${!out && html`<label class="date-row repeat-row">
      <span class="date-label">🔁 ${t("form.repeat")}</span>
      <select class="field" value=${repeat} onChange=${e => setRepeat(e.currentTarget.value)}>
        <option value="">${t("form.noRepeat")}</option>
        ${EVERY.map(x => html`<option value=${x}>${t("every." + x)}</option>`)}
      </select>
    </label>`}
    <button type="button" class="btn btn-primary" disabled=${busy} onClick=${save}>${label}</button>
  </div>`;
}

function Breakdown({ details, currency }) {
  const groups = new Map();
  for (const op of details.transactions) {
    if (op.currency !== currency) continue;
    const name = op.subcategory_name && op.subcategory_name !== details.name ? op.subcategory_name : t("form.noSubcategory");
    groups.set(name, (groups.get(name) || 0) + signed(op));
  }
  // Shares only make sense for what is there: a subcategory emptied by withdrawals drops out.
  const rows = [...groups.entries()].filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1]);
  if (rows.length < 2) return null;
  const sum = rows.reduce((a, [, v]) => a + v, 0);
  return html`<div class="card">
    <div class="card-title">${t("cat.bySubcategory", { cur: currency })}</div>
    ${rows.map(([name, v]) => html`<div class="bar-row">
      <div class="bar-meta"><span>${name}</span><span class="muted">${money(v, currency)} · ${Math.round((v / sum) * 100)}%</span></div>
      <div class="bar"><span style=${`width:${Math.max(3, (v / rows[0][1]) * 100)}%;background:${details.color}`}></span></div>
    </div>`)}
  </div>`;
}

/** An operation's amount as it counts: a withdrawal from savings is negative. */
export function signed(op) {
  return op.source === "withdrawal" ? -op.amount : op.amount;
}

/** An operation in another currency, in the budget's main one at the rate of its own day. */
export function converted(op) {
  // Someone who turned conversion off in Settings sees amounts in their own currency only.
  if (state.me?.show_converted === false) return "";
  return op.rate && op.rate_to && op.rate_to !== op.currency ? money(op.amount * op.rate, op.rate_to) : "";
}

export function OperationList({ ops, onOpen, showCategory, fallbackTitle = "" }) {
  const accounts = useStore().home?.accounts || [];
  if (!ops.length) return html`<${Empty}>${t("cat.noOperations")}<//>`;
  // Which account paid is worth showing only when there is more than one.
  const accountOf = op => (accounts.length > 1 ? accounts.find(a => a.id === op.account_id) : null);
  const days = new Map();
  for (const op of ops) {
    if (!days.has(op.transaction_date)) days.set(op.transaction_date, []);
    days.get(op.transaction_date).push(op);
  }
  return html`<div class="ops">
    ${[...days.entries()].map(([day, list]) => {
      const d = parseYmd(day);
      const diff = Math.round((today() - d) / 86400000);
      const title = diff === 0 ? t("common.today") : diff === 1 ? t("common.yesterday") : `${dayTitle(d)}, ${weekday(d)}`;
      return html`<div class="ops-day">${title}</div>
        ${list.map(op => {
          const initial = op.type === "savings" && String(op.note || "").startsWith(INITIAL_NOTE);
          const where = showCategory ? `${op.category_emoji} ${op.subcategory_name || op.category_name}` : op.subcategory_name;
          // Without a subcategory the description is the clearest title.
          const taken = op.source === "withdrawal";
          // A withdrawal or the half of a transfer says so under the account's name.
          const kindLabel = op.pair_id ? (taken ? t("transfer.out") : t("transfer.in")) : taken ? t("savings.withdrawal") : "";
          const title = initial ? t("savings.start") : where || op.note || kindLabel || fallbackTitle;
          const note = initial ? "" : [kindLabel !== title && kindLabel, op.note !== title && op.note].filter(Boolean).join(" · ");
          return html`<button type="button" class="op" onClick=${() => onOpen?.(op)}>
          <span class="op-main">
            <span class="op-title">${title}</span>
            ${note && html`<span class="op-note">${note}</span>`}
            ${(op.actor_name || accountOf(op)) && html`<span class="op-who">${[accountOf(op) && accountLabel(accountOf(op)), op.actor_name].filter(Boolean).join(" · ")}</span>`}
          </span>
          <span class="op-amounts">
            <span class=${"op-amount op-" + (taken ? "withdrawal" : op.type)}>${op.type === "expense" || taken ? "−" : "+"}${money(op.amount, op.currency)}</span>
            ${converted(op) && html`<span class="op-conv">≈ ${converted(op)}</span>`}
          </span>
        </button>`;
        })}`;
    })}
  </div>`;
}

/** A percent as people write it: 17,5 rather than 17.500. */
export function pct(v) {
  return Number(v || 0).toLocaleString(locale(), { maximumFractionDigits: 3 });
}

function shortDate(s) {
  return parseYmd(s).toLocaleDateString(locale(), { day: "numeric", month: "long" });
}

export function Category({ id }) {
  const s = useStore();
  const details = s.details[id];
  const category = s.home?.categories.find(c => c.id === id) || details;
  const swipe = useSwipe(n => setMonth(addMonths(s.month, n)));
  const [error, setError] = useState(null);
  useEffect(() => {
    if (!details) loadDetails(id).catch(e => setError(errorText(e)));
  }, [id, ymd(s.month)]);
  if (error && !details) return html`<${Empty}>${error}<//>`;
  if (!details || !category) return html`<${Empty}>${t("common.loading")}<//>`;

  const totals = category.type === "income" ? s.home?.month_summary?.income : details.totals_by_currency;
  const used = usedCurrencies(totals);
  const opsCurrencies = usedCurrencies(details.transactions.reduce((m, o) => ((m[o.currency] = (m[o.currency] || 0) + signed(o)), m), {}));
  const ops = details.transactions.map(o => ({ ...o, subcategory_name: o.subcategory_name === details.name ? null : o.subcategory_name }));

  return html`<div ...${swipe}>
    <div class="cat-head" style=${`--cat:${details.color}`}>
      ${!tg?.initData && html`<button type="button" class="icon-btn cat-back" onClick=${back} aria-label=${t("nav.back")}>‹</button>`}
      <span class="cat-emoji">${details.emoji}</span>
      <span class="cat-head-name">${details.name}</span>
      ${category.type !== "savings" && html`<span class="cat-amounts">${(used.length ? used : [lastCurrency()]).map(c => html`<span>${money(totals?.[c] || 0, c)}</span>`)}</span>`}
    </div>
    <${MonthBar} />
    <${AddForm} category=${category} details=${details} />
    ${opsCurrencies.map(c => html`<${Breakdown} details=${details} currency=${c} />`)}
    <div class="section-title">${t("cat.operations", { n: ops.length })}</div>
    <${OperationList} ops=${ops} fallbackTitle=${details.name} onOpen=${op => openSheet("editOperation", { op, categoryId: id })} />
  </div>`;
}
