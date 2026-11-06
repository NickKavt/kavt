// Reports: spending and income for a day, week, month, year or any period.

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText } from "../i18n.js";
import { money, usedCurrencies, ymd, parseYmd, today, addDays, daysBetween, fmt, weekday, shortMonth, monthTitle } from "../format.js";
import { loadReport, search } from "../data.js";
import { state, update, openSheet, useStore } from "../store.js";
import { Seg, Chips, Empty } from "../ui.js";
import { OperationList } from "./category.js";
import { useSwipe } from "./swipe.js";

// The period on screen survives leaving the tab.
function period() {
  if (!state.report) {
    const d = today();
    state.report = { mode: "month", anchor: d, from: ymd(new Date(d.getFullYear(), d.getMonth(), 1)), to: ymd(d), currency: null };
  }
  return state.report;
}

export function range(p) {
  const a = p.anchor;
  switch (p.mode) {
    case "day": return [a, a];
    case "week": {
      const monday = addDays(a, -((a.getDay() + 6) % 7));
      return [monday, addDays(monday, 6)];
    }
    case "year": return [new Date(a.getFullYear(), 0, 1), new Date(a.getFullYear(), 11, 31)];
    case "custom": {
      const x = parseYmd(p.from), y = parseYmd(p.to);
      return x <= y ? [x, y] : [y, x];
    }
    default: return [new Date(a.getFullYear(), a.getMonth(), 1), new Date(a.getFullYear(), a.getMonth() + 1, 0)];
  }
}

function label(p) {
  const [from, to] = range(p);
  const now = today();
  const short = (d, y) => fmt(d, y ? { day: "numeric", month: "short", year: "numeric" } : { day: "numeric", month: "short" });
  switch (p.mode) {
    case "day": {
      const diff = daysBetween(from, now);
      const base = fmt(from, from.getFullYear() !== now.getFullYear() ? { day: "numeric", month: "long", year: "numeric" } : { day: "numeric", month: "long" });
      return diff === 0 ? `${t("common.today")}, ${base}` : diff === 1 ? `${t("common.yesterday")}, ${base}` : `${base}, ${weekday(from)}`;
    }
    case "week": {
      const other = to.getFullYear() !== now.getFullYear();
      return `${short(from, other && from.getFullYear() !== to.getFullYear())} – ${short(to, other)}`;
    }
    case "year": return String(from.getFullYear());
    default: return monthTitle(from);
  }
}

/** The period a report is compared with: the one before, of the same kind. While the period
 * is going on, the one before is cut at the same point, so half a month meets half a month. */
export function compareRange(p, from, to, now = today()) {
  const going = from <= now && now <= to;
  const day = d => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  switch (p.mode) {
    case "day": return [addDays(from, -1), addDays(from, -1)];
    case "week": return [addDays(from, -7), addDays(going ? now : to, -7)];
    case "month": {
      const start = new Date(from.getFullYear(), from.getMonth() - 1, 1);
      const last = new Date(from.getFullYear(), from.getMonth(), 0);
      return [start, going ? day(new Date(Math.min(+last, +addDays(start, now.getDate() - 1)))) : last];
    }
    case "year": {
      const start = new Date(from.getFullYear() - 1, 0, 1);
      const end = going ? new Date(now.getFullYear() - 1, now.getMonth(), Math.min(now.getDate(), new Date(now.getFullYear() - 1, now.getMonth() + 1, 0).getDate())) : new Date(from.getFullYear() - 1, 11, 31);
      return [start, end];
    }
    default: {
      const len = daysBetween(from, to);
      const end = addDays(from, -1);
      return [addDays(end, -len), end];
    }
  }
}

/** "Oct 2026", "2025" or "1 Oct – 6 Oct": the compared period. */
function compareLabel(p, [from, to]) {
  const lastOfMonth = new Date(from.getFullYear(), from.getMonth() + 1, 0);
  if (p.mode === "month" && from.getDate() === 1 && ymd(to) === ymd(lastOfMonth)) return monthTitle(from);
  if (p.mode === "year" && ymd(to) === `${from.getFullYear()}-12-31`) return String(from.getFullYear());
  const short = d => fmt(d, { day: "numeric", month: "short" });
  return ymd(from) === ymd(to) ? short(from) : `${short(from)} – ${short(to)}`;
}

/** "▲ 12 %" for more spending (red), "▼ 8 %" for less (green); "new" where there was none. */
function Delta({ now, before }) {
  if (!(before > 0.005)) return now > 0.005 ? html`<span class="delta delta-new">${t("reports.new")}</span>` : null;
  const pct = Math.round(((now - before) / before) * 100);
  if (pct === 0) return html`<span class="delta">= 0%</span>`;
  return html`<span class=${"delta " + (pct > 0 ? "delta-up" : "delta-down")}>${pct > 0 ? "▲" : "▼"} ${Math.abs(pct)}%</span>`;
}

function shift(p, n) {
  const a = new Date(p.anchor);
  if (p.mode === "day") a.setDate(a.getDate() + n);
  else if (p.mode === "week") a.setDate(a.getDate() + 7 * n);
  else if (p.mode === "month") a.setMonth(a.getMonth() + n, 1);
  else a.setFullYear(a.getFullYear() + n);
  return { ...p, anchor: a };
}

function Bars({ p, report, currency, from, to }) {
  if (p.mode === "day") return null;
  const span = daysBetween(from, to) + 1;
  const byMonth = span > 62;
  const buckets = [];
  if (byMonth) for (let m = new Date(from.getFullYear(), from.getMonth(), 1); m <= to; m = new Date(m.getFullYear(), m.getMonth() + 1, 1)) buckets.push({ key: ymd(m).slice(0, 7), label: shortMonth(m), v: 0 });
  else for (let d = new Date(from); d <= to; d = addDays(d, 1)) buckets.push({ key: ymd(d), label: p.mode === "week" ? weekday(d) : String(d.getDate()), v: 0 });
  const index = new Map(buckets.map((b, i) => [b.key, i]));
  let total = 0;
  for (const day of report.days) {
    const v = day.expense?.[currency] || 0;
    const i = index.get(byMonth ? day.date.slice(0, 7) : day.date);
    if (i !== undefined) buckets[i].v += v;
    total += v;
  }
  if (!total) return null;
  const max = Math.max(...buckets.map(b => b.v));
  const passed = Math.max(1, Math.min(span, daysBetween(from, today()) + 1));
  const every = buckets.length > 16 ? Math.ceil(buckets.length / 8) : 1;
  return html`<div class="card">
    <div class="card-head">
      <span class="card-title">${byMonth ? t("reports.byMonth") : t("reports.byDay")}</span>
      <span class="muted small">${t("reports.average", { sum: money(total / passed, currency) })}</span>
    </div>
    <div class="bars">
      ${buckets.map((b, i) => html`<div class="bars-col" title=${`${b.key}: ${money(b.v, currency)}`}>
        <div class="bars-track"><div class="bars-fill" style=${`height:${b.v ? Math.max(4, (b.v / max) * 100) : 0}%`}></div></div>
        <div class="bars-label">${i % every === 0 ? b.label : ""}</div>
      </div>`)}
    </div>
  </div>`;
}

/** Expenses and income of the period in the main currency, each operation at its own day's rate. */
function InMain({ report }) {
  const settings = useStore().home?.settings;
  const main = settings?.main_currency;
  if (!main || settings.rate_source === "off" || useStore().me?.show_converted === false) return null;
  const sums = { expense: 0, income: 0 };
  let missing = 0, foreign = false;
  for (const op of report.transactions) {
    if (!(op.type in sums)) continue;
    if (op.currency === main) sums[op.type] += op.amount;
    else if (op.rate && op.rate_to === main) {
      sums[op.type] += op.amount * op.rate;
      foreign = true;
    } else missing++;
  }
  // Worth a line only when other currencies are in the period.
  if (!foreign && !missing) return null;
  return html`<div class="card in-main">
    <div class="muted small">${t("reports.inMain", { cur: main })}</div>
    <div class="in-main-row"><span>${t("kind.expenses")}</span><b>${money(sums.expense, main)}</b></div>
    <div class="in-main-row"><span>${t("kind.income")}</span><b>${money(sums.income, main)}</b></div>
    ${missing > 0 && html`<div class="muted small">${t("reports.inMainMissing", { n: missing })}</div>`}
  </div>`;
}

/** "Previous period, Oct 2026: 98 300 ₽ ▼ 5 %" for the chosen currency. */
function CompareLine({ p, report, currency }) {
  const c = report.compare;
  if (!c) return null;
  const before = c.expense?.[currency] || 0, now = report.expense?.[currency] || 0;
  if (!(before > 0.005) && !(now > 0.005)) return null;
  const period = compareLabel(p, [parseYmd(c.from), parseYmd(c.to)]);
  return html`<div class="compare-line">
    <span class="muted">${t("reports.vsPeriod", { period, sum: money(before, currency) })}</span>
    <${Delta} now=${now} before=${before} />
  </div>`;
}

function Categories({ report, currency }) {
  const rows = report.categories.map(c => ({ ...c, v: c.totals?.[currency] || 0 })).filter(c => c.v > 0).sort((a, b) => b.v - a.v);
  if (!rows.length) return null;
  const sum = rows.reduce((a, c) => a + c.v, 0);
  return html`<div class="card">
    <div class="card-title">${t("reports.byCategory", { cur: currency })}</div>
    ${rows.map(c => html`<div class="bar-row">
      <div class="bar-meta"><span>${c.emoji} ${c.name} <span class="muted">· ${c.count}</span></span><span class="muted">${money(c.v, currency)} · ${Math.round((c.v / sum) * 100)}%${report.compare && html` <${Delta} now=${c.v} before=${report.compare.categories?.[c.id]?.[currency] || 0} />`}</span></div>
      <div class="bar"><span style=${`width:${Math.max(3, (c.v / rows[0].v) * 100)}%;background:${c.color}`}></span></div>
    </div>`)}
  </div>`;
}

/** Search in all operations, or in the period on screen: words of a note or category, or an amount. */
function Search({ period, onOpen }) {
  const [q, setQ] = useState(state.reportSearch || "");
  const [inPeriod, setInPeriod] = useState(false);
  const [found, setFound] = useState(null);
  const [error, setError] = useState(null);
  const home = useStore().home;
  useEffect(() => {
    state.reportSearch = q;
    if (!q.trim()) return setFound(null);
    let live = true;
    const timer = setTimeout(() => {
      search(q.trim(), inPeriod ? period : null)
        .then(r => live && (setFound(r), setError(null)))
        .catch(e => live && setError(errorText(e)));
    }, 300);
    return () => { live = false; clearTimeout(timer); };
  }, [q, inPeriod, inPeriod && ymd(period[0]), inPeriod && ymd(period[1]), home]);
  return html`<div class="search">
    <input class="field" type="search" placeholder=${t("reports.searchHint")} value=${q} autofocus
      onInput=${e => setQ(e.currentTarget.value)} enterkeyhint="search" />
    <label class="date-row switch-row" onClick=${() => setInPeriod(!inPeriod)}>
      <span class="date-label">${t("reports.searchPeriod")}</span>
      <span class=${"switch" + (inPeriod ? " on" : "")}></span>
    </label>
    ${error ? html`<${Empty}>${error}<//>`
      : found && !found.transactions.length ? html`<${Empty}>${t("reports.nothing")}<//>`
      : found && html`
        <div class="section-title">${t("cat.operations", { n: found.transactions.length })}</div>
        <${OperationList} ops=${found.transactions} showCategory onOpen=${onOpen} />
        ${found.more && html`<div class="muted small">${t("reports.more")}</div>`}`}
  </div>`;
}

export function Reports() {
  const [p, setP] = useState(period());
  const [report, setReport] = useState(null);
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(false);
  const [searching, setSearching] = useState(!!state.reportSearching);
  const toggleSearch = () => {
    state.reportSearching = !searching;
    setSearching(!searching);
  };
  const openOp = op => openSheet("editOperation", { op, categoryId: op.category_id });
  const set = next => {
    state.report = next;
    setP(next);
  };
  const [from, to] = range(p);
  // An operation changed or deleted (the home screen reloads after it) shows in the report too.
  const home = useStore().home;
  useEffect(() => {
    let live = true;
    setLoading(true);
    setError(null);
    loadReport(from, to, compareRange(p, from, to))
      .then(r => live && setReport(r))
      .catch(e => live && setError(errorText(e)))
      .finally(() => live && setLoading(false));
    return () => (live = false);
  }, [ymd(from), ymd(to), home]);
  const swipe = useSwipe(n => p.mode !== "custom" && set(shift(p, n)));

  const currencies = report ? usedCurrencies(report.expense) : [];
  const currency = currencies.includes(p.currency) ? p.currency : currencies[0];
  const setMode = mode => {
    if (mode === "custom") set({ ...p, mode, from: ymd(from), to: ymd(to) });
    else set({ ...p, mode, anchor: p.mode === "custom" ? parseYmd(p.to) : p.anchor });
  };

  return html`<div ...${swipe}>
    <div class="title-row">
      <h1 class="screen-title">${t("reports.title")}</h1>
      <button type="button" class=${"icon-btn" + (searching ? " active" : "")} onClick=${toggleSearch} aria-label=${t("reports.search")}>${searching ? "✕" : "🔍"}</button>
    </div>
    ${p.mode === "custom"
      ? html`<div class="inline-form">
          <input type="date" class="field" value=${p.from} onChange=${e => e.currentTarget.value && set({ ...p, from: e.currentTarget.value })} aria-label=${t("reports.from")} />
          <span class="muted">—</span>
          <input type="date" class="field" value=${p.to} onChange=${e => e.currentTarget.value && set({ ...p, to: e.currentTarget.value })} aria-label=${t("reports.to")} />
        </div>`
      : html`<div class="monthbar">
          <button type="button" class="icon-btn" onClick=${() => set(shift(p, -1))} aria-label=${t("nav.prev")}>‹</button>
          <span class="monthbar-title">${label(p)}</span>
          <button type="button" class="icon-btn" onClick=${() => set(shift(p, 1))} aria-label=${t("nav.next")}>›</button>
        </div>`}
    <${Seg} options=${[["day", t("reports.day")], ["week", t("reports.week")], ["month", t("reports.month")], ["year", t("reports.year")], ["custom", t("reports.custom")]]} value=${p.mode} onChange=${setMode} />
    ${searching && html`<${Search} period=${[from, to]} onOpen=${openOp} />`}
    <div class=${(loading && report ? "dim" : "") + (searching ? " hidden" : "")}>
      ${error ? html`<${Empty}>${error}<//>` : !report ? html`<${Empty}>${t("common.loading")}<//>` : !report.transactions.length ? html`<${Empty}>${t("reports.empty")}<//>` : html`
        <div class="tiles">
          ${[["expense", t("kind.expenses")], ["income", t("kind.income")], ["savings", t("kind.savings")]].map(([k, name]) => html`<div class=${"tile tile-" + k}>
            <div class="tile-label">${name}</div>
            <div class="tile-value">${usedCurrencies(report[k]).length ? usedCurrencies(report[k]).map(c => html`<span>${money(report[k][c], c)}</span>`) : html`<span class="muted">0</span>`}</div>
          </div>`)}
        </div>
        ${currency && html`<${CompareLine} p=${p} report=${report} currency=${currency} />`}
        <${InMain} report=${report} />
        ${currencies.length > 1 && html`<${Chips} items=${currencies.map(c => [c, c])} value=${currency} onChange=${c => set({ ...p, currency: c })} />`}
        ${currency && html`<div class="report-charts">
          <${Bars} p=${p} report=${report} currency=${currency} from=${from} to=${to} />
          <${Categories} report=${report} currency=${currency} />
        </div>`}
        <div class="section-title">${t("cat.operations", { n: report.transactions.length })}</div>
        <${OperationList} ops=${report.transactions} showCategory onOpen=${openOp} />`}
    </div>
  </div>`;
}
