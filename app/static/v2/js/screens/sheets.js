// Bottom sheets: quick add, editing an operation, subcategories, savings, category editor,
// people, sessions and donations. One is open at a time (state.sheet).

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText, locale } from "../i18n.js";
import { money, ymd, today, dayTitle, parseYmd, myCurrencies, allCurrencies, currencyName, currencySymbol, codeOf } from "../format.js";
import { state, useStore, closeSheet, toast, update, openSheet } from "../store.js";
import * as data from "../data.js";
import { evalAmount } from "../calc.js";
import { haptic, ask, tell, openLink, openTelegramLink } from "../tg.js";
import { Sheet, Seg, Chips, MoneyField, DateField, Row, Empty, lastCurrency, CurrencySelect, ListSelect, sourceItems } from "../ui.js";
import { AddForm, INITIAL_NOTE, pct } from "./category.js";
import { TOOL_SHEETS } from "./tools.js";
import { ACCOUNT_SHEETS, AccountPicker, activeAccounts, accountForCurrency } from "./accounts.js";

async function attempt(fn, done) {
  try {
    await fn();
    haptic("success");
    if (done) toast(done);
    return true;
  } catch (e) {
    haptic("error");
    toast(errorText(e), "error");
    return false;
  }
}

function QuickAdd() {
  const s = useStore();
  const cats = s.home?.categories || [];
  const [kind, setKind] = useState("expense");
  const expenses = cats.filter(c => c.type === "expense");
  const remembered = (() => {
    try {
      return Number(localStorage.getItem("budget:v2:quickCat"));
    } catch {
      return 0;
    }
  })();
  const [catId, setCatId] = useState(expenses.some(c => c.id === remembered) ? remembered : expenses[0]?.id);
  const category = kind === "expense" ? expenses.find(c => c.id === catId) : cats.find(c => c.type === kind);
  useEffect(() => {
    if (category && !s.details[category.id]) data.loadDetails(category.id).catch(() => {});
  }, [category?.id]);
  const pick = id => {
    setCatId(id);
    try {
      localStorage.setItem("budget:v2:quickCat", id);
    } catch {}
  };
  // Savings are accounts now: putting money aside is a transfer into one.
  const pickKind = k => (k === "savings" ? openSheet("moneyTransfer", { toSavings: true }) : setKind(k));
  return html`<${Sheet} title=${t("quick.title")}>
    <${Seg} options=${[["expense", t("kind.expense")], ["income", t("quick.income")], ["savings", t("kind.savings")]]} value=${kind} onChange=${pickKind} />
    ${kind === "expense" && html`<${Chips} items=${expenses.map(c => [c.id, `${c.emoji} ${c.name}`])} value=${catId} onChange=${pick} />`}
    ${category
      ? html`<${AddForm} key=${kind} category=${category} details=${s.details[category.id]} autofocus onSaved=${closeSheet} />`
      : html`<${Empty}>${t("quick.noCategory")}<//>`}
  <//>`;
}

/** One operation: change or delete it. `back` is the sheet it was opened from, shown again after. */
function EditOperation({ op, categoryId, back }) {
  const details = useStore().details[categoryId];
  // Opened from an account or a report, the category's subcategories may not be loaded yet.
  useEffect(() => { if (!details && categoryId) data.loadDetails(categoryId).catch(() => {}); }, [categoryId]);
  const done = () => (back ? openSheet(back.kind, back.props) : closeSheet());
  const interest = op.source === "interest" || op.source === "interest_fixed";
  const subs = details?.subcategories || [];
  const isInitial = op.type === "savings" && String(op.note || "").startsWith(INITIAL_NOTE);
  const [amount, setAmount] = useState(String(op.amount).replace(".", ","));
  const [currency, setCurrency] = useState(op.currency);
  const [note, setNote] = useState(isInitial ? "" : op.note || "");
  const [date, setDate] = useState(op.transaction_date);
  const currentSub = subs.find(x => x.name === op.subcategory_name)?.id ?? null;
  const [picked, setSub] = useState(undefined);
  // Until one is picked, the operation's own (known once the category's details are loaded).
  const sub = picked === undefined ? currentSub : picked;
  const accounts = activeAccounts(useStore());
  const [account, setAccount] = useState(op.account_id ?? null);
  // An operation that moves money is on an account; its currency is the account's.
  const onAccount = op.account_id != null;
  const pickAccount = id => {
    setAccount(id);
    const a = accounts.find(x => x.id === id);
    if (a) setCurrency(a.currency);
  };
  const pickCurrency = c => {
    setCurrency(c);
    const a = accounts.find(x => x.id === account);
    if (onAccount && a?.currency !== c) setAccount(accountForCurrency(state, account, c));
  };
  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) return tell(t("form.enterAmount"));
    const patch = { amount: value, currency, transaction_date: date };
    if (!isInitial) patch.note = note.trim();
    if (picked !== undefined && sub !== currentSub) patch.category_id = sub ?? categoryId;
    if (onAccount && account != null && account !== op.account_id) patch.account_id = account;
    if (await attempt(() => data.updateOperation(op.id, patch), t("op.saved"))) {
      done();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(interest ? t("op.confirmDeleteInterest") : t("op.confirmDelete")))) return;
    if (await attempt(() => data.deleteOperation(op.id), t("op.deleted"))) {
      done();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${isInitial ? t("savings.start") : t("op.title")}>
    <div class="form">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${pickCurrency} onEnter=${save} />
      ${onAccount && html`<${AccountPicker} accounts=${accounts} value=${account} onChange=${pickAccount} />`}
      ${subs.length > 0 && !isInitial && html`<${Chips} items=${[[null, t("form.noSubcategory")], ...subs.map(x => [x.id, x.name])]} value=${sub} onChange=${setSub} />`}
      ${!isInitial && html`<input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />`}
      <${DateField} value=${date} onChange=${setDate} />
      ${op.rate && op.rate_to && op.rate_to !== op.currency && date === op.transaction_date && currency === op.currency && html`<div class="muted small">${t("rates.opRate", {
        day: dayTitle(parseYmd(op.rate_day || op.transaction_date)),
        rate: `1 ${op.currency} = ${Number(op.rate).toLocaleString(locale(), { maximumFractionDigits: 4 })} ${op.rate_to}`,
        sum: money(evalAmount(amount) * op.rate, op.rate_to),
      })}</div>`}
      ${op.actor_name && html`<div class="muted small">${t("op.addedBy", { name: op.actor_name })}</div>`}
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
      <button type="button" class="btn btn-danger" onClick=${remove}>${t("op.delete")}</button>
    </div>
  <//>`;
}

/** Subcategories of a category: rename, delete, add. Shown in the category editor (Manage). */
function SubcategoryList({ categoryId }) {
  const details = useStore().details[categoryId];
  const [name, setName] = useState("");
  const [editing, setEditing] = useState(null);
  const [editName, setEditName] = useState("");
  const reload = () => data.loadDetails(categoryId);
  const add = async () => {
    if (!name.trim()) return;
    if (await attempt(() => data.createSubcategory(categoryId, name.trim()), t("sub.added"))) {
      setName("");
      reload();
    }
  };
  const rename = async sub => {
    if (!editName.trim() || editName.trim() === sub.name) return setEditing(null);
    if (await attempt(() => data.renameSubcategory(sub, editName.trim()))) {
      setEditing(null);
      reload();
    }
  };
  const remove = async sub => {
    if (!(await ask(t("sub.confirmDelete", { name: sub.name })))) return;
    if (await attempt(() => data.deleteSubcategory(sub.id), t("sub.deleted"))) reload();
  };
  const subs = details?.subcategories || [];
  return html`<div class="form sub-list">
    ${subs.length > 0 && html`<div class="muted small">${t("sub.hint")}</div>`}
    ${subs.length > 0 && html`<div class="group">
      ${subs.map(sub => editing === sub.id
        ? html`<div class="row row-edit">
            <input class="field" value=${editName} onInput=${e => setEditName(e.currentTarget.value)} autofocus
              onKeyDown=${e => e.key === "Enter" && rename(sub)} />
            <button type="button" class="chip" onClick=${() => rename(sub)}>✓</button>
          </div>`
        : html`<div class="row">
            <span class="row-text"><span class="row-title">${sub.name}</span></span>
            <button type="button" class="chip chip-icon" onClick=${() => { setEditing(sub.id); setEditName(sub.name); }} aria-label=${t("sub.rename")}>✎</button>
            <button type="button" class="chip chip-icon chip-danger" onClick=${() => remove(sub)} aria-label=${t("common.delete")}>✕</button>
          </div>`)}
    </div>`}
    <div class="inline-form">
      <input class="field" placeholder=${t("sub.newName")} value=${name} onInput=${e => setName(e.currentTarget.value)} onKeyDown=${e => e.key === "Enter" && add()} maxlength="60" />
      <button type="button" class="btn btn-primary btn-small" onClick=${add}>${t("common.add")}</button>
    </div>
  </div>`;
}

/** The last emoji typed, whole: skin tones, flags and joined emoji like 👨‍👩‍👧 stay one icon. Letters are ignored. */
function lastEmoji(text) {
  const parts = [...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(text)].map(g => g.segment);
  return parts.reverse().find(g => /\p{Extended_Pictographic}|\p{Regional_Indicator}/u.test(g)) || "";
}

const EMOJIS = ["🛒", "🍽️", "☕", "🏠", "💡", "🚗", "⛽", "🚌", "🏥", "💊", "🎉", "🎬", "👕", "🎁", "📦", "🐾", "👶", "📚", "💻", "📱", "✈️", "🏖️", "💸", "🧾"];
const COLORS = ["#e0af68", "#9ece6a", "#73daca", "#2ac3de", "#6183bb", "#7aa2f7", "#bb9af7", "#f7768e", "#ff9e64", "#545c7e"];

function CategoryEditor({ id }) {
  const existing = id ? state.home?.categories.find(c => c.id === id) : null;
  // Its subcategories are edited here, in Manage only.
  useEffect(() => {
    if (existing && !state.details[id]) data.loadDetails(id).catch(() => {});
  }, [id]);
  const [name, setName] = useState(existing?.name || "");
  const [emoji, setEmoji] = useState(existing?.emoji || "🛒");
  const [color, setColor] = useState(existing?.color || COLORS[0]);
  const save = async () => {
    if (!name.trim()) return tell(t("err.emptyName"));
    const body = { name: name.trim(), emoji: emoji.trim() || "🛒", color };
    // The server replaces the goal with what is sent, so the current one is kept.
    const ok = await attempt(
      () => existing ? data.updateCategory(id, { ...body, target_amount: existing.target_amount ?? null }) : data.createCategory(body),
      existing ? t("catEdit.saved") : t("catEdit.created"));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(t("catEdit.confirmDelete", { name: existing.name })))) return;
    if (await attempt(() => data.deleteCategory(id), t("catEdit.deleted"))) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${existing ? t("catEdit.edit") : t("catEdit.new")}>
    <div class="form">
      <input class="field" placeholder=${t("catEdit.name")} value=${name} onInput=${e => setName(e.currentTarget.value)} maxlength="40" autofocus=${!existing} />
      <label class="emoji-pick">
        <input class="field emoji-field" value=${emoji} aria-label=${t("catEdit.emoji")}
          onFocus=${e => e.currentTarget.select()}
          onInput=${e => { const picked = lastEmoji(e.currentTarget.value); setEmoji(picked || emoji); e.currentTarget.value = picked || emoji; }} />
        <span class="emoji-pick-hint">${t("catEdit.emojiHint")}</span>
      </label>
      <div class="emoji-grid">
        ${EMOJIS.map(e => html`<button type="button" class=${"emoji-btn" + (e === emoji ? " active" : "")} onClick=${() => setEmoji(e)}>${e}</button>`)}
      </div>
      <div class="color-row">
        ${[...new Set([...COLORS, color])].map(c => html`<button type="button" class=${"color-dot" + (c === color ? " active" : "")}
          style=${`background:${c}`} onClick=${() => setColor(c)} aria-label=${c}></button>`)}
      </div>
      <button type="button" class="btn btn-primary" onClick=${save}>${existing ? t("common.save") : t("catEdit.create")}</button>
    </div>
    ${existing && html`<div class="section-title">${t("sub.title")}</div>
      <${SubcategoryList} categoryId=${id} />`}
    ${existing?.type === "expense" && html`<div class="form"><button type="button" class="btn btn-danger" onClick=${remove}>${t("catEdit.delete")}</button></div>`}
  <//>`;
}

/** A one-time link into the budget, made here and shown once; the list shows links not used yet. */
function Invites() {
  const [list, setList] = useState(null);
  const [role, setRole] = useState("user");
  const [made, setMade] = useState(null);
  const reload = () => data.loadInvites().then(setList).catch(() => setList([]));
  useEffect(() => { reload(); }, []);
  const make = async () => {
    try {
      const inv = await data.createInvite(role);
      setMade(`${location.origin}/#/invite/${inv.token}`);
      haptic("success");
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const share = async () => {
    try {
      if (navigator.share) return await navigator.share({ title: t("invite.shareTitle"), url: made });
      await navigator.clipboard.writeText(made);
      toast(t("donate.copied"));
    } catch {}
  };
  const revoke = async inv => {
    if (await attempt(() => data.deleteInvite(inv.id), t("invite.revoked"))) reload();
  };
  const until = ts => new Date(ts * 1000).toLocaleDateString(locale(), { day: "numeric", month: "long" });
  return html`<div class="card-title">${t("invite.title")}</div>
    <div class="form">
      <div class="muted small">${t("invite.hint")}</div>
      <${Seg} options=${[["user", t("role.user")], ["admin", t("role.admin")]]} value=${role} onChange=${setRole} />
      <div class="muted small">${role === "admin" ? t("role.adminHint") : t("role.userHint")}</div>
      <button type="button" class="btn btn-primary" onClick=${make}>🔗 ${t("invite.make")}</button>
      ${made && html`<input class="field donate-address" readonly value=${made} onFocus=${e => e.currentTarget.select()} />
        <button type="button" class="btn" onClick=${share}>${navigator.share ? t("invite.share") : t("donate.copy")}</button>`}
    </div>
    ${list?.length > 0 && html`<div class="group">
      ${list.map(inv => html`<div class="row">
        <span class="row-text">
          <span class="row-title">${inv.role === "admin" ? t("role.admin") : t("role.user")}</span>
          <span class="row-sub">${t("invite.until", { date: until(inv.expires_at) })}</span>
        </span>
        <button type="button" class="chip chip-danger" onClick=${() => revoke(inv)}>${t("invite.revoke")}</button>
      </div>`)}
    </div>`}`;
}

/** A new password: the current one first, the new one twice. */
function PasswordSheet() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const save = async () => {
    if (next !== repeat) return tell(t("auth.mismatch"));
    if (await attempt(() => data.changePassword(current, next), t("auth.passwordChanged"))) closeSheet();
  };
  return html`<${Sheet} title=${t("auth.changePassword")}>
    <div class="form">
      <input class="field" type="password" autocomplete="current-password" placeholder=${t("auth.current")} value=${current} onInput=${e => setCurrent(e.currentTarget.value)} />
      <input class="field" type="password" autocomplete="new-password" placeholder=${t("auth.newPassword")} value=${next} onInput=${e => setNext(e.currentTarget.value)} />
      <input class="field" type="password" autocomplete="new-password" placeholder=${t("auth.repeat")} value=${repeat} onInput=${e => setRepeat(e.currentTarget.value)} />
      <div class="muted small">${t("auth.otherDevices")}</div>
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
    </div>
  <//>`;
}

function Users() {
  const me = useStore().me;
  const telegram = useStore().server?.telegram;
  const [users, setUsers] = useState(null);
  const [query, setQuery] = useState("");
  const [name, setName] = useState("");
  const [role, setRole] = useState("user");
  const reload = () => data.loadUsers().then(setUsers).catch(e => toast(errorText(e), "error"));
  useEffect(() => { reload(); }, []);
  const add = async () => {
    const q = query.trim();
    if (!q) return tell(t("users.enterQuery"));
    const byId = /^\d+$/.test(q);
    if (!(await ask(byId ? t("users.confirmAddId") : t("users.confirmAddName")))) return;
    if (await attempt(() => data.addUser(q, name.trim(), role), t("users.added"))) {
      setQuery("");
      setName("");
      reload();
    }
  };
  const changeRole = async (u, r) => {
    if (await attempt(() => data.setUserRole(u.id, r), t("users.roleSaved"))) reload();
  };
  const remove = async u => {
    if (!(await ask(t("users.confirmRemove", { name: displayName(u) })))) return;
    if (await attempt(() => data.removeUser(u.id), t("users.removed"))) reload();
  };
  return html`<${Sheet} title=${t("users.title")}>
    <div class="group">
      ${users === null && html`<div class="row muted">${t("common.loading")}</div>`}
      ${users?.map(u => {
        const pending = u.telegram_id.startsWith("pending:");
        const local = u.telegram_id.startsWith("local:");
        return html`<div class="row user-row">
          <span class="row-text">
            <span class="row-title">${displayName(u)}${u.id === me?.id ? ` · ${t("users.you")}` : ""}</span>
            <span class="row-sub">${pending ? t("users.pending", { name: u.telegram_id.slice(8) }) : local ? t("auth.loginIs", { login: u.telegram_id.slice(6) }) : `Telegram ID ${u.telegram_id}`}</span>
          </span>
          ${u.role === "owner"
            ? html`<span class="role-badge">Owner</span>`
            : html`<select class="field role-select" value=${u.role} onChange=${e => changeRole(u, e.currentTarget.value)}>
                <option value="user">${t("role.user")}</option>
                <option value="admin">${t("role.admin")}</option>
              </select>`}
          ${u.role !== "owner" && u.id !== me?.id && html`<button type="button" class="chip chip-icon chip-danger" onClick=${() => remove(u)} aria-label=${t("users.remove")}>✕</button>`}
        </div>`;
      })}
    </div>
    <${Invites} />
    ${telegram && html`<div class="card-title">${t("users.addTitle")}</div>
    <div class="form">
      <input class="field" placeholder=${t("users.queryPlaceholder")} value=${query} onInput=${e => setQuery(e.currentTarget.value)} autocapitalize="off" autocomplete="off" spellcheck="false" />
      <input class="field" placeholder=${t("users.namePlaceholder")} value=${name} onInput=${e => setName(e.currentTarget.value)} />
      <${Seg} options=${[["user", t("role.user")], ["admin", t("role.admin")]]} value=${role} onChange=${setRole} />
      <div class="muted small">${role === "admin" ? t("role.adminHint") : t("role.userHint")}</div>
      <button type="button" class="btn btn-primary" onClick=${add}>${t("users.add")}</button>
    </div>`}
  <//>`;
}

export function displayName(u) {
  return [u.first_name, u.last_name].filter(Boolean).join(" ") || (u.username ? "@" + u.username : u.telegram_id.replace(/^pending:/, "@"));
}

function Sessions() {
  const [list, setList] = useState(null);
  const reload = () => data.loadSessions().then(setList).catch(e => toast(errorText(e), "error"));
  useEffect(() => { reload(); }, []);
  const end = async s => {
    if (!(await ask(t("sessions.confirmEnd")))) return;
    if (await attempt(() => data.endSession(s.token_suffix), t("sessions.ended"))) reload();
  };
  const when = v => {
    if (!v) return "—";
    const d = new Date(String(v).replace(" ", "T").replace(/\+00$/, "Z"));
    return isNaN(d) ? v : `${dayTitle(d)} ${d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  };
  return html`<${Sheet} title=${t("sessions.title")}>
    <div class="group">
      ${list === null && html`<div class="row muted">${t("common.loading")}</div>`}
      ${list?.map(s => html`<div class=${"row" + (s.is_current ? " row-current" : "")}>
        <span class="row-text">
          <span class="row-title">${device(s.user_agent)}${s.is_current ? ` · ${t("sessions.current")}` : ""}</span>
          <span class="row-sub">${t("sessions.lastUsed", { when: when(s.last_used_at || s.created_at) })}</span>
        </span>
        ${!s.is_current && html`<button type="button" class="chip chip-danger" onClick=${() => end(s)}>${t("sessions.end")}</button>`}
      </div>`)}
    </div>
  <//>`;
}

/** Ways to support the author. The address is shown in full: a wallet link opens only in some
 *  apps (not in Telegram Desktop), so copying it and pasting into @wallet always works. */
function Donate({ ways }) {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(ways.ton);
    } catch {
      // Some Telegram web views block the clipboard API; a selected field still copies.
      const field = document.getElementById("donate-address");
      field.select();
      if (!document.execCommand("copy")) return toast(t("donate.copyByHand"), "error");
    }
    haptic("success");
    toast(t("donate.copied"));
  };
  return html`<${Sheet} title=${t("donate.title")}>
    <div class="muted small">${t("donate.hint")}</div>
    ${ways.ton && html`<div class="card">
      <div class="card-title">${t("donate.ton")}</div>
      <input id="donate-address" class="field donate-address" readonly value=${ways.ton} onFocus=${e => e.currentTarget.select()} />
      <div class="donate-actions">
        <button type="button" class="btn btn-primary" onClick=${copy}>${t("donate.copy")}</button>
        <button type="button" class="btn" onClick=${() => openTelegramLink("https://t.me/wallet")}>${t("donate.openWallet")}</button>
      </div>
      <div class="muted small donate-steps">${t("donate.tonSteps")}</div>
    </div>`}
    ${(ways.github || ways.url) && html`<div class="group">
      ${ways.github && html`<${Row} icon="🐙" title="GitHub Sponsors" subtitle=${t("donate.githubSub")} onClick=${() => openLink("https://github.com/sponsors/" + ways.github)} />`}
      ${ways.url && html`<${Row} icon="🔗" title=${t("donate.other")} subtitle=${new URL(ways.url).host} onClick=${() => openLink(ways.url)} />`}
    </div>`}
  <//>`;
}

/** A number typed with a comma or a dot; NaN when it isn't one. */
function decimal(text) {
  const s = String(text).replace(/\s/g, "").replace(",", ".");
  return s === "" ? NaN : Number(s);
}

const TERMS = [32, 92, 181, 365];

/** A readable device name from the browser's description. */
function device(ua = "") {
  if (/iPhone|iPad/.test(ua)) return "iPhone / iPad";
  if (/Android/.test(ua)) return "Android";
  if (/Mac OS/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows";
  if (/Linux/.test(ua)) return "Linux";
  return ua ? ua.slice(0, 40) : t("sessions.unknown");
}

/** The budget's main currency and the source of rates for operations in other currencies. */
function BudgetRates() {
  const current = useStore().home?.settings || { main_currency: "₺", rate_source: "tcmb" };
  const [main, setMain] = useState(current.main_currency);
  const [source, setSource] = useState(current.rate_source);
  const save = async () => {
    if (await attempt(() => data.saveSettings({ main_currency: main, rate_source: source }), t("budgetRates.saved"))) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${t("budgetRates.title")}>
    <div class="muted small">${t("budgetRates.hint")}</div>
    <div class="form">
      <div class="muted small">${t("budgetRates.main")}</div>
      <${CurrencySelect} value=${main} onChange=${setMain} />
      <${ListSelect} items=${sourceItems(["tcmb", "cbr", "ecb", "fawaz", "off"])} value=${source} onChange=${setSource} label=${t("budgetRates.source")} />
      ${source === "ecb" && html`<div class="muted small">${t("rates.ecbNoRub")}</div>`}
      <div class="muted small">${t("budgetRates.fallback")}</div>
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
    </div>
  <//>`;
}

/** This person's own list of currencies offered in forms; the rest stay one tap away. */
function MyCurrencies() {
  const s = useStore();
  const [list, setList] = useState(myCurrencies(s));
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const fits = id => !needle || codeOf(id).toLowerCase().includes(needle) || id.toLowerCase().includes(needle) || currencyName(id).toLowerCase().includes(needle);
  const toggle = id => setList(list.includes(id) ? list.filter(x => x !== id) : [...list, id]);
  const save = async currencies => {
    try {
      await data.savePrefs({ currencies });
      update({ me: { ...s.me, currencies } });
      toast(t("common.saved"));
      closeSheet();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const row = id => html`<button type="button" key=${id} class=${"picker-row" + (list.includes(id) ? " active" : "")} onClick=${() => toggle(id)}>
    <span class=${"pr-icon" + (currencySymbol(id).length > 2 ? " pr-code" : "")}>${currencySymbol(id)}</span>
    <span class="pr-text"><span class="pr-name">${currencyName(id)}</span><span class="muted small">${codeOf(id)}</span></span>
    <span class="pr-check">${list.includes(id) ? "✓" : ""}</span>
  </button>`;
  return html`<${Sheet} title=${t("currency.myTitle")} tall>
    <div class="muted small">${t("currency.myHint")}</div>
    <input class="field" placeholder=${t("currency.search")} value=${q} onInput=${e => setQ(e.currentTarget.value)} autocomplete="off" />
    <div class="picker-list my-currencies">
      ${list.filter(fits).map(row)}
      ${allCurrencies().filter(id => !list.includes(id)).filter(fits).slice(0, 200).map(row)}
    </div>
    <div class="form">
      <button type="button" class="btn btn-primary" onClick=${() => (list.length ? save(list) : tell(t("currency.pickOne")))}>${t("common.save")}</button>
      ${s.me?.currencies && html`<button type="button" class="btn" onClick=${() => save(null)}>${t("currency.reset")}</button>`}
    </div>
  <//>`;
}

const SHEETS = { myCurrencies: MyCurrencies, budgetRates: BudgetRates, quickAdd: QuickAdd, editOperation: EditOperation, categoryEditor: CategoryEditor, users: Users, sessions: Sessions, donate: Donate, password: PasswordSheet, ...TOOL_SHEETS, ...ACCOUNT_SHEETS };

export function SheetHost() {
  const sheet = useStore().sheet;
  if (!sheet) return null;
  const C = SHEETS[sheet.kind];
  return C ? html`<${C} key=${sheet.sheetKey} ...${sheet} />` : null;
}
