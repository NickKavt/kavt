// Screens are addressed by the hash: #/ home, #/reports, #/manage, #/settings, #/c/12.
// Each history entry remembers how deep it is, so Back and the Home tab always lead
// home without leaving stray entries behind.

import { state, update, closeSheet } from "./store.js";
import { setBack } from "./tg.js";

export const TABS = ["home", "reports", "manage", "settings"];

function parse(hash) {
  const path = hash.replace(/^#/, "") || "/";
  const cat = /^\/c\/(\d+)$/.exec(path);
  if (cat) return { name: "category", id: Number(cat[1]) };
  const name = path.slice(1);
  return TABS.includes(name) ? { name } : { name: "home" };
}

const depth = () => history.state?.depth || 0;

function pathOf(route) {
  return route.name === "category" ? `/c/${route.id}` : route.name === "home" ? "/" : `/${route.name}`;
}

export function navigate(route) {
  const from = state.route.name;
  if (route.name === "home") return goHome();
  if (pathOf(route) === pathOf(state.route)) return;
  // Moving between tabs replaces the entry; anything else is a step deeper.
  const replace = TABS.includes(from) && from !== "home" && TABS.includes(route.name);
  const d = replace ? depth() : depth() + 1;
  history[replace ? "replaceState" : "pushState"]({ depth: d }, "", "#" + pathOf(route));
  apply(route);
}

export function goHome() {
  if (depth() > 0) history.go(-depth());
  else apply({ name: "home" });
}

export function back() {
  if (state.sheet) return closeSheet();
  if (depth() > 0) history.back();
  else goHome();
}

function apply(route) {
  update({ route, sheet: null });
  window.scrollTo(0, 0);
  syncBack();
}

/** Telegram's Back button: on everything but the home screen, and over any sheet. */
export function syncBack() {
  setBack(state.route.name !== "home" || state.sheet ? back : null);
}

export function startRouter() {
  // A fresh start always begins at home; a reload keeps the screen it was on.
  if (!history.state) history.replaceState({ depth: 0 }, "", "#/");
  update({ route: parse(location.hash) });
  window.addEventListener("popstate", () => apply(parse(location.hash)));
  syncBack();
}
