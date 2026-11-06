// Home: the month's balance, where the money went, and the categories.

import { html, useState } from "../../vendor/preact-htm.js";
import { t } from "../i18n.js";
import { CURRENCIES, money, usedCurrencies, monthTitle, addMonths, sameMonth, today, ymd } from "../format.js";
import { state, useStore, openSheet } from "../store.js";
import { navigate } from "../router.js";
import { setMonth } from "../data.js";
import { haptic } from "../tg.js";
import { Empty, lastCurrency } from "../ui.js";
import { useSwipe } from "./swipe.js";
import { AccountsCard, SavingsCard } from "./accounts.js";
import { RatesLine, Converted } from "./rates-ui.js";

export function MonthBar() {
  const s = useStore();
  const shift = n => {
    haptic();
    setMonth(addMonths(s.month, n));
  };
  const isCurrent = sameMonth(s.month, today());
  return html`<div class="monthbar">
    <button type="button" class="icon-btn" onClick=${() => shift(-1)} aria-label=${t("nav.prev")}>‹</button>
    <button type="button" class="monthbar-title" onClick=${() => !isCurrent && setMonth(new Date(today().getFullYear(), today().getMonth(), 1))}>
      ${monthTitle(s.month)}${!isCurrent && html`<span class="monthbar-hint">${t("home.backToNow")}</span>`}
    </button>
    <button type="button" class="icon-btn" onClick=${() => shift(1)} aria-label=${t("nav.next")}>›</button>
  </div>`;
}

/** One thin bar split by category: where the month's money went. */
function ShareBar({ parts, total }) {
  return html`<div class="share-bar" aria-hidden="true">
    ${parts.map(p => html`<span style=${`flex:${p.value / total};background:${p.color}`}></span>`)}
  </div>`;
}

const near0 = v => Math.abs(v || 0) < 0.005;
const signedMoney = (v, cur) => (v > 0 ? "+" : "") + money(v, cur);

/** What else moved the everyday money this month besides income and expenses. */
function moves(sum, cur) {
  const saved = sum.saved?.[cur] || 0, exchanged = sum.exchanged?.[cur] || 0, opened = sum.opened?.[cur] || 0;
  return [
    !near0(saved) && t("home.toSavings", { sum: signedMoney(-saved, cur) }),
    !near0(exchanged) && t("home.exchanged", { sum: signedMoney(exchanged, cur) }),
    !near0(opened) && t("home.opened", { sum: signedMoney(opened, cur) }),
  ].filter(Boolean);
}

/** "By the month's end ≈ 142 000 ₽"; a tap shows what it is made of. Current month only. */
function ForecastLine({ forecast, currency }) {
  const l = forecast?.by_currency?.[currency];
  if (!l) return null;
  const usual = Math.round(l.per_day * forecast.days_left);
  return html`<details class="forecast">
    <summary><span>${t("forecast.end")}</span><b class=${l.end < 0 ? "negative" : ""}>≈ ${money(Math.round(l.end), currency)}</b></summary>
    <div class="forecast-rows">
      <span>${t("forecast.now")}</span><span>${money(l.now, currency)}</span>
      ${Math.abs(l.scheduled) >= 0.005 && html`<span>${t("forecast.scheduled")}</span><span>${signedMoney(l.scheduled, currency)}</span>`}
      ${usual > 0 && html`<span>${t("forecast.usual", { sum: money(Math.round(l.per_day), currency) })}</span><span>${signedMoney(-usual, currency)}</span>`}
    </div>
    ${forecast.short_history && html`<div class="muted small">${t("forecast.short")}</div>`}
  </details>`;
}

/** One column per currency with money this month; with several, a tap picks the one the list below shows. */
function Summary({ home, currencies, currency, onPick, parts, spent }) {
  const sum = home.month_summary || {};
  const several = currencies.length > 1;
  return html`<div class="card summary">
    <div class="summary-label">${t("home.balance")}</div>
    <div class="summary-cols">
      ${currencies.map(cur => {
        const balance = sum.balance?.[cur] || 0;
        const carry = sum.carry_in?.[cur] || 0;
        return html`<button type="button" class=${"summary-col" + (several && cur === currency ? " active" : "")}
          onClick=${() => several && onPick(cur)} disabled=${!several}>
          <span class=${"summary-balance" + (balance < 0 ? " negative" : "")}>${money(balance, cur)}</span>
          <span class="summary-flow">
            <span class="flow-in">↑ ${money(sum.income?.[cur] || 0, cur)}</span>
            <span class="flow-out">↓ ${money(sum.expense?.[cur] || 0, cur)}</span>
          </span>
          ${Math.abs(carry) > 0.004 && html`<span class="flow-carry">${t("home.carried", { sum: money(carry, cur) })}</span>`}
          ${moves(sum, cur).map(m => html`<span class="flow-carry">${m}</span>`)}
          <${Converted} amount=${balance} currency=${cur} />
        </button>`;
      })}
    </div>
    <${ForecastLine} forecast=${home.forecast} currency=${currency} />
    ${spent > 0 && html`<${ShareBar} parts=${parts} total=${spent} />`}
    <${RatesLine} base=${currency} />
  </div>`;
}

const open = c => navigate({ name: "category", id: c.id });

/** How a category stands against its monthly limit, or null without one. */
export function limitState(c) {
  if (!(c.limit_amount > 0) || !c.limit_currency) return null;
  // Whole units: spending in other currencies is converted, and kopecks of it mean nothing.
  const spent = Math.round(c.limit_spent || 0), limit = c.limit_amount, cur = c.limit_currency;
  const ratio = spent / limit;
  // Spending at this pace goes past the limit by the month's end.
  const expected = c.limit_forecast > limit ? Math.round(c.limit_forecast) : null;
  const level = ratio > 1 ? "over" : ratio >= 0.8 || expected ? "warn" : "ok";
  return { spent, limit, cur, ratio, left: limit - spent, level, expected, partial: !!c.limit_partial };
}

/** "12 400 из 15 000 ₽" with a bar that turns yellow near the limit and red past it. */
function LimitLine({ c, l }) {
  return html`<span class="cat-line">
    <span class="cat-bar"><span class=${"limit-" + l.level} style=${`width:${Math.max(Math.min(l.ratio, 1) * 100, l.spent > 0 ? 1.5 : 0)}%;${l.level === "ok" ? `background:${c.color}` : ""}`}></span></span>
    <span class=${"cat-pct limit-text-" + l.level}>${l.level === "over"
      ? t("limit.over", { sum: money(-l.left, l.cur) })
      : t("limit.of", { spent: money(l.spent, l.cur), limit: money(l.limit, l.cur) })}</span>
  </span>`;
}

/** An expense category: its share of the month's spending in the chosen currency. */
function ExpenseRow({ c, currency, spent }) {
  const l = limitState(c);
  const value = c.totals_by_currency?.[currency] || 0;
  const others = usedCurrencies(c.totals_by_currency).filter(cur => cur !== currency);
  const pct = spent > 0 ? (value / spent) * 100 : 0;
  return html`<button type="button" class=${"cat-row" + (value > 0 || others.length || l ? "" : " idle")} onClick=${() => open(c)}>
    <span class="cat-emoji">${c.emoji}</span>
    <span class="cat-main">
      <span class="cat-line">
        <span class="cat-name">${c.name}</span>
        <span class="cat-value">${money(value, currency)}</span>
      </span>
      ${l && html`<${LimitLine} c=${c} l=${l} />`}
      ${!l && value > 0 && html`<span class="cat-line">
        <span class="cat-bar"><span style=${`width:${Math.max(pct, 1.5)}%;background:${c.color}`}></span></span>
        <span class="cat-pct">${pct < 1 ? "<1" : Math.round(pct)}%</span>
      </span>`}
      ${others.length > 0 && html`<span class="cat-extra">${others.map(cur => money(c.totals_by_currency[cur], cur)).join(" · ")}</span>`}
    </span>
  </button>`;
}

/** Savings show what has been saved in all, with the goal; income shows the month. */
function FundRow({ c, savings, income }) {
  const isSavings = c.type === "savings" && savings;
  const month = isSavings ? c.totals_by_currency : income;
  const used = usedCurrencies(month);
  const goal = isSavings && savings.target_amount > 0;
  return html`<button type="button" class="cat-row" onClick=${() => open(c)}>
    <span class="cat-emoji">${c.emoji}</span>
    <span class="cat-main">
      <span class="cat-line">
        <span class="cat-name">${c.name}</span>
        <span class="cat-value">${isSavings
          ? money(savings.saved_amount, savings.currency)
          : used.length ? used.map(cur => money(month[cur], cur)).join(" · ") : money(0, lastCurrency())}</span>
      </span>
      ${goal && html`<span class="cat-line">
        <span class="cat-bar"><span class="goal-fill" style=${`width:${Math.min(100, savings.progress_percent)}%`}></span></span>
        <span class="cat-pct">${t("home.goalOf", { sum: money(savings.target_amount, savings.currency) })}</span>
      </span>`}
      ${isSavings && used.length > 0 && !(used.length === 1 && used[0] === savings.currency && Math.abs(month[used[0]] - savings.saved_amount) < 0.005) && html`<span class="cat-extra">${t("home.thisMonth")} ${used.map(cur => "+" + money(month[cur], cur)).join(" · ")}</span>`}
    </span>
  </button>`;
}

export function Home() {
  const s = useStore();
  const home = s.home;
  const swipe = useSwipe(n => setMonth(addMonths(s.month, n)));
  const [picked, setPicked] = useState(null);
  if (!home) return html`<${MonthBar} /><${Empty}>${t("common.loading")}<//>`;

  const expenses = home.categories.filter(c => c.type === "expense");
  const totals = {};
  for (const c of expenses) for (const [cur, v] of Object.entries(c.totals_by_currency || {})) totals[cur] = (totals[cur] || 0) + v;
  const spentIn = usedCurrencies(totals);
  const sum = home.month_summary || {};
  const active = new Set([...spentIn, ...usedCurrencies(sum.income), ...usedCurrencies(sum.balance), ...usedCurrencies(sum.carry_in)]);
  // A fixed order, so the columns don't swap places: the four first, then others by code.
  const currencies = [...CURRENCIES.filter(c => active.has(c)), ...[...active].filter(c => !CURRENCIES.includes(c)).sort()];
  if (!currencies.length) currencies.push(lastCurrency());
  const currency = currencies.includes(picked) ? picked : spentIn[0] || currencies[0];
  const spent = totals[currency] || 0;
  const value = c => c.totals_by_currency?.[currency] || 0;
  // Biggest spending first; empty categories keep their own order at the end. The rows are new
  // for each month: Chromium kept the dimmed look on a row that moved and stopped being empty.
  const sorted = [...expenses].sort((a, b) => value(b) - value(a));
  const parts = sorted.filter(c => value(c) > 0).map(c => ({ value: value(c), color: c.color }));
  const funds = ["income"].map(k => home.categories.find(c => c.type === k)).filter(Boolean);

  return html`<div ...${swipe}>
    <${MonthBar} />
    <div class="home-grid">
      <div class="home-main">
        <${Summary} home=${home} currencies=${currencies} currency=${currency} onPick=${c => { haptic(); setPicked(c); }} parts=${parts} spent=${spent} />
        ${spentIn.length === 0 && html`<div class="hint-card">${t("home.noExpenses")}</div>`}
        ${sorted.length > 0 && html`<div class="group cat-group">
          ${sorted.map(c => html`<${ExpenseRow} key=${`${c.id}:${ymd(s.month)}`} c=${c} currency=${currency} spent=${spent} />`)}
        </div>`}
      </div>
      <div class="home-side">
        ${funds.length > 0 && html`<div class="group cat-group">
          ${funds.map(c => html`<${FundRow} key=${c.id} c=${c} savings=${home.savings_summary} income=${home.month_summary?.income} />`)}
        </div>`}
        <${SavingsCard} />
        <${AccountsCard} />
      </div>
    </div>
    <button type="button" class="fab" onClick=${() => { haptic(); openSheet("quickAdd", {}); }}>＋ ${t("home.addExpense")}</button>
  </div>`;
}
