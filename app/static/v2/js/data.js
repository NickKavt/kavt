// Loading and changing budget data. Every change reloads what is on screen, so totals
// always come from the server rather than being recalculated here.

import { api } from "./api.js";
import { state, update } from "./store.js";
import { ymd } from "./format.js";

const homeKey = month => `budget:v2:home:${state.me?.id}:${ymd(month)}`;

/** The last home data of a month, shown at once on start while fresh data loads. */
export function cachedHome(month) {
  try {
    const raw = localStorage.getItem(homeKey(month));
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

export async function loadHome() {
  const month = state.month;
  update({ homeLoading: true });
  try {
    const home = await api(`/api/home?month=${ymd(month)}`);
    // A slow answer for a month the user has already left is dropped.
    if (ymd(month) !== ymd(state.month)) return;
    update({ home });
    try {
      localStorage.setItem(homeKey(month), JSON.stringify(home));
    } catch {}
  } finally {
    if (ymd(month) === ymd(state.month)) update({ homeLoading: false });
  }
}

export async function loadDetails(id) {
  const month = state.month;
  const details = await api(`/api/categories/${id}/details?month=${ymd(month)}`);
  if (ymd(month) !== ymd(state.month)) return;
  update(s => ({ details: { ...s.details, [id]: details } }));
}

/** Reloads the home screen and the category on screen, if any. */
export async function refresh() {
  // Other categories loaded earlier (say, by the quick-add sheet) may be out of date now.
  const open = state.route.name === "category" ? state.route.id : null;
  update(s => ({ details: open && s.details[open] ? { [open]: s.details[open] } : {} }));
  const jobs = [loadHome()];
  if (open) jobs.push(loadDetails(open));
  await Promise.all(jobs);
}

export function setMonth(month) {
  update({ month, home: cachedHome(month), details: {} });
  return refresh();
}

/** Adds an expense, income or savings operation. */
export async function addOperation({ category, subcategoryId, amount, currency, note, date }) {
  const body = { amount, currency, note: note || "", transaction_date: date };
  if (category.type === "expense") {
    await api(`/api/categories/${category.id}/expenses`, { method: "POST", body: { ...body, is_debt: false, subcategory_id: subcategoryId ?? null } });
  } else {
    // Income and savings may sit in a subcategory too; the server books them on the id given.
    const target = subcategoryId ?? category.id;
    await api(`/api/${category.type === "income" ? "income" : "savings"}?category_id=${target}`, { method: "POST", body });
  }
}

/** Takes money out of savings (the category or one of its accounts) back into the month. */
export async function withdraw({ category, subcategoryId, amount, currency, note, date }) {
  const body = { amount, currency, note: note || "", transaction_date: date };
  await api(`/api/savings/withdraw?category_id=${subcategoryId ?? category.id}`, { method: "POST", body });
}

/** Moves money between savings accounts; both halves are created together. */
export const transfer = ({ fromId, toId, amount, currency, note, date }) =>
  api("/api/savings/transfer", { method: "POST", body: { from_id: fromId, to_id: toId, amount, currency, note: note || "", transaction_date: date } });

export const updateOperation = (id, patch) => api(`/api/transactions/${id}`, { method: "PATCH", body: patch });
export const deleteOperation = id => api(`/api/transactions/${id}`, { method: "DELETE" });

export const createCategory = c => api("/api/categories", { method: "POST", body: { ...c, type: "expense" } });
export const updateCategory = (id, c) => api(`/api/categories/${id}`, { method: "PATCH", body: c });
export const deleteCategory = id => api(`/api/categories/${id}`, { method: "DELETE" });

// Added while a past month is on screen, it shows from that month.
export const createSubcategory = (parentId, name) =>
  api(`/api/categories/${parentId}/subcategories`, { method: "POST", body: { name, emoji: "•", color: "#545c7e", month: ymd(state.month) } });
export const renameSubcategory = (sub, name) =>
  api(`/api/subcategories/${sub.id}`, { method: "PATCH", body: { name, emoji: sub.emoji || "•", color: sub.color || "#545c7e" } });
export const deleteSubcategory = id => api(`/api/subcategories/${id}`, { method: "DELETE" });

export const loadAccounts = () => api("/api/savings/accounts");
export const createAccount = a => api("/api/savings/accounts", { method: "POST", body: a });
export const updateAccount = (id, a) => api(`/api/savings/accounts/${id}`, { method: "PATCH", body: a });
export const stopAccount = id => api(`/api/savings/accounts/${id}`, { method: "DELETE" });

export const saveSettings = settings => api("/api/settings", { method: "PUT", body: settings });

export const loadReport = (from, to) => api(`/api/report?from=${ymd(from)}&to=${ymd(to)}`);

export const loadUsers = () => api("/api/users");
export const loadInvites = () => api("/api/invites");
export const createInvite = role => api("/api/invites", { method: "POST", body: { role } });
export const deleteInvite = id => api(`/api/invites/${id}`, { method: "DELETE" });
export const changePassword = (current, next) => api("/api/auth/password", { method: "POST", body: { current, new: next } });
export const addUser = (query, firstName, role) => api("/api/users", { method: "POST", body: { query, first_name: firstName, role } });
export const setUserRole = (id, role) => api(`/api/users/${id}/role`, { method: "PATCH", body: { role } });
export const removeUser = id => api(`/api/users/${id}`, { method: "DELETE" });

export const loadSessions = () => api("/api/sessions");
export const endSession = suffix => api(`/api/sessions/${suffix}`, { method: "DELETE" });
export const endOtherSessions = () => api("/api/auth/revoke-others", { method: "POST" });
export const signOut = () => api("/api/auth/logout", { method: "POST" });

export const loadRecurring = () => api("/api/recurring");
/** A repeating operation from its first day; the server adds what is due right away. */
export const createRecurring = ({ categoryId, amount, currency, note, every, start }) =>
  api("/api/recurring", { method: "POST", body: { category_id: categoryId, amount, currency, note: note || "", every, start_date: start } });
export const updateRecurring = (id, patch) => api(`/api/recurring/${id}`, { method: "PATCH", body: patch });
export const stopRecurring = id => api(`/api/recurring/${id}`, { method: "DELETE" });

export const importRows = (rows, dryRun) => api("/api/import", { method: "POST", body: { rows, dry_run: dryRun } });

export const twoFactorSetup = () => api("/api/2fa/setup", { method: "POST" });
export const twoFactorEnable = code => api("/api/2fa/enable", { method: "POST", body: { code } });
export const twoFactorDisable = code => api("/api/2fa/disable", { method: "POST", body: { code } });
export const newRecoveryCodes = code => api("/api/2fa/recovery-codes", { method: "POST", body: { code } });

/** Saves the budget as a CSV file. */
export async function exportCsv() {
  const res = await fetch("/api/export.csv", { credentials: "same-origin" });
  if (!res.ok) throw Object.assign(new Error((await res.text()).trim()), { status: res.status });
  const url = URL.createObjectURL(await res.blob());
  const a = Object.assign(document.createElement("a"), { href: url, download: `budget-${ymd(new Date())}.csv` });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
