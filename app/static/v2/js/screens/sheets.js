// Bottom sheets: quick add, editing an operation, subcategories, savings, category editor,
// people, sessions and donations. One is open at a time (state.sheet).

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText, locale } from "../i18n.js";
import { money, ymd, CURRENCIES, dayTitle, parseYmd } from "../format.js";
import { state, useStore, closeSheet, toast, update } from "../store.js";
import * as data from "../data.js";
import { evalAmount } from "../calc.js";
import { haptic, ask, tell, openLink, openTelegramLink } from "../tg.js";
import { Sheet, Seg, Chips, MoneyField, DateField, Row, Empty, lastCurrency } from "../ui.js";
import { AddForm, INITIAL_NOTE, pct } from "./category.js";
import { TOOL_SHEETS } from "./tools.js";

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
  return html`<${Sheet} title=${t("quick.title")}>
    <${Seg} options=${[["expense", t("kind.expense")], ["income", t("quick.income")], ["savings", t("kind.savings")]]} value=${kind} onChange=${setKind} />
    ${kind === "expense" && html`<${Chips} items=${expenses.map(c => [c.id, `${c.emoji} ${c.name}`])} value=${catId} onChange=${pick} />`}
    ${category
      ? html`<${AddForm} key=${category.id} category=${category} details=${s.details[category.id]} autofocus onSaved=${closeSheet} />`
      : html`<${Empty}>${t("quick.noCategory")}<//>`}
  <//>`;
}

function EditOperation({ op, categoryId }) {
  const details = useStore().details[categoryId];
  const subs = details?.subcategories || [];
  const isInitial = op.type === "savings" && String(op.note || "").startsWith(INITIAL_NOTE);
  const [amount, setAmount] = useState(String(op.amount).replace(".", ","));
  const [currency, setCurrency] = useState(op.currency);
  const [note, setNote] = useState(isInitial ? "" : op.note || "");
  const [date, setDate] = useState(op.transaction_date);
  const currentSub = subs.find(x => x.name === op.subcategory_name)?.id ?? null;
  const [sub, setSub] = useState(currentSub);
  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) return tell(t("form.enterAmount"));
    const patch = { amount: value, currency, transaction_date: date };
    if (!isInitial) patch.note = note.trim();
    if (sub !== currentSub) patch.category_id = sub ?? categoryId;
    if (await attempt(() => data.updateOperation(op.id, patch), t("op.saved"))) {
      closeSheet();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(t("op.confirmDelete")))) return;
    if (await attempt(() => data.deleteOperation(op.id), t("op.deleted"))) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${isInitial ? t("savings.start") : t("op.title")}>
    <div class="form">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${setCurrency} onEnter=${save} />
      ${subs.length > 0 && !isInitial && html`<${Chips} items=${[[null, t("form.noSubcategory")], ...subs.map(x => [x.id, x.name])]} value=${sub} onChange=${setSub} />`}
      ${!isInitial && html`<input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />`}
      <${DateField} value=${date} onChange=${setDate} />
      ${op.actor_name && html`<div class="muted small">${t("op.addedBy", { name: op.actor_name })}</div>`}
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
      <button type="button" class="btn btn-danger" onClick=${remove}>${t("op.delete")}</button>
    </div>
  <//>`;
}

function Subcategories({ categoryId }) {
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
  return html`<${Sheet} title=${t("sub.title")}>
    <div class="muted small">${t("sub.hint")}</div>
    <div class="group">
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
      ${!subs.length && html`<div class="row muted">${t("sub.none")}</div>`}
    </div>
    <div class="inline-form">
      <input class="field" placeholder=${t("sub.newName")} value=${name} onInput=${e => setName(e.currentTarget.value)} onKeyDown=${e => e.key === "Enter" && add()} maxlength="60" />
      <button type="button" class="btn btn-primary btn-small" onClick=${add}>${t("common.add")}</button>
    </div>
  <//>`;
}

function savingsCategory() {
  return state.home?.categories.find(c => c.type === "savings");
}

function SavingsGoal() {
  const summary = state.home?.savings_summary;
  const cat = savingsCategory();
  const [amount, setAmount] = useState(summary?.target_amount ? String(summary.target_amount) : "");
  const save = async (value) => {
    const ok = await attempt(() => data.updateCategory(cat.id, { name: cat.name, emoji: cat.emoji, color: cat.color, target_amount: value }), t("savings.goalSaved"));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${t("savings.goal")}>
    <div class="form">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${summary?.currency || "₺"} onCurrency=${() => {}} autofocus />
      <button type="button" class="btn btn-primary" onClick=${() => { const v = evalAmount(amount); v > 0 ? save(v) : tell(t("form.enterAmount")); }}>${t("common.save")}</button>
      ${summary?.target_amount > 0 && html`<button type="button" class="btn" onClick=${() => save(null)}>${t("savings.removeGoal")}</button>`}
    </div>
  <//>`;
}

function SavingsStart() {
  const summary = state.home?.savings_summary;
  const initial = summary?.initial;
  const [amount, setAmount] = useState(initial ? String(initial.amount).replace(".", ",") : "");
  const [currency, setCurrency] = useState(initial?.currency || summary?.currency || "₺");
  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) return tell(t("form.enterAmount"));
    // One starting balance: an existing one is changed, never added a second time.
    const ok = await attempt(
      () => initial
        ? data.updateOperation(initial.id, { amount: value, currency })
        : data.addOperation({ category: savingsCategory(), amount: value, currency, note: INITIAL_NOTE, date: "2000-01-01" }),
      t("savings.startSaved"));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  const remove = async () => {
    if (!(await ask(t("savings.confirmRemoveStart")))) return;
    if (await attempt(() => data.deleteOperation(initial.id), t("op.deleted"))) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${t("savings.start")}>
    <div class="muted small">${t("savings.startHint")}</div>
    <div class="form">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${setCurrency} autofocus onEnter=${save} />
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
      ${initial && html`<button type="button" class="btn btn-danger" onClick=${remove}>${t("savings.removeStart")}</button>`}
    </div>
  <//>`;
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
      ${existing?.type === "expense" && html`<button type="button" class="btn btn-danger" onClick=${remove}>${t("catEdit.delete")}</button>`}
    </div>
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
    <div class="muted small">${t("users.roles")}</div>
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

/** Money from one savings account to another; `places` are the savings and its accounts. */
function Transfer({ places }) {
  const [from, setFrom] = useState(places[1]?.id ?? places[0].id);
  const [to, setTo] = useState(places[0].id === from ? places[1]?.id : places[0].id);
  const [amount, setAmount] = useState("");
  const [currency, setCurrency] = useState(lastCurrency());
  const [note, setNote] = useState("");
  const [date, setDate] = useState(ymd(new Date()));
  const items = places.map(p => [p.id, p.name]);
  const pickFrom = id => {
    setFrom(id);
    if (id === to) setTo(places.find(p => p.id !== id)?.id);
  };
  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) return tell(t("form.enterAmount"));
    if (from === to) return tell(t("transfer.same"));
    const ok = await attempt(() => data.transfer({ fromId: from, toId: to, amount: value, currency, note: note.trim(), date }), t("transfer.done"));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  return html`<${Sheet} title=${t("transfer.title")}>
    <div class="form">
      <div class="muted small">${t("transfer.from")}</div>
      <${Chips} items=${items} value=${from} onChange=${pickFrom} />
      <div class="muted small">${t("transfer.to")}</div>
      <${Chips} items=${items.filter(([id]) => id !== from)} value=${to} onChange=${setTo} />
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${setCurrency} onEnter=${save} />
      <input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />
      <${DateField} value=${date} onChange=${setDate} />
      <div class="muted small">${t("transfer.hint")}</div>
      <button type="button" class="btn btn-primary" onClick=${save}>${t("transfer.button")}</button>
    </div>
  <//>`;
}

/** A new savings account with interest, or the settings of one. */
function AccountEditor({ account }) {
  const editing = !!account;
  const [name, setName] = useState(account?.name || "");
  const [kind, setKind] = useState(account?.kind || "flexible");
  const [currency, setCurrency] = useState(account?.currency || lastCurrency());
  const [rate, setRate] = useState(account ? pct(account.rate) : "");
  const [tax, setTax] = useState(account?.tax ? pct(account.tax) : "");
  const [days, setDays] = useState(String(account?.term_days || 32));
  const [start, setStart] = useState(account?.start || ymd(new Date()));
  const [rateFrom, setRateFrom] = useState(ymd(new Date()));
  const taxValue = tax.trim() ? decimal(tax) : 0;
  const rateChanged = editing && (decimal(rate) !== account.rate || taxValue !== account.tax);

  const save = async () => {
    if (!name.trim()) return tell(t("err.emptyName"));
    const r = decimal(rate);
    if (!(r >= 0)) return tell(t("acc.enterRate"));
    if (!(taxValue >= 0 && taxValue < 100)) return tell(t("acc.enterTax"));
    const termDays = Math.round(decimal(days));
    if (kind === "term" && !(termDays >= 1)) return tell(t("acc.enterDays"));
    const body = { name: name.trim(), kind, currency, rate: r, tax: taxValue, term_days: kind === "term" ? termDays : null, start, rate_from: rateChanged ? rateFrom : null };
    const ok = await attempt(() => (editing ? data.updateAccount(account.id, body) : data.createAccount(body)), (editing ? t("acc.saved") : t("acc.created")));
    if (ok) {
      closeSheet();
      data.refresh();
    }
  };
  const stop = async () => {
    if (!(await ask(t("acc.confirmStop", { name: account.name })))) return;
    if (await attempt(() => data.stopAccount(account.id), t("acc.stopped"))) {
      closeSheet();
      data.refresh();
    }
  };

  return html`<${Sheet} title=${editing ? account.name : t("acc.new")}>
    <div class="form">
      <input class="field" value=${name} onInput=${e => setName(e.currentTarget.value)} placeholder=${t("acc.namePlaceholder")} maxlength="60" />
      <${Seg} options=${[["flexible", t("acc.flexible")], ["term", t("acc.term")]]} value=${kind} onChange=${setKind} />
      <div class="muted small">${kind === "term" ? t("acc.termHint") : t("acc.flexibleHint")}</div>
      <div class="acc-fields">
        <label class="acc-field">
          <span class="muted">${t("acc.rate")}</span>
          <span class="acc-input"><input class="field" inputmode="decimal" value=${rate} onInput=${e => setRate(e.currentTarget.value)} placeholder="45" /><span>%</span></span>
        </label>
        <label class="acc-field">
          <span class="muted">${t("acc.tax")}</span>
          <span class="acc-input"><input class="field" inputmode="decimal" value=${tax} onInput=${e => setTax(e.currentTarget.value)} placeholder="0" /><span>%</span></span>
        </label>
      </div>
      ${kind === "term" && html`<label class="acc-field">
        <span class="muted">${t("acc.days")}</span>
        <input class="field" inputmode="numeric" value=${days} onInput=${e => setDays(e.currentTarget.value)} />
      </label>
      <${Chips} items=${TERMS.map(n => [String(n), t("acc.daysShort", { n })])} value=${days} onChange=${setDays} />`}
      <${Chips} items=${CURRENCIES.map(c => [c, c])} value=${currency} onChange=${setCurrency} />
      <label class="date-row">
        <span class="date-label">${t("acc.start")}</span>
        <input type="date" class="field date-input" value=${start} max=${ymd(new Date())} onChange=${e => e.currentTarget.value && setStart(e.currentTarget.value)} />
      </label>
      ${rateChanged && html`<label class="date-row">
        <span class="date-label">${t("acc.rateFrom")}</span>
        <input type="date" class="field date-input" value=${rateFrom} min=${start} max=${ymd(new Date())} onChange=${e => e.currentTarget.value && setRateFrom(e.currentTarget.value)} />
      </label>`}
      <div class="muted small">${t("acc.depositHint")} ${t("acc.fixHint")}</div>
      <button type="button" class="btn btn-primary" onClick=${save}>${editing ? t("common.save") : t("catEdit.create")}</button>
      ${editing && html`<button type="button" class="btn btn-danger" onClick=${stop}>${t("acc.stop")}</button>`}
    </div>
  <//>`;
}

/** A readable device name from the browser's description. */
function device(ua = "") {
  if (/iPhone|iPad/.test(ua)) return "iPhone / iPad";
  if (/Android/.test(ua)) return "Android";
  if (/Mac OS/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows";
  if (/Linux/.test(ua)) return "Linux";
  return ua ? ua.slice(0, 40) : t("sessions.unknown");
}

const SHEETS = { quickAdd: QuickAdd, editOperation: EditOperation, subcategories: Subcategories, savingsGoal: SavingsGoal, savingsStart: SavingsStart, categoryEditor: CategoryEditor, users: Users, sessions: Sessions, donate: Donate, account: AccountEditor, transfer: Transfer, password: PasswordSheet, ...TOOL_SHEETS };

export function SheetHost() {
  const sheet = useStore().sheet;
  if (!sheet) return null;
  const C = SHEETS[sheet.kind];
  return C ? html`<${C} key=${sheet.sheetKey} ...${sheet} />` : null;
}
