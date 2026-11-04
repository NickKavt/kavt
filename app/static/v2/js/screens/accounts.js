// Accounts: where the money is (a card, cash), each in one currency, shared or someone's own.
// Income and expenses are paid from one; transfers move money between them, also between
// currencies. Savings accounts with interest are edited where they were (sheets.js) and
// listed here too.

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText } from "../i18n.js";
import { money, ymd, today, dayTitle, parseYmd } from "../format.js";
import { state, useStore, openSheet, closeSheet, toast } from "../store.js";
import * as data from "../data.js";
import { api } from "../api.js";
import { evalAmount } from "../calc.js";
import { haptic, ask, tell } from "../tg.js";
import { Sheet, Seg, Chips, MoneyField, Row, Empty, CurrencySelect } from "../ui.js";

const KEY = "budget:v2:account";
const EMOJIS = ["💳", "💵", "🏦", "👛", "💰", "🪙", "📱", "🏠"];

export const activeAccounts = s => (s.home?.accounts || []).filter(a => !a.archived);
/** Cards and cash: where everyday income and expenses go. */
export const everydayAccounts = s => activeAccounts(s).filter(a => a.kind !== "savings");
export const savingsAccounts = s => activeAccounts(s).filter(a => a.kind === "savings");
const TERMS = [32, 91, 181, 365];
/**
 * The tax Türkiye withholds from lira deposit interest (stopaj, 2026): 17.5% up to six months
 * and on flexible accounts, 15% up to a year, 10% beyond. Other currencies: none suggested.
 */
export function suggestedTax(currency, kind, days) {
  if (currency !== "₺") return null;
  if (kind !== "term" || !(days > 0) || days <= 182) return 17.5;
  return days <= 365 ? 15 : 10;
}
const decimal = text => Number(String(text).trim().replace(/\s/g, "").replace(",", "."));
const pctText = v => (v || v === 0 ? String(v).replace(".", ",") : "");
export const accountLabel = a => `${a.emoji} ${a.name}${a.owner_name ? ` · ${a.owner_name}` : ""}`;
/** The label with the currency, unless the name already says it ("Основной ₺"). */
export const accountChoice = a => (a.name.includes(a.currency) ? accountLabel(a) : `${accountLabel(a)} · ${a.currency}`);

function remembered() {
  try {
    return Number(localStorage.getItem(KEY)) || null;
  } catch {
    return null;
  }
}

export function rememberAccount(id) {
  try {
    localStorage.setItem(KEY, String(id ?? ""));
  } catch {}
}

/** The account a new operation starts with: the last one used, else the person's own, else shared. */
/**
 * The account an operation moves to when its currency is changed by hand: one of the same kind
 * as the account it was on (an expense stays off savings, interest stays on them).
 */
export function accountForCurrency(s, currentId, currency) {
  const current = activeAccounts(s).find(a => a.id === currentId);
  const pool = current?.kind === "savings" ? savingsAccounts(s) : everydayAccounts(s);
  return defaultAccount(pool, currency, s.me?.id)?.id ?? null;
}

export function defaultAccount(accounts, currency, meId) {
  const last = accounts.find(a => a.id === remembered());
  if (last && (!currency || last.currency === currency)) return last;
  const fit = accounts.filter(a => !currency || a.currency === currency);
  return fit.find(a => a.owner_id === meId) || fit.find(a => a.owner_id == null) || fit[0] || null;
}

/**
 * A warning when an operation's day is before an account's starting balance: that account's
 * balance doesn't count it (the starting balance already includes what happened before).
 */
export function BeforeStart({ accounts, date }) {
  const early = accounts.filter(a => a?.initial_date && date < a.initial_date);
  if (!early.length) return null;
  return html`<div class="warn small">${early.map(a => t("accounts.beforeStart", { name: a.name, date: dayTitle(parseYmd(a.initial_date)) })).join(" ")}</div>`;
}

/**
 * A field showing the chosen account with its balance; tapping it opens the list of accounts
 * grouped as mine, shared and other people's, each with its balance and currency.
 */
export function AccountSelect({ accounts, value, onChange, label, bad, inside }) {
  const me = useStore().me?.id;
  const [open, setOpen] = useState(false);
  const cur = accounts.find(a => a.id === value);
  const others = [...new Set(accounts.filter(a => a.owner_id != null && a.owner_id !== me).map(a => a.owner_id))];
  const groups = [
    [t("accounts.mine"), accounts.filter(a => a.owner_id === me)],
    [t("accounts.sharedGroup"), accounts.filter(a => a.owner_id == null)],
    ...others.map(id => [accounts.find(a => a.owner_id === id).owner_name || "?", accounts.filter(a => a.owner_id === id)]),
  ].filter(([, list]) => list.length);
  const pick = id => {
    haptic();
    onChange(id);
    setOpen(false);
  };
  // `inside`: the label is a caption in the field itself (the transfer's from/to block).
  return html`<div class="account-select">
    ${label && !inside && html`<div class=${"muted small" + (bad ? " bad-label" : "")}>${label}</div>`}
    <button type="button" class=${"field account-field" + (inside ? " af-inside" : "") + (bad ? " field-bad" : "")} onClick=${() => { haptic(); setOpen(true); }}>
      ${inside && html`<span class=${"af-caption" + (bad ? " bad-label" : "")}>${label}</span>`}
      ${cur
        ? html`<span class="af-main">${cur.emoji} ${cur.name}${cur.owner_name ? html`<span class="muted"> · ${cur.owner_name}</span>` : ""}</span>
          <span class=${"af-balance" + (cur.balance < 0 ? " neg" : "")}>${money(cur.balance, cur.currency)}</span>`
        : html`<span class="af-main muted">${t("accounts.choose")}</span>`}
      <span class="af-arrow">›</span>
    </button>
    ${open && html`<div class="picker-backdrop" onClick=${() => setOpen(false)}>
      <div class="picker" onClick=${e => e.stopPropagation()}>
        <div class="picker-title">${label || t("accounts.choose")}</div>
        <div class="picker-list">
          ${groups.map(([name, list]) => html`<div class="picker-group">${name}</div>
            ${list.map(a => html`<button type="button" class=${"picker-row" + (a.id === value ? " active" : "")} onClick=${() => pick(a.id)}>
              <span class="pr-icon">${a.emoji}</span>
              <span class="pr-text"><span class="pr-name">${a.name}</span><span class="muted small">${a.currency}</span></span>
              <span class=${"pr-balance" + (a.balance < 0 ? " neg" : "")}>${money(a.balance, a.currency)}</span>
              <span class="pr-check">${a.id === value ? "✓" : ""}</span>
            </button>`)}`)}
        </div>
      </div>
    </div>`}
  </div>`;
}

/** Account choice in forms; hidden while the budget has a single account. */
export function AccountPicker({ accounts, value, onChange, label }) {
  if (accounts.length < 2) return null;
  return html`<${AccountSelect} accounts=${accounts} value=${value} onChange=${onChange} label=${label || t("accounts.account")} />`;
}

/** Accounts with their balances on the home screen. */
export function AccountsCard() {
  const s = useStore();
  const list = everydayAccounts(s);
  if (!list.length) return null;
  return html`<div class="group cat-group accounts-card">
    <div class="card-head accounts-head">
      <span class="card-title">${t("accounts.title")}</span>
      ${activeAccounts(s).length > 1 && html`<button type="button" class="chip" onClick=${() => { haptic(); openSheet("moneyTransfer", {}); }}>⇄ ${t("accounts.transfer")}</button>`}
    </div>
    ${list.map(a => html`<${Row} key=${a.id} icon=${a.emoji} title=${a.name} subtitle=${a.owner_name || ""}
      right=${html`<b class=${a.balance < 0 ? "neg" : ""}>${money(a.balance, a.currency)}</b>`} onClick=${() => openSheet("accountView", { id: a.id })} />`)}
  </div>`;
}

/**
 * Savings on the home screen: the total against the budget's goal, then each savings account
 * with its balance, its own goal and interest. "Put aside" is a transfer into one of them.
 */
export function SavingsCard() {
  const s = useStore();
  const list = savingsAccounts(s);
  const sum = s.home?.savings_summary;
  const isAdmin = s.me?.role === "admin" || s.me?.role === "owner";
  const goal = sum?.target_amount > 0;
  const others = Object.entries(sum?.saved_by_currency || {}).filter(([c, v]) => c !== sum?.currency && Math.abs(v) > 0.004);
  return html`<div class="group cat-group savings-card">
    <div class="card-head accounts-head">
      <span class="card-title">🎯 ${t("kind.savings")}</span>
      <button type="button" class="chip" onClick=${() => { haptic(); openSheet("moneyTransfer", { toSavings: true }); }}>＋ ${t("savings.putAside")}</button>
    </div>
    <div class="savings-total">
      <b>${money(sum?.saved_amount || 0, sum?.currency || "")}</b>${others.map(([c, v]) => html`<span class="muted"> · ${money(v, c)}</span>`)}
    </div>
    ${goal && html`<div class="savings-goal">
      <div class="bar bar-big"><span style=${`width:${Math.min(100, sum.progress_percent)}%`}></span></div>
      <div class="muted small">${t("savings.goalLine", { goal: money(sum.target_amount, sum.currency), left: money(sum.remaining_amount, sum.currency), pct: Math.round(sum.progress_percent) })}</div>
    </div>`}
    ${isAdmin && html`<button type="button" class="link-btn savings-goal-btn" onClick=${() => openSheet("savingsGoal", {})}>🎯 ${goal ? t("savings.changeGoal") : t("savings.setGoal")}</button>`}
    ${list.map(a => html`<button type="button" key=${a.id} class="row savings-row" onClick=${() => openSheet("accountView", { id: a.id })}>
      <span class="row-icon">${a.emoji}</span>
      <span class="row-text">
        <span class="row-title">${a.name}</span>
        <span class="row-sub">${[a.owner_name, a.interest && t("savings.rateShort", { rate: pctText(a.interest.rate) }), a.interest?.accrued > 0 && t("savings.accruedShort", { sum: money(a.interest.accrued, a.currency) })].filter(Boolean).join(" · ")}</span>
        ${a.target_amount > 0 && html`<span class="bar"><span style=${`width:${Math.min(100, Math.max(0, (a.balance / a.target_amount) * 100))}%`}></span></span>`}
      </span>
      <span class="row-right"><b>${money(a.balance, a.currency)}</b>${a.target_amount > 0 && html`<span class="muted small"> / ${money(a.target_amount, a.currency)}</span>`}</span>
    </button>`)}
    ${!list.length && html`<${Row} icon="＋" title=${t("savings.addAccount")} tone="accent" onClick=${() => openSheet("moneyAccount", { kind: "savings" })} />`}
  </div>`;
}

/** The budget's savings goal, in its main currency. */
function SavingsGoalSheet() {
  const s = useStore();
  const sum = s.home?.savings_summary;
  const [amount, setAmount] = useState(sum?.target_amount > 0 ? String(sum.target_amount).replace(".", ",") : "");
  const save = async value => {
    if (await attempt(() => data.saveSavingsGoal(value), t("savings.goalSaved"))) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${t("savings.goalTitle")}>
    <div class="muted small">${t("savings.goalHint")}</div>
    <div class="form">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${sum?.currency || ""} onCurrency=${() => {}} locked autofocus onEnter=${() => save(evalAmount(amount))} />
      <button type="button" class="btn btn-primary" onClick=${() => { const v = evalAmount(amount); if (!(v > 0)) return tell(t("form.enterAmount")); save(v); }}>${t("common.save")}</button>
      ${sum?.target_amount > 0 && html`<button type="button" class="btn btn-danger" onClick=${() => save(null)}>${t("savings.removeGoal")}</button>`}
    </div>
  <//>`;
}

/** Every account, savings ones too, from Manage. */
function MoneyAccounts() {
  const s = useStore();
  const all = s.home?.accounts || [];
  const savings = savingsAccounts(s);
  const archived = all.filter(a => a.archived);
  return html`<${Sheet} title=${t("accounts.title")}>
    <div class="muted small">${t("accounts.hint")}</div>
    <div class="group">
      ${everydayAccounts(s).map(a => html`<${Row} key=${a.id} icon=${a.emoji} title=${a.name}
        subtitle=${[a.owner_name || t("accounts.shared"), a.currency].join(" · ")}
        right=${money(a.balance, a.currency)} onClick=${() => openSheet("accountView", { id: a.id })} />`)}
      <${Row} icon="＋" title=${t("accounts.add")} tone="accent" onClick=${() => openSheet("moneyAccount", {})} />
    </div>
    <div class="section-title">${t("accounts.savings")}</div>
    <div class="group">
      ${savings.map(a => html`<${Row} key=${a.id} icon=${a.emoji} title=${a.name}
        subtitle=${[a.owner_name || t("accounts.shared"), a.currency, a.interest && t("savings.rateShort", { rate: pctText(a.interest.rate) })].filter(Boolean).join(" · ")}
        right=${money(a.balance, a.currency)} onClick=${() => openSheet("accountView", { id: a.id })} />`)}
      <${Row} icon="＋" title=${t("savings.addAccount")} tone="accent" onClick=${() => openSheet("moneyAccount", { kind: "savings" })} />
    </div>
    ${archived.length > 0 && html`<div class="section-title">${t("accounts.archived")}</div>
    <div class="group">
      ${archived.map(a => html`<${Row} key=${a.id} icon=${a.emoji} title=${a.name} subtitle=${a.currency} tone="muted"
        right=${money(a.balance, a.currency)} onClick=${() => openSheet("moneyAccount", { account: a })} />`)}
    </div>`}
  <//>`;
}

async function attempt(fn, done) {
  try {
    const r = await fn();
    haptic("success");
    if (done) toast(done);
    return r ?? true;
  } catch (e) {
    haptic("error");
    toast(errorText(e), "error");
    return false;
  }
}

/** A new account or an existing one: name, icon, currency, whose, and the starting balance. */
function MoneyAccount({ account, kind: startKind }) {
  const s = useStore();
  const editing = !!account;
  const [kind, setKind] = useState(account?.kind || startKind || "regular");
  const savings = kind === "savings";
  const [target, setTarget] = useState(account?.target_amount > 0 ? String(account.target_amount).replace(".", ",") : "");
  // Interest of a savings account (src/savings.rs).
  const i = account?.interest;
  const [earns, setEarns] = useState(!!i);
  const [ikind, setIkind] = useState(i?.kind || "flexible");
  const [rate, setRate] = useState(i ? pctText(i.rate) : "");
  const [tax, setTaxText] = useState(i?.tax ? pctText(i.tax) : "");
  // Typed by hand (or already set): no longer follows the suggestion for the term.
  const [taxTouched, setTaxTouched] = useState(!!i);
  const setTax = v => {
    setTaxTouched(true);
    setTaxText(v);
  };
  const [days, setDays] = useState(String(i?.term_days || 32));
  const [renew, setRenew] = useState(i ? i.renew !== false : true);
  const [payoutId, setPayoutId] = useState(i?.payout_account_id ?? null);
  const [istart, setIstart] = useState(i?.start || account?.initial_date || ymd(today()));
  const [rateFrom, setRateFrom] = useState(ymd(today()));
  const rateChanged = !!i && (decimal(rate) !== i.rate || (tax.trim() ? decimal(tax) : 0) !== i.tax);
  const [name, setName] = useState(account?.name || "");
  const [emoji, setEmoji] = useState(account?.emoji || (startKind === "savings" ? "🎯" : "💳"));
  const [currency, setCurrency] = useState(account?.currency || s.home?.settings?.main_currency || "₺");
  const [owner, setOwner] = useState(editing ? account.owner_id ?? null : s.me?.id ?? null);
  const [amount, setAmount] = useState(account?.initial_amount ? String(account.initial_amount).replace(".", ",") : "");
  const [from, setFrom] = useState(account?.initial_date || (editing ? "" : ymd(today())));
  const [people, setPeople] = useState(s.me ? [s.me] : []);
  const isAdmin = s.me?.role === "admin" || s.me?.role === "owner";
  useEffect(() => {
    // Only admins see the list of people; others can make shared accounts or their own.
    if (isAdmin) data.loadUsers().then(setPeople).catch(() => {});
  }, []);
  const owners = [[null, t("accounts.shared")], ...people.map(p => [p.id, [p.first_name, p.last_name].filter(Boolean).join(" ") || p.username || p.login || "?"])];
  if (editing && account.owner_id != null && !owners.some(([id]) => id === account.owner_id)) owners.push([account.owner_id, account.owner_name || "?"]);
  const locked = editing && account.used > 0;
  // A new account's money: already there ("start") or moved from another account ("transfer").
  const [origin, setOrigin] = useState("start");
  const sources = activeAccounts(s).filter(a => a.currency === currency);
  const [sourceId, setSourceId] = useState(null);
  const source = sources.find(a => a.id === sourceId) || defaultAccount(sources, currency, s.me?.id);
  const moving = !editing && origin === "transfer";
  const [triedSource, setTriedSource] = useState(false);
  const suggested = suggestedTax(currency, ikind, Math.round(decimal(days)));
  useEffect(() => {
    if (!taxTouched) setTaxText(suggested == null ? "" : pctText(suggested));
  }, [suggested, taxTouched]);
  // Where an ended deposit's money goes: another account of its currency.
  const payouts = activeAccounts(s).filter(a => a.currency === currency && a.id !== account?.id);
  const payout = payouts.find(a => a.id === payoutId) || payoutDefault(payouts, s.me?.id);

  const body = archived => {
    const v = amount.trim() ? evalAmount(amount) : 0;
    if (!Number.isFinite(v)) return null;
    const goal = target.trim() ? evalAmount(target) : null;
    let interest = null;
    if (savings && earns) {
      const termDays = Math.round(decimal(days));
      const ends = ikind === "term" && !renew;
      interest = {
        kind: ikind, rate: decimal(rate), tax: tax.trim() ? decimal(tax) : 0, term_days: ikind === "term" ? termDays : null,
        start: istart, rate_from: rateChanged ? rateFrom : null, renew: !ends, payout_account_id: ends ? payout?.id ?? null : null,
      };
    }
    return { name: name.trim(), emoji, currency, owner_id: owner, initial_amount: v, initial_date: from || null, archived, kind, target_amount: savings && goal > 0 ? goal : null, interest };
  };
  const save = async () => {
    if (!name.trim()) return tell(t("err.emptyName"));
    const b = body(account?.archived || false);
    if (!b) return tell(t("form.enterAmount"));
    if (b.interest) {
      if (!(b.interest.rate >= 0)) return tell(t("acc.enterRate"));
      if (!(b.interest.tax >= 0 && b.interest.tax < 100)) return tell(t("acc.enterTax"));
      if (ikind === "term" && !(b.interest.term_days >= 1)) return tell(t("acc.enterDays"));
    }
    if (moving) {
      setTriedSource(true);
      if (!source) {
        haptic("error");
        return;
      }
      if (!(b.initial_amount > 0)) return tell(t("form.enterAmount"));
      // The account starts empty; the money arrives by a transfer, so it leaves the other account.
      const made = await attempt(() => data.createMoneyAccount({ ...b, initial_amount: 0, initial_date: null }));
      if (!made) return;
      const ok = await attempt(() => data.accountTransfer({ fromId: source.id, toId: made.id, amount: b.initial_amount, note: "", date: from || ymd(today()) }), t("acc.created"));
      if (ok) {
        closeSheet();
        data.refresh();
      }
      return;
    }
    const ok = await attempt(() => (editing ? data.updateMoneyAccount(account.id, b) : data.createMoneyAccount(b)), editing ? t("acc.saved") : t("acc.created"));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  const restore = async () => {
    if (await attempt(() => data.updateMoneyAccount(account.id, body(false)), t("acc.saved"))) {
      closeSheet();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(account.used > 0 ? t("accounts.confirmArchive", { name: account.name }) : t("accounts.confirmDelete", { name: account.name })))) return;
    const r = await attempt(() => data.deleteMoneyAccount(account.id));
    if (r) {
      toast(r.result === "archived" ? t("accounts.archivedDone") : t("accounts.deleted"));
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${editing ? account.name : t("accounts.new")}>
    <div class="form">
      <${Seg} options=${[["regular", t("accounts.kindRegular")], ["savings", t("accounts.kindSavings")]]} value=${kind} onChange=${setKind} />
      <input class="field" placeholder=${savings ? t("savings.namePlaceholder") : t("accounts.namePlaceholder")} value=${name} onInput=${e => setName(e.currentTarget.value)} maxlength="40" autofocus=${!editing} />
      <${Chips} items=${EMOJIS.map(e => [e, e])} value=${emoji} onChange=${setEmoji} />
      <div class="muted small">${t("form.currency")}</div>
      ${locked
        ? html`<div class="muted small">${currency} · ${t("accounts.currencyLocked")}</div>`
        : html`<${CurrencySelect} value=${currency} onChange=${setCurrency} />`}
      <div class="muted small">${t("accounts.whose")}</div>
      <${Chips} items=${owners} value=${owner} onChange=${setOwner} />
      ${!editing && html`<${Seg} options=${[["start", t("accounts.originStart")], ["transfer", t("accounts.originTransfer")]]} value=${origin} onChange=${setOrigin} />`}
      ${moving
        ? html`<div class="muted small">${t("transfer.from")}</div>
          ${sources.length
            ? html`<${AccountSelect} accounts=${sources} value=${source?.id} onChange=${setSourceId} />`
            : html`<div class=${triedSource ? "error-note small" : "warn small"}>${triedSource ? "⚠️ " : ""}${t("accounts.noSource")}</div>`}
          <div class="muted small">${t("accounts.moveAmount")}</div>`
        : html`<div class="muted small">${t("accounts.start")}</div>`}
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${() => {}} locked onEnter=${save} />
      <label class="date-row">
        <span class="date-label">📅 ${t("accounts.startFrom")}</span>
        <input type="date" class="field date-input" value=${from} max=${ymd(today())} onChange=${e => setFrom(e.currentTarget.value)} />
        ${from && !moving && html`<button type="button" class="link-btn" onClick=${e => { e.preventDefault(); setFrom(""); }}>${t("accounts.fromStart")}</button>`}
      </label>
      <div class="muted small">${moving ? t("accounts.moveHint") : from ? t("accounts.startHint", { date: dayTitle(parseYmd(from)) }) : t("accounts.startHintAll")}</div>
      ${savings && html`<div class="muted small">${t("savings.accountGoal")}</div>
        <${MoneyField} value=${target} onInput=${setTarget} currency=${currency} onCurrency=${() => {}} locked />
        <label class="date-row switch-row" onClick=${() => { haptic(); setEarns(!earns); }}>
          <span class="date-label">💹 ${t("savings.earns")}</span>
          <span class=${"switch" + (earns ? " on" : "")}></span>
        </label>
        ${earns && html`<div class="form interest-form">
          <${Seg} options=${[["flexible", t("acc.flexible")], ["term", t("acc.term")]]} value=${ikind} onChange=${setIkind} />
          <div class="muted small">${ikind === "term" ? t("acc.termHint") : t("acc.flexibleHint")}</div>
          <div class="acc-fields">
            <label class="acc-field">
              <span class="muted">${t("acc.rate")}</span>
              <span class="acc-input"><input class="field" inputmode="decimal" value=${rate} onInput=${e => setRate(e.currentTarget.value)} placeholder="45" /><span>%</span></span>
            </label>
            <label class="acc-field">
              <span class="muted">${t("acc.tax")}</span>
              <span class="acc-input"><input class="field" inputmode="decimal" value=${tax} onInput=${e => setTax(e.currentTarget.value)} placeholder="0" /><span>%</span></span>
            </label>
          </div>
          ${ikind === "term" && html`<label class="acc-field">
            <span class="muted">${t("acc.days")}</span>
            <input class="field" inputmode="numeric" value=${days} onInput=${e => setDays(e.currentTarget.value)} />
          </label>
          <${Chips} items=${TERMS.map(n => [String(n), t("acc.daysShort", { n })])} value=${days} onChange=${setDays} />
          <${Seg} options=${[[true, t("acc.renew")], [false, t("acc.noRenew")]]} value=${renew} onChange=${setRenew} />
          <div class="muted small">${renew ? t("acc.renewHint") : t("acc.noRenewHint")}</div>
          ${!renew && (payouts.length
            ? html`<${AccountSelect} accounts=${payouts} value=${payout?.id} onChange=${setPayoutId} />`
            : html`<div class="warn small">${t("acc.noPayout")}</div>`)}`}
          ${suggested != null && html`<div class="muted small">${t("acc.taxHint")}</div>`}
          <label class="date-row">
            <span class="date-label">${t("acc.start")}</span>
            <input type="date" class="field date-input" value=${istart} max=${ymd(today())} onChange=${e => e.currentTarget.value && setIstart(e.currentTarget.value)} />
          </label>
          ${rateChanged && html`<label class="date-row">
            <span class="date-label">${t("acc.rateFrom")}</span>
            <input type="date" class="field date-input" value=${rateFrom} min=${istart} max=${ymd(today())} onChange=${e => e.currentTarget.value && setRateFrom(e.currentTarget.value)} />
          </label>`}
          <div class="muted small">${t("savings.interestHint")}</div>
        </div>`}`}
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
      ${editing && account.archived && html`<button type="button" class="btn" onClick=${restore}>${t("accounts.restore")}</button>`}
      ${editing && !account.archived && html`<button type="button" class="btn btn-danger" onClick=${remove}>${account.used > 0 ? t("accounts.archive") : t("common.delete")}</button>`}
    </div>
  <//>`;
}

/** One account: its balance and operations, newest first. */
function AccountView({ id }) {
  const s = useStore();
  const a = (s.home?.accounts || []).find(x => x.id === id);
  const [ops, setOps] = useState(null);
  const reload = () => data.loadAccountOperations(id).then(setOps).catch(e => toast(errorText(e), "error"));
  useEffect(() => { reload(); }, [id, s.home]);
  if (!a) return html`<${Sheet} title=${t("accounts.title")}><${Empty}>${t("common.loading")}<//><//>`;
  // Any operation opens for changing or deleting; afterwards the account is shown again.
  const open = op => {
    haptic();
    if (op.type === "transfer") return openSheet("moneyTransfer", { op, accountId: a.id });
    if (op.category_id) openSheet("editOperation", { op: { ...op, account_id: a.id }, categoryId: op.category_id, back: { kind: "accountView", props: { id: a.id } } });
  };
  return html`<${Sheet} title=${`${a.emoji} ${a.name}`}>
    <div class="account-balance">
      <div class="muted small">${[a.owner_name || t("accounts.shared"), a.archived ? t("accounts.archived") : ""].filter(Boolean).join(" · ")}</div>
      <div class=${"account-balance-value" + (a.balance < 0 ? " neg" : "")}>${money(a.balance, a.currency)}</div>
      ${a.initial_amount !== 0 && html`<div class="muted small">${a.initial_date
        ? t("accounts.startOn", { sum: money(a.initial_amount, a.currency), date: dayTitle(parseYmd(a.initial_date)) })
        : t("accounts.startAll", { sum: money(a.initial_amount, a.currency) })}</div>`}
    </div>
    ${a.target_amount > 0 && html`<div class="savings-goal">
      <div class="bar bar-big"><span style=${`width:${Math.min(100, Math.max(0, (a.balance / a.target_amount) * 100))}%`}></span></div>
      <div class="muted small">${t("savings.goalLine", { goal: money(a.target_amount, a.currency), left: money(Math.max(0, a.target_amount - a.balance), a.currency), pct: Math.round(Math.max(0, (a.balance / a.target_amount) * 100)) })}</div>
    </div>`}
    ${a.interest && html`<div class="card interest-card">
      <div class="in-main-row"><span>${a.interest.kind === "term" ? t("acc.term") : t("acc.flexible")}</span><b>${pctText(a.interest.rate)}%${a.interest.tax ? html`<span class="muted small"> · ${t("acc.tax")} ${pctText(a.interest.tax)}%</span>` : ""}</b></div>
      ${a.interest.ended
        ? html`<div class="in-main-row"><span class="muted">${t("acc.ended", { date: dayTitle(parseYmd(a.interest.ended)) })}</span><span>${payoutName(s, a.interest.payout_account_id)}</span></div>`
        : html`<div class="in-main-row"><span class="muted">${t("savings.accrued")}</span><span>${money(a.interest.accrued, a.currency)}</span></div>
          <div class="in-main-row"><span class="muted">${t("savings.nextPayout", { date: dayTitle(parseYmd(a.interest.next_payout_date)) })}</span><span>${money(a.interest.next_payout, a.currency)}</span></div>
          ${a.interest.next_tax > 0 && html`<div class="in-main-row small"><span class="muted">${t("acc.grossTax")}</span><span class="muted">${money(a.interest.next_gross, a.currency)} − ${money(a.interest.next_tax, a.currency)}</span></div>`}
          ${a.interest.kind === "term" && html`<div class="muted small">${a.interest.renew ? t("acc.renewsShort") : t("acc.returnsTo", { name: payoutName(s, a.interest.payout_account_id) })}</div>`}`}
      <div class="in-main-row"><span class="muted">${t("savings.paidTotal")}</span><span>${money(a.interest.interest_paid, a.currency)}</span></div>
      <div class="in-main-row"><span class="muted">${t("savings.yearForecast")}</span><span>${money(a.interest.year_forecast, a.currency)}</span></div>
    </div>`}
    <div class="chips">
      ${a.interest?.ended && html`<button type="button" class="chip active" onClick=${() => openSheet("newTerm", { id: a.id })}>↻ ${t("acc.newTerm")}</button>`}
      ${a.kind === "savings" && html`<button type="button" class="chip" onClick=${() => openSheet("moneyTransfer", { toId: a.id })}>＋ ${t("savings.putAside")}</button>`}
      <button type="button" class="chip" onClick=${() => openSheet("moneyTransfer", { fromId: a.id })}>⇄ ${t("accounts.transfer")}</button>
      <button type="button" class="chip" onClick=${() => openSheet("moneyAccount", { account: a })}>✎ ${t("accounts.edit")}</button>
    </div>
    ${ops === null ? html`<${Empty}>${t("common.loading")}<//>` : !ops.length ? html`<${Empty}>${t("cat.noOperations")}<//>` : html`<div class="ops">
      ${ops.map(op => {
        const transfer = op.type === "transfer";
        const title = transfer
          ? `⇄ ${op.other_account || ""}`
          : `${op.category_emoji || ""} ${op.subcategory_name || op.category_name || ""}`.trim();
        const other = transfer && op.other_currency && op.other_currency !== op.currency ? money(op.other_amount, op.other_currency) : "";
        // The rate it went at, from this account's side: 1 of the money that left = x of what arrived.
        const rateOf = other && op.amount && op.other_amount
          ? (op.signed < 0 ? `1 ${op.currency} = ${rateText(op.other_amount / op.amount)} ${op.other_currency}` : `1 ${op.other_currency} = ${rateText(op.amount / op.other_amount)} ${op.currency}`)
          : "";
        const note = [dayTitle(parseYmd(op.transaction_date)), op.note, other && (op.signed < 0 ? `→ ${other}` : `← ${other}`), rateOf].filter(Boolean).join(" · ");
        return html`<button type="button" class="op" onClick=${() => open(op)}>
          <span class="op-main">
            <span class="op-title">${title}</span>
            <span class="op-note">${note}</span>
            ${op.actor_name && html`<span class="op-who">${op.actor_name}</span>`}
          </span>
          <span class=${"op-amount " + (op.signed < 0 ? "op-expense" : "op-income")}>${op.signed < 0 ? "−" : "+"}${money(op.amount, op.currency)}</span>
        </button>`;
      })}
    </div>`}
  <//>`;
}

/** Where a deposit's money goes back to by default: a shared everyday account, else one's own, else any. */
const payoutDefault = (list, meId) => {
  const everyday = list.filter(a => a.kind !== "savings");
  return everyday.find(a => a.owner_id == null) || everyday.find(a => a.owner_id === meId) || everyday[0] || list[0] || null;
};

/** The name of a deposit's payout account, or a dash when there is none. */
const payoutName = (s, id) => {
  const p = (s.home?.accounts || []).find(x => x.id === id);
  return p ? `${p.emoji} ${p.name}` : "—";
};

/**
 * A new term of a deposit that doesn't renew: its day, days, rate and tax (the last term's by
 * default), and the money put in, moved from another account on that day.
 */
function NewTerm({ id }) {
  const s = useStore();
  const a = (s.home?.accounts || []).find(x => x.id === id);
  const i = a?.interest;
  const [start, setStart] = useState(ymd(today()));
  const [days, setDays] = useState(String(i?.term_days || 32));
  const [rate, setRate] = useState(i ? pctText(i.rate) : "");
  const [tax, setTax] = useState(i ? pctText(i.tax) : "");
  const sources = activeAccounts(s).filter(x => x.currency === a?.currency && x.id !== id);
  const [fromId, setFromId] = useState(i?.payout_account_id ?? null);
  const from = sources.find(x => x.id === fromId) || payoutDefault(sources, s.me?.id);
  const [amount, setAmount] = useState("");
  if (!a || !i) return html`<${Sheet} title=${t("acc.newTerm")}><${Empty}>${t("common.loading")}<//><//>`;
  const save = async () => {
    const termDays = Math.round(decimal(days));
    if (!(termDays >= 1)) return tell(t("acc.enterDays"));
    if (!(decimal(rate) >= 0)) return tell(t("acc.enterRate"));
    const v = amount.trim() ? evalAmount(amount) : 0;
    if (!Number.isFinite(v) || v < 0) return tell(t("form.enterAmount"));
    const body = {
      interest: { kind: "term", rate: decimal(rate), tax: tax.trim() ? decimal(tax) : 0, term_days: termDays, start, renew: false, payout_account_id: i.payout_account_id ?? null },
      from_id: v > 0 ? from?.id ?? null : null,
      amount: v > 0 ? v : null,
    };
    if (await attempt(() => data.newTerm(id, body), t("acc.saved"))) {
      openSheet("accountView", { id });
      data.refresh();
    }
  };
  return html`<${Sheet} title=${`${a.emoji} ${a.name} · ${t("acc.newTerm")}`}>
    <div class="form">
      <div class="muted small">${t("acc.newTermHint", { sum: money(a.balance, a.currency) })}</div>
      <label class="date-row">
        <span class="date-label">${t("acc.start")}</span>
        <input type="date" class="field date-input" value=${start} max=${ymd(today())} onChange=${e => e.currentTarget.value && setStart(e.currentTarget.value)} />
      </label>
      <label class="acc-field">
        <span class="muted">${t("acc.days")}</span>
        <input class="field" inputmode="numeric" value=${days} onInput=${e => setDays(e.currentTarget.value)} />
      </label>
      <${Chips} items=${TERMS.map(n => [String(n), t("acc.daysShort", { n })])} value=${days} onChange=${setDays} />
      <div class="acc-fields">
        <label class="acc-field">
          <span class="muted">${t("acc.rate")}</span>
          <span class="acc-input"><input class="field" inputmode="decimal" value=${rate} onInput=${e => setRate(e.currentTarget.value)} placeholder="45" /><span>%</span></span>
        </label>
        <label class="acc-field">
          <span class="muted">${t("acc.tax")}</span>
          <span class="acc-input"><input class="field" inputmode="decimal" value=${tax} onInput=${e => setTax(e.currentTarget.value)} placeholder="0" /><span>%</span></span>
        </label>
      </div>
      <div class="muted small">${t("acc.newTermMoney")}</div>
      ${sources.length > 0 && html`<${AccountSelect} accounts=${sources} value=${from?.id} onChange=${setFromId} />`}
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${a.currency} onCurrency=${() => {}} locked onEnter=${save} />
      <button type="button" class="btn btn-primary" onClick=${save}>${t("acc.openTerm")}</button>
    </div>
  <//>`;
}

const FROM_KEY = "budget:v2:transferFrom";

/** A rate as typed or shown: up to six significant digits, comma as in the amount fields. */
const rateText = r => (r > 0 ? String(Number(r.toPrecision(6))).replace(".", ",") : "");
const num = v => Number(String(v).trim().replace(/\s/g, "").replace(",", "."));
const amountText = v => (v > 0 ? String(Math.round(v * 100) / 100).replace(".", ",") : "");

/**
 * Money from one account to another. Between currencies the rate of the transfer's day comes
 * from the budget's source; it, or the amount received, can be corrected by hand and the other
 * one follows.
 */
function MoneyTransfer({ fromId, toId, toSavings, op, accountId }) {
  const s = useStore();
  const list = activeAccounts(s);
  const all = s.home?.accounts || [];
  // Editing: this account's half and the other one, whichever went out.
  const out = op ? op.signed < 0 : true;
  // Opened from an account's operations: back to that account afterwards.
  const done = () => (accountId != null ? openSheet("accountView", { id: accountId }) : closeSheet());
  const lastFrom = (() => {
    try {
      return Number(localStorage.getItem(FROM_KEY)) || null;
    } catch {
      return null;
    }
  })();
  // A new transfer starts from the last account money was moved from, else the first account (the main one).
  const everyday = list.filter(a => a.kind !== "savings");
  const intoSavings = toId ?? (toSavings ? list.find(a => a.kind === "savings")?.id : null);
  const startFrom = op ? (out ? accountId : op.other_account_id)
    : fromId ?? (intoSavings ? (everyday.find(a => a.id === lastFrom) || everyday[0])?.id : null) ?? list.find(a => a.id === lastFrom)?.id ?? list[0]?.id;
  const startTo = op ? (out ? op.other_account_id : accountId) : intoSavings ?? list.find(a => a.id !== startFrom)?.id;
  const sent = op ? (out ? op.amount : op.other_amount) : 0;
  const got = op ? (out ? op.other_amount : op.amount) : 0;
  const [from, setFrom] = useState(startFrom);
  const [to, setTo] = useState(startTo);
  const [amount, setAmount] = useState(op ? amountText(sent) : "");
  const [received, setReceived] = useState(op ? amountText(got) : "");
  const [rate, setRate] = useState(op && sent ? rateText(got / sent) : "");
  // "auto": the source's rate for the day; "manual": typed (or an existing transfer's own).
  const [manual, setManual] = useState(!!op);
  const [auto, setAuto] = useState(null);
  const [note, setNote] = useState(op?.note || "");
  const [date, setDate] = useState(op?.transaction_date || ymd(today()));
  const fromAcc = all.find(a => a.id === from), toAcc = all.find(a => a.id === to);
  // Shown in red after a try to save: what is wrong with the choice of accounts.
  const [tried, setTried] = useState(false);
  const fromBad = !fromAcc;
  const toBad = !toAcc || from === to;
  const choiceError = fromBad ? t("accounts.pickFrom") : !toAcc ? t("accounts.pickTo") : from === to ? t("accounts.pickDifferent") : "";
  const cross = !!(fromAcc && toAcc && fromAcc.currency !== toAcc.currency);
  const source = (() => {
    const src = s.home?.settings?.rate_source;
    return src && src !== "off" ? src : "tcmb";
  })();
  useEffect(() => {
    if (!cross) return setAuto(null);
    let live = true;
    setAuto({ loading: true });
    api(`/api/rates?source=${source}&day=${date}`)
      .then(r => {
        const a = r.in_try?.[fromAcc.currency], b = r.in_try?.[toAcc.currency];
        if (live) setAuto(a && b ? { rate: a / b, source: r.source, date: r.date } : { none: true, source: r.source });
      })
      .catch(() => live && setAuto({ none: true, source }));
    return () => (live = false);
  }, [cross, fromAcc?.currency, toAcc?.currency, date]);
  // The source's rate, until one is typed: the amount received follows it.
  useEffect(() => {
    if (manual || !auto?.rate) return;
    setRate(rateText(auto.rate));
    const v = evalAmount(amount);
    setReceived(v > 0 ? amountText(v * auto.rate) : "");
  }, [auto, manual]);
  const onAmount = v => {
    setAmount(v);
    const r = num(rate), x = evalAmount(v);
    if (cross && r > 0) setReceived(x > 0 ? amountText(x * r) : "");
  };
  const onRate = v => {
    setRate(v);
    setManual(true);
    const r = num(v), x = evalAmount(amount);
    if (r > 0 && x > 0) setReceived(amountText(x * r));
  };
  const onReceived = v => {
    setReceived(v);
    setManual(true);
    const w = evalAmount(v), x = evalAmount(amount);
    if (w > 0 && x > 0) setRate(rateText(w / x));
  };
  const backToSource = () => setManual(false);
  const pickFrom = id => {
    setFrom(id);
    setManual(false);
  };
  const pickTo = id => {
    setTo(id);
    setManual(false);
  };
  const swap = () => {
    haptic();
    setFrom(to);
    setTo(from);
    setManual(false);
  };
  const save = async () => {
    setTried(true);
    if (choiceError) {
      haptic("error");
      return;
    }
    const v = evalAmount(amount);
    if (!(v > 0)) return tell(t("form.enterAmount"));
    const w = cross ? evalAmount(received) : v;
    if (!(w > 0)) return tell(t("accounts.enterReceived"));
    const body = { fromId: from, toId: to, amount: v, toAmount: w, note: note.trim(), date };
    const ok = await attempt(() => (op ? data.updateAccountTransfer(op.id, body) : data.accountTransfer(body)), t("transfer.done"));
    if (ok) {
      try {
        localStorage.setItem(FROM_KEY, String(from));
      } catch {}
      done();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(t("accounts.confirmDeleteTransfer")))) return;
    if (await attempt(() => data.deleteAccountTransfer(op.id), t("op.deleted"))) {
      done();
      data.refresh();
    }
  };
  if (list.length < 2 && !op) return html`<${Sheet} title=${t("accounts.transfer")}><${Empty}>${t("accounts.needTwo")}<//><//>`;
  const rateNote = manual
    ? t("accounts.yourRate")
    : auto?.loading ? t("common.loading")
    : auto?.rate ? t("accounts.rateOf", { source: t("rates.src." + auto.source), date: dayTitle(parseYmd(auto.date)) })
    : auto?.none ? t("accounts.noRate", { source: t("rates.src." + (auto.source || source)) }) : "";
  return html`<${Sheet} title=${op ? t("accounts.transferEdit") : t("accounts.transfer")}>
    <div class="form">
      <div class="transfer-pair">
        <${AccountSelect} accounts=${list} value=${from} onChange=${pickFrom} label=${t("transfer.from")} bad=${tried && fromBad} inside />
        <${AccountSelect} accounts=${list} value=${to} onChange=${pickTo} label=${t("transfer.to")} bad=${tried && toBad} inside />
        <button type="button" class="swap-btn" onClick=${swap} aria-label=${t("accounts.swap")}>⇅</button>
      </div>
      ${tried && choiceError && html`<div class="error-note small">⚠️ ${choiceError}</div>`}
      <div class="muted small">${cross ? t("accounts.send") : t("accounts.moveAmount")}</div>
      <${MoneyField} value=${amount} onInput=${onAmount} currency=${fromAcc?.currency || ""} onCurrency=${() => {}} locked autofocus=${!op} onEnter=${save} />
      ${cross && html`<div class="rate-row">
          <span>1 ${fromAcc.currency} =</span>
          <input class="field rate-input" inputmode="decimal" value=${rate} onInput=${e => onRate(e.currentTarget.value)} aria-label=${t("accounts.rate")} />
          <span>${toAcc.currency}</span>
        </div>
        <div class="muted small rate-note">${rateNote}
          ${manual && auto?.rate && html` · <button type="button" class="link-btn" onClick=${backToSource}>${t("accounts.useSourceRate")}</button>`}</div>
        <div class="muted small">${t("accounts.received")}</div>
        <${MoneyField} value=${received} onInput=${onReceived} currency=${toAcc.currency} onCurrency=${() => {}} locked onEnter=${save} />`}
      <input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />
      <label class="date-row">
        <span class="date-label">📅 ${t("form.date")}</span>
        <input type="date" class="field date-input" value=${date} onChange=${e => e.currentTarget.value && setDate(e.currentTarget.value)} />
      </label>
      <${BeforeStart} accounts=${[fromAcc, toAcc]} date=${date} />
      <button type="button" class="btn btn-primary" onClick=${save}>${op ? t("common.save") : t("accounts.transfer")}</button>
      ${op && html`<button type="button" class="btn btn-danger" onClick=${remove}>${t("common.delete")}</button>`}
    </div>
  <//>`;
}

export const ACCOUNT_SHEETS = { savingsGoal: SavingsGoalSheet, moneyAccounts: MoneyAccounts, moneyAccount: MoneyAccount, accountView: AccountView, moneyTransfer: MoneyTransfer, newTerm: NewTerm };
