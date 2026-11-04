// Small building blocks shared by the screens.

import { html, useEffect, useRef, useState } from "../vendor/preact-htm.js";
import { t } from "./i18n.js";
import { money, ymd, today, myCurrencies, allCurrencies, currencyName, currencySymbol, codeOf } from "./format.js";
import { evalAmount, isExpression } from "./calc.js";
import { state, closeSheet } from "./store.js";
import { haptic } from "./tg.js";

export function Toast({ toast }) {
  if (!toast) return null;
  return html`<div class="toast toast-${toast.kind}" key=${toast.id} role="status">${toast.text}</div>`;
}

/** Bottom sheet over the screen; a tap outside or Telegram's Back closes it. */
// Sheets and lists sit in the part of the screen that is visible: above the phone keyboard
// when it is open. Browsers move a fixed bottom panel differently when the keyboard opens;
// with these two values every panel stays put and nothing jumps.
(function trackViewport() {
  const vv = window.visualViewport;
  if (!vv) return;
  const set = () => {
    const root = document.documentElement.style;
    root.setProperty("--vvh", `${vv.height}px`);
    root.setProperty("--vvtop", `${vv.offsetTop}px`);
  };
  set();
  vv.addEventListener("resize", set);
  vv.addEventListener("scroll", set);
})();

/** A panel from the bottom. `tall`: a constant height, for sheets whose content shrinks as you type (search). */
export function Sheet({ title, children, onClose = closeSheet, tall }) {
  useEffect(() => {
    document.body.classList.add("sheet-open");
    return () => document.body.classList.remove("sheet-open");
  }, []);
  return html`
    <div class="sheet-backdrop" onClick=${onClose}>
      <div class=${"sheet" + (tall ? " sheet-tall" : "")} role="dialog" aria-modal="true" aria-label=${title} onClick=${e => e.stopPropagation()}>
        <div class="sheet-handle"></div>
        ${title && html`<div class="sheet-title">${title}</div>`}
        <div class="sheet-body">${children}</div>
      </div>
    </div>`;
}

/**
 * A tap on a choice button keeps the focus where it is: with the amount field focused, the
 * phone keyboard stays up instead of closing and jumping back over the form.
 */
const keepFocus = e => e.preventDefault();

/** Segmented switch: options are [value, label] pairs. */
export function Seg({ options, value, onChange }) {
  return html`<div class="seg">
    ${options.map(([v, label]) => html`<button type="button" class=${"seg-btn" + (v === value ? " active" : "")} onPointerDown=${keepFocus}
      onClick=${() => { if (v !== value) { haptic(); onChange(v); } }}>${label}</button>`)}
  </div>`;
}

/** Choice buttons; `active` lists the chosen ones when several can be on at once. */
export function Chips({ items, value, onChange, extra, active }) {
  return html`<div class="chips">
    ${items.map(([v, label]) => html`<button type="button" class=${"chip" + (v === value || active?.includes(v) ? " active" : "")} onPointerDown=${keepFocus}
      onClick=${() => { haptic(); onChange(v); }}>${label}</button>`)}
    ${extra}
  </div>`;
}

/**
 * Amount with a currency. Sums can be typed right in the field ("357+578"); the result
 * is shown dimmed inside it and is what gets saved.
 */
export function MoneyField({ value, onInput, currency, onCurrency, autofocus, onEnter, locked }) {
  const ref = useRef(null);
  const [picking, setPicking] = useState(false);
  useEffect(() => {
    if (autofocus) setTimeout(() => ref.current?.focus(), 250);
  }, []);
  const result = evalAmount(value);
  const preview = isExpression(value) && Number.isFinite(result) ? `= ${money(result)}` : "";
  const currencies = [...new Set([...myCurrencies(state), currency].filter(Boolean))];
  const [searching, setSearching] = useState(false);
  return html`
    <div class="money">
      <div class="money-row">
        <div class="money-input-wrap">
          <input ref=${ref} class="field money-input" inputmode="text" autocomplete="off" enterkeyhint="done"
            placeholder=${t("form.amountPlaceholder")} value=${value}
            style=${preview ? `padding-right:${preview.length * 10 + 24}px` : ""}
            onInput=${e => onInput(e.currentTarget.value)}
            onKeyDown=${e => { if (e.key === "Enter") { e.preventDefault(); onEnter?.(); } }} />
          ${preview && html`<span class="money-preview">${preview}</span>`}
        </div>
        <button type="button" class="field money-currency" disabled=${locked} onPointerDown=${keepFocus} onClick=${() => setPicking(!picking)} aria-label=${t("form.currency")}>${currencySymbol(currency)}</button>
      </div>
      ${picking && html`<div class="chips chips-tight">
        ${currencies.map(c => html`<button type="button" class=${"chip" + (c === currency ? " active" : "")} onPointerDown=${keepFocus}
          onClick=${() => { haptic(); onCurrency(c); setPicking(false); }}>${currencySymbol(c)}</button>`)}
        <button type="button" class="chip" onClick=${() => setSearching(true)} aria-label=${t("currency.other")}>…</button>
      </div>`}
      ${searching && html`<${CurrencyPicker} value=${currency} onPick=${c => { onCurrency(c); setSearching(false); setPicking(false); }} onClose=${() => setSearching(false)} />`}
    </div>`;
}

export function DateField({ value, onChange }) {
  const isToday = value === ymd(today());
  return html`<label class="date-row">
    <span class="date-label">📅 ${t("form.date")}</span>
    <input type="date" class="field date-input" value=${value} onChange=${e => e.currentTarget.value && onChange(e.currentTarget.value)} />
    ${!isToday && html`<button type="button" class="link-btn" onClick=${e => { e.preventDefault(); onChange(ymd(today())); }}>${t("form.today")}</button>`}
  </label>`;
}

/** Row of a grouped list, like in phone settings. */
export function Row({ icon, title, subtitle, right, onClick, tone, chevron = true }) {
  return html`<button type="button" class=${"row" + (tone ? " row-" + tone : "")} onClick=${onClick}>
    ${icon !== undefined && html`<span class="row-icon">${icon}</span>`}
    <span class="row-text">
      <span class="row-title">${title}</span>
      ${subtitle && html`<span class="row-sub">${subtitle}</span>`}
    </span>
    ${right !== undefined && html`<span class="row-right">${right}</span>`}
    ${chevron && html`<span class="row-chevron">›</span>`}
  </button>`;
}

export function Empty({ children }) {
  return html`<div class="empty">${children}</div>`;
}

/** Remembers the last currency used, so the next form starts with it. */
export function lastCurrency() {
  try {
    return localStorage.getItem(`budget:v2:currency:${state.me?.id}`) || "₺";
  } catch {
    return "₺";
  }
}

export function rememberCurrency(c) {
  try {
    localStorage.setItem(`budget:v2:currency:${state.me?.id}`, c);
  } catch {}
}

/** Every currency, searchable by code or name, as a list over the sheet. */
export function CurrencyPicker({ value, onPick, onClose, title }) {
  // A constant height: the search field stays where it is while the list gets shorter.
  const [q, setQ] = useState("");
  const mine = myCurrencies(state);
  const all = allCurrencies();
  const needle = q.trim().toLowerCase();
  const fits = id => !needle || codeOf(id).toLowerCase().includes(needle) || id.toLowerCase().includes(needle) || currencyName(id).toLowerCase().includes(needle);
  const rest = all.filter(id => !mine.includes(id)).filter(fits);
  const row = id => html`<button type="button" key=${id} class=${"picker-row" + (id === value ? " active" : "")} onClick=${() => { haptic(); onPick(id); }}>
    <span class=${"pr-icon" + (currencySymbol(id).length > 2 ? " pr-code" : "")}>${currencySymbol(id)}</span>
    <span class="pr-text"><span class="pr-name">${currencyName(id)}</span><span class="muted small">${codeOf(id)}</span></span>
    <span class="pr-check">${id === value ? "✓" : ""}</span>
  </button>`;
  return html`<div class="picker-backdrop" onClick=${onClose}>
    <div class="picker picker-search" onClick=${e => e.stopPropagation()}>
      <div class="picker-title">${title || t("currency.choose")}</div>
      <input class="field" placeholder=${t("currency.search")} value=${q} onInput=${e => setQ(e.currentTarget.value)} autocomplete="off" />
      <div class="picker-list">
        ${mine.filter(fits).length > 0 && html`<div class="picker-group">${t("currency.mine")}</div>${mine.filter(fits).map(row)}`}
        ${rest.length > 0 && html`<div class="picker-group">${t("currency.all")}</div>${rest.slice(0, 200).map(row)}`}
      </div>
    </div>
  </div>`;
}

/** A field with the chosen currency; tapping it opens the list of all currencies. */
export function CurrencySelect({ value, onChange, title }) {
  const [open, setOpen] = useState(false);
  return html`<div class="account-select">
    <button type="button" class="field account-field" onClick=${() => { haptic(); setOpen(true); }}>
      <span class="af-main">${currencySymbol(value)} · ${currencyName(value)}</span>
      <span class="af-arrow">›</span>
    </button>
    ${open && html`<${CurrencyPicker} value=${value} title=${title} onPick=${c => { onChange(c); setOpen(false); }} onClose=${() => setOpen(false)} />`}
  </div>`;
}

/**
 * A field showing the chosen item; tapping it opens the list of items over the sheet.
 * `items`: [{ value, label, hint? }].
 */
export function ListSelect({ items, value, onChange, label, title }) {
  const [open, setOpen] = useState(false);
  const cur = items.find(i => i.value === value);
  return html`<div class="account-select">
    <button type="button" class="field account-field af-inside" onClick=${() => { haptic(); setOpen(true); }}>
      ${label && html`<span class="af-caption">${label}</span>`}
      <span class="af-main">${cur ? cur.label : "—"}</span>
      <span class="af-arrow">›</span>
    </button>
    ${open && html`<div class="picker-backdrop" onClick=${() => setOpen(false)}>
      <div class="picker" onClick=${e => e.stopPropagation()}>
        <div class="picker-title">${title || label}</div>
        <div class="picker-list">
          ${items.map(i => html`<button type="button" key=${i.value} class=${"picker-row" + (i.value === value ? " active" : "")}
            onClick=${() => { haptic(); setOpen(false); if (i.value !== value) onChange(i.value); }}>
            <span class="pr-text"><span class="pr-name">${i.label}</span>${i.hint && html`<span class="muted small">${i.hint}</span>`}</span>
            <span class="pr-check">${i.value === value ? "✓" : ""}</span>
          </button>`)}
        </div>
      </div>
    </div>`}
  </div>`;
}

/** Sources of exchange rates as list items, with what each one has. */
export function sourceItems(sources) {
  return sources.map(x => ({ value: x, label: t(x === "off" ? "budgetRates.off" : "rates.src." + x), hint: t("rates.about." + x) }));
}
