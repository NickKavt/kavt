// Manage: categories (open the editor), people of the budget; Settings: language, sign-in.

import { html } from "../../vendor/preact-htm.js";
import { t, LANGS, currentLang, setLang, errorText } from "../i18n.js";
import { state, useStore, openSheet, update, toast } from "../store.js";
import { endOtherSessions, signOut, exportCsv, savePrefs } from "../data.js";
import { ask, haptic, telegramUser, openLink, tg } from "../tg.js";
import { Row, Chips, lastCurrency, ListSelect, sourceItems } from "../ui.js";
import { myCurrencies } from "../format.js";
import { SOURCES, setPrefs } from "../rates.js";
import { RatesLine } from "./rates-ui.js";

export function Manage() {
  const s = useStore();
  const cats = (s.home?.categories || []).filter(c => c.type === "expense" || c.type === "income");
  const isAdmin = s.me?.role === "admin" || s.me?.role === "owner";
  const download = async () => {
    try {
      await exportCsv();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return html`<div>
    <h1 class="screen-title">${t("manage.title")}</h1>
    <div class="section-title">${t("manage.categories")}</div>
    <div class="muted small">${t("manage.hint")}</div>
    <div class="group">
      ${cats.map(c => html`<${Row} key=${c.id} icon=${c.emoji} title=${c.name} right=${html`<span class="dot" style=${`background:${c.color}`}></span>`}
        onClick=${() => openSheet("categoryEditor", { id: c.id })} />`)}
      <${Row} icon="＋" title=${t("manage.addCategory")} tone="accent" onClick=${() => openSheet("categoryEditor", {})} />
    </div>
    <div class="section-title">${t("manage.budget")}</div>
    <div class="group">
      <${Row} icon="💳" title=${t("accounts.title")} subtitle=${t("accounts.sub")} onClick=${() => openSheet("moneyAccounts", {})} />
      ${isAdmin
        ? html`<${Row} icon="👥" title=${t("users.title")} onClick=${() => openSheet("users", {})} />`
        : html`<div class="row muted">${t("manage.membersOnlyAdmins")}</div>`}
      ${isAdmin && html`<${Row} icon="💱" title=${t("budgetRates.title")}
        subtitle=${s.home?.settings ? `${s.home.settings.main_currency} · ${s.home.settings.rate_source === "off" ? t("budgetRates.off") : t("rates.src." + s.home.settings.rate_source)}` : ""}
        onClick=${() => openSheet("budgetRates", {})} />`}
    </div>
    <div class="section-title">${t("manage.data")}</div>
    <div class="group">
      <${Row} icon="🔁" title=${t("recurring.title")} subtitle=${t("recurring.sub")} onClick=${() => openSheet("recurring", {})} />
      <${Row} icon="⬇️" title=${t("export.title")} subtitle=${t("export.sub")} onClick=${download} />
      <${Row} icon="⬆️" title=${t("import.title")} subtitle=${t("import.sub")} onClick=${() => openSheet("import", {})} />
    </div>
  </div>`;
}

// The page carries the address of this copy's source code (AGPL: users may ask for it).
const SOURCE = document.querySelector('meta[name="source"]')?.content.replace("__SOURCE__", "") || "";
const DONATE = readDonate();

function readDonate() {
  try {
    const d = JSON.parse(document.querySelector('meta[name="donate"]')?.content || "{}");
    return d.ton || d.github || d.url ? d : null;
  } catch {
    return null;
  }
}

export function Settings() {
  const s = useStore();
  const u = telegramUser() || s.me || {};
  const name = [u.first_name, u.last_name].filter(Boolean).join(" ") || (u.username ? "@" + u.username : "");
  const pick = code => {
    if (code === currentLang()) return;
    haptic();
    setLang(code).then(() => update({}));
  };
  const others = async () => {
    if (!(await ask(t("settings.confirmOthers")))) return;
    try {
      await endOtherSessions();
      toast(t("settings.othersEnded"));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const out = async () => {
    if (!(await ask(t("settings.confirmSignOut")))) return;
    try {
      await signOut();
    } catch {}
    // In a browser the sign-in form comes back; Telegram would sign straight in again.
    if (tg?.initData) update({ fatal: "signed-out", me: null });
    else update({ auth: { mode: state.server?.demo ? "demo" : "login" }, me: null, home: null });
  };
  return html`<div>
    <h1 class="screen-title">${t("settings.title")}</h1>
    ${name && html`<div class="muted who">${t("settings.signedInAs")} <b>${name}</b></div>`}
    <div class="settings-lang"><${ListSelect} items=${Object.entries(LANGS).map(([code, l]) => ({ value: code, label: l.name }))} value=${currentLang()} onChange=${pick}
      label=${t("settings.language")} /></div>
    <div class="section-title">${t("currency.sectionRates")}</div>
    <div class="group">
      <${Row} icon="💱" title=${t("currency.myTitle")} subtitle=${myCurrencies(s).join(" ")} onClick=${() => openSheet("myCurrencies", {})} />
      <${Row} icon="≈" title=${t("currency.showConverted")} subtitle=${t("currency.showConvertedHint")} chevron=${false}
        right=${html`<span class=${"switch" + (s.me?.show_converted !== false ? " on" : "")}></span>`}
        onClick=${async () => { haptic(); const v = s.me?.show_converted === false; try { await savePrefs({ show_converted: v }); update({ me: { ...s.me, show_converted: v } }); } catch (e) { toast(errorText(e), "error"); } }} />
      <${Row} icon="💱" title=${t("rates.toggle")} subtitle=${t("rates.toggleHint")} chevron=${false}
        right=${html`<span class=${"switch" + (s.ratesPrefs.on ? " on" : "")}></span>`} onClick=${() => { haptic(); setPrefs({ on: !s.ratesPrefs.on }); }} />
    </div>
    ${s.ratesPrefs.on && html`<div class="card form rates-settings">
      <${ListSelect} items=${sourceItems(SOURCES)} value=${s.ratesPrefs.source} onChange=${source => setPrefs({ source })} label=${t("rates.source")} />
      ${s.ratesPrefs.source === "ecb" && html`<div class="muted small">${t("rates.ecbNoRub")}</div>`}
      <div class="muted small">${t("rates.show")}</div>
      <${Chips} items=${myCurrencies(s).map(c => [c, c])} value=${null} active=${s.ratesPrefs.show}
        onChange=${c => setPrefs({ show: s.ratesPrefs.show.includes(c) ? s.ratesPrefs.show.filter(x => x !== c) : [...s.ratesPrefs.show, c] })} />
      <${RatesLine} base=${lastCurrency()} />
    </div>`}
    <div class="section-title">${t("settings.signIn")}</div>
    <div class="group">
      ${s.me?.has_password && html`<${Row} icon="🔑" title=${t("auth.changePassword")} subtitle=${t("auth.loginIs", { login: s.me.login })} onClick=${() => openSheet("password", {})} />`}
      ${s.me?.has_password && html`<${Row} icon="🛡️" title=${t("tfa.title")} subtitle=${s.me.totp ? t("tfa.isOn") : t("tfa.isOff")} onClick=${() => openSheet("twoFactor", {})} />`}
      <${Row} icon="📱" title=${t("sessions.title")} onClick=${() => openSheet("sessions", {})} />
      <${Row} icon="🔐" title=${t("settings.signOutOthers")} onClick=${others} />
      <${Row} icon="↩️" title=${t("settings.signOut")} tone="danger" onClick=${out} />
    </div>
    ${SOURCE && html`<div class="section-title">${t("settings.about")}</div>
    <div class="group">
      ${DONATE && html`<${Row} icon="💛" title=${t("donate.title")} subtitle=${t("donate.sub")} onClick=${() => openSheet("donate", { ways: DONATE })} />`}
      <${Row} icon="📜" title=${t("settings.source")} subtitle=${t("settings.license")} onClick=${() => openLink(SOURCE)} />
    </div>`}
  </div>`;
}
