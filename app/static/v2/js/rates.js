// Exchange rates, for people who turn them on in Settings. The choice lives on this device:
// not everyone needs them, and nothing in the budget is converted.

import { api } from "./api.js";
import { state, update } from "./store.js";

const KEY = "budget:v2:rates";
export const SOURCES = ["tcmb", "cbr", "ecb", "fawaz"];
/** Codes for links to Google. */
const CODE = { "₺": "TRY", "$": "USD", "€": "EUR", "₽": "RUB" };

export function loadPrefs() {
  try {
    const p = JSON.parse(localStorage.getItem(KEY) || "null");
    if (p && typeof p === "object") return { on: !!p.on, source: SOURCES.includes(p.source) ? p.source : "tcmb", show: Array.isArray(p.show) ? p.show : ["₽", "$"] };
  } catch {}
  return { on: false, source: "tcmb", show: ["₽", "$"] };
}

export function setPrefs(patch) {
  const prefs = { ...state.ratesPrefs, ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {}
  const sourceChanged = prefs.source !== state.ratesPrefs.source;
  update({ ratesPrefs: prefs, rates: sourceChanged ? null : state.rates });
  if (prefs.on) refreshRates();
}

/** Fetches the rates of the chosen source when they are on; failures leave the screen as is. */
export async function refreshRates() {
  const { on, source } = state.ratesPrefs;
  if (!on) return;
  try {
    const rates = await api(`/api/rates?source=${source}`);
    if (state.ratesPrefs.source === source) update({ rates, ratesError: false });
  } catch {
    update({ ratesError: true });
  }
}

/** `amount` of `from` in `to`, or null when the source lacks one of them. */
export function convert(amount, from, to, rates) {
  const a = rates?.in_try?.[from], b = rates?.in_try?.[to];
  return a && b ? (amount * a) / b : null;
}

export function googleLink(from, to) {
  return `https://www.google.com/search?q=${encodeURIComponent(`1 ${CODE[from] || from} to ${CODE[to] || to}`)}`;
}
