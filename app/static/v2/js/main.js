// Start-up and the app frame: the screen for the route, the tab bar, sheets and toasts.

import { html, render, useEffect } from "../vendor/preact-htm.js";
import { initTelegram, haptic } from "./tg.js";
import { api, signIn } from "./api.js";
import { t, errorText } from "./i18n.js";
import { state, update, useStore, toast } from "./store.js";
import { startRouter, navigate, syncBack, back, TABS } from "./router.js";
import { cachedHome, refresh } from "./data.js";
import { loadPrefs, refreshRates } from "./rates.js";
import { Toast } from "./ui.js";
import { Home } from "./screens/home.js";
import { Category } from "./screens/category.js";
import { Reports } from "./screens/reports.js";
import { Manage, Settings } from "./screens/manage.js";
import { SheetHost } from "./screens/sheets.js";
import { AuthScreen } from "./auth.js";

const TAB_ICONS = { home: "🏠", reports: "📊", manage: "🗂", settings: "⚙️" };

function TabBar({ route }) {
  // On a phone a category opens full screen without the bar; on a wide screen the bar stays as a side menu.
  return html`<nav class=${"tabbar" + (route.name === "category" ? " tabbar-detail" : "")}>
    ${TABS.map(tab => html`<button type="button" class=${"tab" + (route.name === tab || (route.name === "category" && tab === "home") ? " active" : "")}
      onClick=${() => { haptic(); navigate({ name: tab }); }}>
      <span class="tab-icon">${TAB_ICONS[tab]}</span><span class="tab-label">${t("tab." + tab)}</span>
    </button>`)}
  </nav>`;
}

function Fatal({ kind }) {
  if (kind === "signed-out") {
    return html`<div class="fatal">
      <div class="fatal-icon">👋</div>
      <div class="fatal-title">${t("fatal.signedOut")}</div>
      <button type="button" class="btn btn-primary" onClick=${() => location.reload()}>${t("fatal.signInAgain")}</button>
    </div>`;
  }
  if (kind === "not-invited") {
    return html`<div class="fatal">
      <div class="fatal-icon">🔒</div>
      <div class="fatal-title">${t("fatal.notInvited")}</div>
      <div class="muted">${t("fatal.notInvitedHint")}</div>
    </div>`;
  }
  return html`<div class="fatal">
    <div class="fatal-icon">⚠️</div>
    <div class="fatal-title">${t("fatal.cantStart")}</div>
    <div class="muted">${kind}</div>
    <button type="button" class="btn btn-primary" onClick=${() => location.reload()}>${t("fatal.retry")}</button>
  </div>`;
}

function App() {
  const s = useStore();
  useEffect(syncBack, [s.route, !!s.sheet]);
  if (s.fatal) return html`<${Fatal} kind=${s.fatal} />`;
  if (s.auth) return html`<${AuthScreen} key=${s.auth.mode} mode=${s.auth.mode} token=${s.auth.token} onDone=${signedIn} />`;
  if (s.booting) return html`<div class="boot"><div class="spinner"></div></div>`;
  const r = s.route;
  const screen = r.name === "category" ? html`<${Category} key=${r.id} id=${r.id} />`
    : r.name === "reports" ? html`<${Reports} />`
    : r.name === "manage" ? html`<${Manage} />`
    : r.name === "settings" ? html`<${Settings} />`
    : html`<${Home} />`;
  return html`
    ${!s.online && html`<div class="offline">${t("err.offline")}</div>`}
    <main class=${"screen" + (r.name === "category" ? "" : " with-tabs") + (r.name === "home" || r.name === "reports" ? " screen-wide" : "")}>${screen}</main>
    <${TabBar} route=${r} />
    <${SheetHost} />
    <${Toast} toast=${s.toast} />`;
}

// The server's answer to someone nobody has added to a budget yet.
const NOT_INVITED = "Вас ещё не пригласили в бюджет";

/** Loads the budget once someone is signed in, however they signed in. */
async function signedIn(me) {
  update({ auth: null, me, home: null });
  update({ home: cachedHome(state.month), booting: false });
  refreshRates();
  await refresh();
}

async function boot() {
  initTelegram();
  update({ ratesPrefs: loadPrefs() });
  // An invitation link opens as #/invite/<token>; it is read before the router resets the address.
  const invite = /^#\/invite\/([0-9a-f]{16,})$/.exec(location.hash)?.[1];
  startRouter();
  render(html`<${App} />`, document.getElementById("app"));
  try {
    const server = await api("/api/setup").catch(() => ({}));
    update({ server });
    // An existing session is reused; signing in happens only when there is none.
    let me;
    try {
      me = await api("/api/me");
    } catch (e) {
      if (e.status !== 401) throw e;
      try {
        me = await signIn();
      } catch (err) {
        if (err.message !== "need-login") throw err;
        // A browser: the owner is created first, an invitation leads to joining, else sign in.
        const mode = invite ? "invite" : server.needs_setup ? "setup" : "login";
        return update({ auth: { mode, token: invite }, booting: false });
      }
    }
    await signedIn(me);
  } catch (e) {
    const kind = e.status === 403 && e.message === NOT_INVITED ? "not-invited" : errorText(e);
    update({ fatal: kind, booting: false });
  }
}

window.addEventListener("online", () => {
  update({ online: true });
  toast(t("common.online"));
  refresh().catch(() => {});
});
window.addEventListener("offline", () => update({ online: false }));
// In a browser Escape does what Telegram's Back button does: closes a sheet, then a screen.
window.addEventListener("keydown", e => {
  if (e.key === "Escape" && !state.auth && !state.fatal) back();
});
// Coming back to the app shows what others have added meanwhile.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && state.me) refresh().catch(() => {});
});

boot();
