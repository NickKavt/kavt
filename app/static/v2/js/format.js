// Money, dates and periods, written in the interface language.

import { locale } from "./i18n.js";

/** The four currencies the app always had; their symbols are their ids. Any other is its ISO code. */
export const CURRENCIES = ["₺", "$", "€", "₽"];
const CODE = { "₺": "TRY", "$": "USD", "€": "EUR", "₽": "RUB" };
const ID = { TRY: "₺", USD: "$", EUR: "€", RUB: "₽" };
export const codeOf = id => CODE[id] || id;
export const idOf = code => ID[code] || code;

/** Every ISO currency the browser knows, as app ids; a short list where it can't tell. */
export function allCurrencies() {
  let codes;
  try {
    codes = Intl.supportedValuesOf("currency");
  } catch {
    codes = ["TRY", "USD", "EUR", "RUB", "GBP", "CHF", "JPY", "CNY", "KZT", "GEL", "AZN", "AMD", "UAH", "BYN", "UZS", "KGS", "PLN", "CZK", "SEK", "NOK", "DKK", "AED", "SAR", "ILS", "INR", "THB", "KRW", "CAD", "AUD", "BRL"];
  }
  return codes.map(idOf);
}

/** The currency's name in the interface language ("Фунт стерлингов"). */
export function currencyName(id) {
  try {
    return new Intl.DisplayNames([locale()], { type: "currency" }).of(codeOf(id)) || id;
  } catch {
    return id;
  }
}

const symbols = {};
const UNIQUE_SIGN = new Set(["GBP", "JPY", "INR", "KRW", "UAH", "KZT", "GEL", "AZN", "ILS", "THB", "VND", "NGN", "PHP", "AMD", "MNT", "BDT", "LAK", "KHR", "CRC", "PYG"]);
/** How an amount shows its currency: the four by their symbols, others by a symbol only when it
 *  can't be mistaken for another (£, ₸, ₾), else by the code. */
export function currencySymbol(id) {
  if (!id || CODE[id]) return id || "";
  if (symbols[id] !== undefined) return symbols[id];
  let sym = id;
  // Only where the sign belongs to this one currency: "£" is the pound sterling, not the
  // Falklands' or Gibraltar's pound; elsewhere the code is clearer.
  if (UNIQUE_SIGN.has(id)) {
    try {
      const narrow = new Intl.NumberFormat("en", { style: "currency", currency: id, currencyDisplay: "narrowSymbol" }).formatToParts(0).find(p => p.type === "currency")?.value;
      if (narrow && !/[A-Za-z]/.test(narrow)) sym = narrow;
    } catch {}
  }
  return (symbols[id] = sym);
}

/**
 * The currencies offered to this person in forms: their own list from Settings, else those the
 * budget uses (the main one, the accounts'), else the four.
 */
export function myCurrencies(s) {
  const own = s?.me?.currencies;
  if (own?.length) return own;
  const used = [s?.home?.settings?.main_currency, ...(s?.home?.accounts || []).filter(a => !a.archived).map(a => a.currency)].filter(Boolean);
  const list = [...new Set([...CURRENCIES.filter(c => used.includes(c)), ...used])];
  return list.length ? list : CURRENCIES;
}

export function money(value, currency = "") {
  const n = Math.round((Number(value) || 0) * 100) / 100;
  return `${n.toLocaleString(locale(), { maximumFractionDigits: 2 })} ${currencySymbol(currency)}`.trim();
}

/** Currencies of a totals map that have money, the largest first. */
export function usedCurrencies(totals) {
  return Object.entries(totals || {})
    .filter(([, v]) => Math.abs(Number(v)) > 0.004)
    .sort((a, b) => Math.abs(b[1]) - Math.abs(a[1]))
    .map(([c]) => c);
}

export function ymd(d) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function parseYmd(s) {
  const [y, m, d] = String(s).split("-").map(Number);
  return new Date(y, m - 1, d || 1);
}

export function today() {
  const d = new Date();
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

export function addDays(d, n) {
  const r = new Date(d);
  r.setDate(r.getDate() + n);
  return r;
}

export function monthStart(d) {
  return new Date(d.getFullYear(), d.getMonth(), 1);
}

export function addMonths(d, n) {
  return new Date(d.getFullYear(), d.getMonth() + n, 1);
}

export function sameMonth(a, b) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth();
}

export function daysBetween(a, b) {
  return Math.round((parseYmd(ymd(b)) - parseYmd(ymd(a))) / 86400000);
}

export const fmt = (d, opts) => d.toLocaleDateString(locale(), opts);

export function monthTitle(d) {
  const s = fmt(d, { month: "long", year: "numeric" }).replace(/\s?г\.$/, "");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function dayTitle(d) {
  const withYear = d.getFullYear() !== new Date().getFullYear();
  return fmt(d, withYear ? { day: "numeric", month: "long", year: "numeric" } : { day: "numeric", month: "long" });
}

export const weekday = d => fmt(d, { weekday: "short" }).replace(/\.$/, "");
export const shortMonth = d => fmt(d, { month: "short" }).replace(/\.$/, "");
