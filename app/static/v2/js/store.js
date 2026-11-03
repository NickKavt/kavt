// One shared state for the whole app. Components read it with useStore() and redraw
// when update() changes it; it is small enough that redrawing everything is cheap.

import { useEffect, useReducer } from "../vendor/preact-htm.js";
import { monthStart, today } from "./format.js";

export const state = {
  booting: true,
  /** Why the app cannot start: "signed-out", "not-invited" or an error text. */
  fatal: null,
  me: null,
  route: { name: "home" },
  month: monthStart(today()),
  home: null,
  homeLoading: false,
  /** Category details by id, for the month on screen. */
  details: {},
  /** The bottom sheet on top of the screen: { kind, ...props }. */
  sheet: null,
  toast: null,
  online: navigator.onLine,
  /** Exchange rates: what this device asked for (rates.js) and the latest from the server. */
  ratesPrefs: { on: false, source: "tcmb", show: ["₽", "$"] },
  rates: null,
  ratesError: false,
  /** The sign-in screen in a browser: { mode: setup | login | register | invite, token }. */
  auth: null,
  /** What the server allows: { needs_setup, telegram, signup_open }. */
  server: null,
};

const listeners = new Set();

export function update(patch) {
  Object.assign(state, typeof patch === "function" ? patch(state) : patch);
  listeners.forEach(fn => fn());
}

export function useStore() {
  const [, redraw] = useReducer(n => n + 1, 0);
  useEffect(() => {
    listeners.add(redraw);
    return () => listeners.delete(redraw);
  }, []);
  return state;
}

let toastTimer = null;

export function toast(text, kind = "ok") {
  clearTimeout(toastTimer);
  update({ toast: { text, kind, id: Date.now() } });
  toastTimer = setTimeout(() => update({ toast: null }), 2600);
}

export const openSheet = (kind, props = {}) => update({ sheet: { kind, ...props, sheetKey: Date.now() } });
export const closeSheet = () => update({ sheet: null });
