// Money, dates and periods, written in the interface language.

import { locale } from "./i18n.js";

/** Currencies offered in forms; any other one already in the data is shown as is. */
export const CURRENCIES = ["₺", "$", "€", "₽"];

export function money(value, currency = "") {
  const n = Math.round((Number(value) || 0) * 100) / 100;
  return `${n.toLocaleString(locale(), { maximumFractionDigits: 2 })} ${currency}`.trim();
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
