// Sign-in in a browser: creating the owner on a fresh install, signing in with a login and
// password, accepting an invitation link and, where the install allows it, a budget of one's
// own. Inside Telegram none of this is shown: Telegram signs people in.

import { html, useEffect, useState } from "../vendor/preact-htm.js";
import { t, errorText, currentLang, setLang, LANGS } from "./i18n.js";
import { api } from "./api.js";
import { state, update } from "./store.js";

// The server's answer to a right password when a code is still needed (src/totp.rs).
const CODE_NEEDED = "Нужен код из приложения";

function Field({ label, ...props }) {
  return html`<label class="auth-field">
    <span class="muted">${label}</span>
    <input class="field" ...${props} />
  </label>`;
}

/** The whole screen for `mode`: setup, login, register or invite (with `token`). */
export function AuthScreen({ mode, token, onDone }) {
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [repeat, setRepeat] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [invite, setInvite] = useState(null);
  // After a right password, when two-factor sign-in is on: the code from the app.
  const [code, setCode] = useState(null);
  const creating = mode !== "login";
  const server = state.server || {};

  useEffect(() => {
    if (mode !== "invite") return;
    api(`/api/invite/${token}`).then(setInvite).catch(e => setInvite({ error: errorText(e) }));
  }, [mode, token]);

  const submit = async e => {
    e.preventDefault();
    setError("");
    if (creating && password !== repeat) return setError(t("auth.mismatch"));
    setBusy(true);
    try {
      // A new budget names its first categories in the language it was made in.
      const body = { login: login.trim(), password, name: name.trim(), lang: currentLang(), ...(code !== null && { code: code.trim() }) };
      const path = mode === "setup" ? "/api/setup" : mode === "register" ? "/api/auth/register" : mode === "invite" ? `/api/invite/${token}` : "/api/auth/login";
      const me = await api(path, { method: "POST", body });
      // An invitation link is used up; it shouldn't open again on the next start.
      if (mode === "invite") history.replaceState(null, "", "#/");
      onDone(me);
    } catch (err) {
      if (mode === "login" && err.status === 401 && err.message === CODE_NEEDED) {
        if (code === null) return setCode("");
      }
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  if (mode === "invite" && invite?.error) {
    return html`<div class="auth">
      <div class="auth-logo">🔗</div>
      <h1 class="auth-title">${t("auth.inviteGone")}</h1>
      <div class="muted auth-hint">${invite.error}</div>
      <button type="button" class="btn btn-primary" onClick=${() => { history.replaceState(null, "", "#/"); update({ auth: { mode: "login" } }); }}>${t("auth.toLogin")}</button>
    </div>`;
  }

  const title = mode === "setup" ? t("auth.setupTitle") : mode === "register" ? t("auth.registerTitle") : mode === "invite" ? t("auth.inviteTitle") : t("auth.loginTitle");
  const hint = mode === "setup" ? t("auth.setupHint") : mode === "invite" ? (invite?.owner ? t("auth.inviteHint", { name: invite.owner }) : "") : mode === "register" ? t("auth.registerHint") : "";
  return html`<form class="auth" onSubmit=${submit}>
    <div class="auth-logo"><img src="/static/icons/icon-192.png" alt="" width="72" height="72" /></div>
    <h1 class="auth-title">${title}</h1>
    ${hint && html`<div class="muted auth-hint">${hint}</div>`}
    ${creating && html`<${Field} label=${t("auth.name")} value=${name} onInput=${e => setName(e.currentTarget.value)} autocomplete="name" maxlength="60" placeholder=${t("auth.namePlaceholder")} />`}
    ${code !== null && html`<div class="muted auth-hint">${t("tfa.loginHint")}</div>
      <${Field} label=${t("tfa.code")} value=${code} onInput=${e => setCode(e.currentTarget.value)} autocomplete="one-time-code" inputmode="numeric" autofocus
        class="field code-input" placeholder=${t("tfa.codeOrRecovery")} maxlength="16" required />`}
    <div class=${code !== null ? "hidden" : "auth-fields"}>
    <${Field} label=${t("auth.login")} value=${login} onInput=${e => setLogin(e.currentTarget.value)} autocomplete="username" autocapitalize="none" spellcheck="false" required minlength="3" maxlength="32" />
    <${Field} label=${t("auth.password")} type="password" value=${password} onInput=${e => setPassword(e.currentTarget.value)} autocomplete=${creating ? "new-password" : "current-password"} required minlength=${creating ? 8 : 1} />
    ${creating && html`<${Field} label=${t("auth.repeat")} type="password" value=${repeat} onInput=${e => setRepeat(e.currentTarget.value)} autocomplete="new-password" required minlength="8" />`}
    </div>
    ${creating && html`<div class="muted small">${t("auth.rules")}</div>`}
    ${error && html`<div class="auth-error">${error}</div>`}
    <button type="submit" class="btn btn-primary" disabled=${busy}>${mode === "login" ? t("auth.signIn") : t("auth.create")}</button>
    ${mode === "login" && server.signup_open && html`<button type="button" class="link-btn auth-switch" onClick=${() => update({ auth: { mode: "register" } })}>${t("auth.toRegister")}</button>`}
    ${mode === "register" && html`<button type="button" class="link-btn auth-switch" onClick=${() => update({ auth: { mode: "login" } })}>${t("auth.toLogin")}</button>`}
    ${code !== null && html`<button type="button" class="link-btn auth-switch" onClick=${() => { setCode(null); setError(""); }}>${t("tfa.otherLogin")}</button>`}
    ${mode === "login" && code === null && html`<div class="muted small auth-foot">${t("auth.noAccount")}</div>`}
  </form>`;
}

/** The language a demo opens in: the one chosen before, else the browser's. */
function demoLang() {
  try {
    if (localStorage.getItem("budget:lang")) return currentLang();
  } catch {}
  const browser = (navigator.language || "").slice(0, 2).toLowerCase();
  return LANGS[browser] ? browser : "en";
}

/** The public demo's start: one button makes a budget with an example in it (src/demo.rs). */
export function DemoScreen({ onDone }) {
  const [lang, pick] = useState(() => {
    const l = demoLang();
    setLang(l);
    return l;
  });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const choose = l => {
    setLang(l);
    pick(l);
  };
  const start = async () => {
    setError("");
    setBusy(true);
    try {
      onDone(await api("/api/demo", { method: "POST", body: { lang } }));
    } catch (err) {
      setError(errorText(err));
      setBusy(false);
    }
  };
  const source = state.server?.source;
  return html`<div class="auth">
    <div class="auth-logo"><img src="/static/icons/icon-192.png" alt="" width="72" height="72" /></div>
    <h1 class="auth-title">${t("demo.title")}</h1>
    <div class="muted auth-hint">${t("demo.hint")}</div>
    <div class="seg demo-langs">
      ${Object.entries(LANGS).map(([code, l]) => html`<button type="button" class=${"seg-btn" + (code === lang ? " active" : "")} onClick=${() => choose(code)}>${l.name}</button>`)}
    </div>
    ${error && html`<div class="auth-error">${error}</div>`}
    <button type="button" class="btn btn-primary" disabled=${busy} onClick=${start}>${busy ? t("demo.making") : t("demo.start")}</button>
    <div class="muted small auth-foot">${t("demo.note")}</div>
    ${source && html`<a class="link-btn auth-switch" href=${source} target="_blank" rel="noopener">${t("demo.source")}</a>`}
  </div>`;
}

/** A line above every screen of the demo: what it is and where the real thing is. */
export function DemoBar() {
  const source = state.server?.source;
  return html`<div class="demo-bar">
    ${t("demo.bar")}${source && html` · <a href=${source} target="_blank" rel="noopener">${t("demo.install")}</a>`}
  </div>`;
}
