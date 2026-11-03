// Small building blocks shared by the screens.

import { html, useEffect, useRef, useState } from "../vendor/preact-htm.js";
import { t } from "./i18n.js";
import { CURRENCIES, money, ymd, today } from "./format.js";
import { evalAmount, isExpression } from "./calc.js";
import { state, closeSheet } from "./store.js";
import { haptic } from "./tg.js";

export function Toast({ toast }) {
  if (!toast) return null;
  return html`<div class="toast toast-${toast.kind}" key=${toast.id} role="status">${toast.text}</div>`;
}

/** Bottom sheet over the screen; a tap outside or Telegram's Back closes it. */
export function Sheet({ title, children, onClose = closeSheet }) {
  useEffect(() => {
    document.body.classList.add("sheet-open");
    return () => document.body.classList.remove("sheet-open");
  }, []);
  return html`
    <div class="sheet-backdrop" onClick=${onClose}>
      <div class="sheet" role="dialog" aria-modal="true" aria-label=${title} onClick=${e => e.stopPropagation()}>
        <div class="sheet-handle"></div>
        ${title && html`<div class="sheet-title">${title}</div>`}
        <div class="sheet-body">${children}</div>
      </div>
    </div>`;
}

/** Segmented switch: options are [value, label] pairs. */
export function Seg({ options, value, onChange }) {
  return html`<div class="seg">
    ${options.map(([v, label]) => html`<button type="button" class=${"seg-btn" + (v === value ? " active" : "")}
      onClick=${() => { if (v !== value) { haptic(); onChange(v); } }}>${label}</button>`)}
  </div>`;
}

/** Choice buttons; `active` lists the chosen ones when several can be on at once. */
export function Chips({ items, value, onChange, extra, active }) {
  return html`<div class="chips">
    ${items.map(([v, label]) => html`<button type="button" class=${"chip" + (v === value || active?.includes(v) ? " active" : "")}
      onClick=${() => { haptic(); onChange(v); }}>${label}</button>`)}
    ${extra}
  </div>`;
}

/**
 * Amount with a currency. Sums can be typed right in the field ("357+578"); the result
 * is shown dimmed inside it and is what gets saved.
 */
export function MoneyField({ value, onInput, currency, onCurrency, autofocus, onEnter }) {
  const ref = useRef(null);
  const [picking, setPicking] = useState(false);
  useEffect(() => {
    if (autofocus) setTimeout(() => ref.current?.focus(), 250);
  }, []);
  const result = evalAmount(value);
  const preview = isExpression(value) && Number.isFinite(result) ? `= ${money(result)}` : "";
  const currencies = [...new Set([...CURRENCIES, currency])];
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
        <button type="button" class="field money-currency" onClick=${() => setPicking(!picking)} aria-label=${t("form.currency")}>${currency}</button>
      </div>
      ${picking && html`<div class="chips chips-tight">
        ${currencies.map(c => html`<button type="button" class=${"chip" + (c === currency ? " active" : "")}
          onClick=${() => { haptic(); onCurrency(c); setPicking(false); }}>${c}</button>`)}
      </div>`}
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
