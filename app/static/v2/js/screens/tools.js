// Sheets for keeping the budget: recurring operations, CSV import and two-factor sign-in.

import { html, useEffect, useState } from "../../vendor/preact-htm.js";
import { t, errorText, locale } from "../i18n.js";
import { money, parseYmd, myCurrencies } from "../format.js";
import { closeSheet, toast, update, state, useStore } from "../store.js";
import * as data from "../data.js";
import { evalAmount } from "../calc.js";
import { haptic, ask, tell } from "../tg.js";
import { Sheet, Seg, Chips, MoneyField, Empty, lastCurrency } from "../ui.js";
import { AccountPicker, activeAccounts } from "./accounts.js";
import { FIELDS, readText, parse, guessColumns, isOwnExport, toRows } from "../csv.js";

export const EVERY = ["week", "month", "year"];

const dayMonth = s => parseYmd(s).toLocaleDateString(locale(), { day: "numeric", month: "long", year: parseYmd(s).getFullYear() === new Date().getFullYear() ? undefined : "numeric" });

// ---------------------------------------------------------------------------
// Recurring operations
// ---------------------------------------------------------------------------
function RecurringItem({ r, onChanged }) {
  const [open, setOpen] = useState(false);
  const [amount, setAmount] = useState(String(r.amount).replace(".", ","));
  const [currency, setCurrency] = useState(r.currency);
  const [note, setNote] = useState(r.note || "");
  const accounts = activeAccounts(useStore());
  const [account, setAccount] = useState(r.account_id ?? null);
  // The account sets the currency; another currency by hand moves to an account in it.
  const pickAccount = id => {
    setAccount(id);
    const a = accounts.find(x => x.id === id);
    if (a) setCurrency(a.currency);
  };
  const pickCurrency = c => {
    setCurrency(c);
    if (accounts.find(x => x.id === account)?.currency !== c) setAccount(accounts.find(x => x.currency === c)?.id ?? null);
  };
  const title = r.subcategory_name ? `${r.category_name} · ${r.subcategory_name}` : r.category_name;
  const sign = r.type === "expense" ? "−" : "+";
  const save = async () => {
    const value = evalAmount(amount);
    if (!(value > 0)) return tell(t("form.enterAmount"));
    try {
      await data.updateRecurring(r.id, { amount: value, currency, note: note.trim(), account_id: account });
      haptic("success");
      toast(t("recurring.saved"));
      setOpen(false);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const stop = async () => {
    if (!(await ask(t("recurring.confirmStop")))) return;
    try {
      await data.stopRecurring(r.id);
      toast(t("recurring.stopped"));
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return html`<div class="recurring">
    <button type="button" class="row" onClick=${() => setOpen(!open)}>
      <span class="row-icon">${r.emoji}</span>
      <span class="row-text">
        <span class="row-title">${r.note || title}</span>
        <span class="row-sub">${r.note ? title + " · " : ""}${t("every." + r.every)} · ${t("recurring.next", { date: dayMonth(r.next_date) })}</span>
      </span>
      <span class=${"op-amount" + (r.type === "expense" ? "" : " op-income")}>${sign}${money(r.amount, r.currency)}</span>
    </button>
    ${open && html`<div class="form recurring-edit">
      <${MoneyField} value=${amount} onInput=${setAmount} currency=${currency} onCurrency=${pickCurrency} onEnter=${save} />
      <${AccountPicker} accounts=${accounts} value=${account} onChange=${pickAccount} />
      <input class="field" placeholder=${t("form.note")} value=${note} onInput=${e => setNote(e.currentTarget.value)} maxlength="200" />
      <div class="muted small">${t("recurring.fromNext")}</div>
      <button type="button" class="btn btn-primary" onClick=${save}>${t("common.save")}</button>
      <button type="button" class="btn btn-danger" onClick=${stop}>${t("recurring.stop")}</button>
    </div>`}
  </div>`;
}

function Recurring() {
  const [list, setList] = useState(null);
  const reload = () => data.loadRecurring().then(setList).catch(e => { setList([]); toast(errorText(e), "error"); });
  useEffect(() => { reload(); }, []);
  const changed = () => { reload(); data.refresh(); };
  return html`<${Sheet} title=${t("recurring.title")}>
    <div class="muted small sheet-hint">${t("recurring.hint")}</div>
    ${list === null ? html`<${Empty}>${t("common.loading")}<//>`
      : list.length === 0 ? html`<${Empty}>${t("recurring.empty")}<//>`
      : html`<div class="group">${list.map(r => html`<${RecurringItem} key=${r.id} r=${r} onChanged=${changed} />`)}</div>`}
  <//>`;
}

// ---------------------------------------------------------------------------
// CSV import
// ---------------------------------------------------------------------------
function ImportSheet() {
  const [file, setFile] = useState(null);
  const [lines, setLines] = useState(null);
  const [cols, setCols] = useState({});
  const [currency, setCurrency] = useState(lastCurrency());
  const [bySign, setBySign] = useState(true);
  const [preview, setPreview] = useState(null);
  const [busy, setBusy] = useState(false);
  const own = lines ? isOwnExport(lines[0]) : false;

  const pick = async e => {
    const f = e.currentTarget.files?.[0];
    if (!f) return;
    setPreview(null);
    try {
      const parsed = parse(await readText(f));
      if (parsed.length < 2) return tell(t("import.emptyFile"));
      setFile(f.name);
      setLines(parsed);
      const guessed = guessColumns(parsed[0]);
      setCols(guessed);
      // Without a type column, a file with minuses most likely marks expenses with them.
      setBySign(guessed.amount >= 0 && parsed.slice(1).some(l => /^\s*[-−(]/.test(l[guessed.amount] || "")));
    } catch (err) {
      toast(errorText(err), "error");
    }
  };

  const prepared = () => toRows(lines, cols, { own, currency, bySign });

  const check = async () => {
    if (cols.date < 0 || cols.amount < 0) return tell(t("import.needDateAmount"));
    const p = prepared();
    if (p.errors.length) {
      const e = p.errors[0];
      return setPreview({ errors: p.errors, first: e });
    }
    if (!p.rows.length) return tell(t("import.nothing"));
    setBusy(true);
    try {
      const res = await data.importRows(p.rows, true);
      setPreview({ ...res, rows: p.rows, skippedTransfers: p.skipped });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const run = async () => {
    setBusy(true);
    try {
      const res = await data.importRows(preview.rows, false);
      haptic("success");
      toast(t("import.done", { n: res.added }));
      closeSheet();
      update({ details: {} });
      data.refresh();
    } catch (e) {
      haptic("error");
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const header = lines?.[0] || [];
  const sample = lines?.[1] || [];
  return html`<${Sheet} title=${t("import.title")}>
    <div class="form">
      <div class="muted small">${t("import.hint")}</div>
      <label class="btn import-file">
        📄 ${file || t("import.choose")}
        <input type="file" accept=".csv,.txt,text/csv,text/plain" onChange=${pick} hidden />
      </label>
      ${lines && html`
        <div class="muted small">${t("import.lines", { n: lines.length - 1 })}${own ? " · " + t("import.own") : ""}</div>
        <div class="import-map">
          ${FIELDS.map(f => html`<label class="import-col">
            <span>${t("import.field." + f)}${f === "date" || f === "amount" ? " *" : ""}</span>
            <select class="field" value=${cols[f]} onChange=${e => { setCols({ ...cols, [f]: +e.currentTarget.value }); setPreview(null); }}>
              <option value="-1">—</option>
              ${header.map((h, i) => html`<option value=${i}>${h || t("import.column", { n: i + 1 })}${sample[i] ? ` (${String(sample[i]).slice(0, 18)})` : ""}</option>`)}
            </select>
          </label>`)}
        </div>
        ${cols.currency < 0 && html`<div class="muted small">${t("import.currencyAll")}</div>
          <${Chips} items=${myCurrencies(state).map(c => [c, c])} value=${currency} onChange=${c => { setCurrency(c); setPreview(null); }} />`}
        ${cols.type < 0 && html`<div class="muted small">${t("import.noType")}</div>
          <${Seg} options=${[[true, t("import.bySign")], [false, t("import.allExpenses")]]} value=${bySign} onChange=${v => { setBySign(v); setPreview(null); }} />`}
        ${!preview && html`<button type="button" class="btn btn-primary" disabled=${busy} onClick=${check}>${t("import.check")}</button>`}
      `}
      ${preview?.errors && html`<div class="auth-error">
        ${t("import.badLine", { line: preview.first.line, field: t("import.field." + preview.first.field), value: preview.first.value || "∅" })}
        ${preview.errors.length > 1 ? " " + t("import.moreBad", { n: preview.errors.length - 1 }) : ""}
      </div>`}
      ${preview && !preview.errors && html`<div class="card import-preview">
        <div>${t("import.willAdd", { n: preview.added })}</div>
        ${preview.skipped > 0 && html`<div class="muted small">${t("import.already", { n: preview.skipped })}</div>`}
        ${preview.skippedTransfers > 0 && html`<div class="muted small">${t("import.transfers", { n: preview.skippedTransfers })}</div>`}
        ${preview.created.length > 0 && html`<div class="muted small">${t("import.newCats")} ${preview.created.slice(0, 12).join(", ")}${preview.created.length > 12 ? "…" : ""}</div>`}
        <div class="muted small">${t("import.sample")} ${preview.rows.slice(0, 3).map(r => `${dayMonth(r.date)} ${r.type === "expense" ? "−" : "+"}${money(r.amount, r.currency)} ${r.category || ""}`).join(" · ")}</div>
      </div>
      ${preview.added > 0
        ? html`<button type="button" class="btn btn-primary" disabled=${busy} onClick=${run}>${t("import.run", { n: preview.added })}</button>`
        : html`<button type="button" class="btn" onClick=${closeSheet}>${t("import.close")}</button>`}`}
    </div>
  <//>`;
}

// ---------------------------------------------------------------------------
// Two-factor sign-in
// ---------------------------------------------------------------------------
function CodeInput({ value, onInput, onEnter, recovery }) {
  return html`<input class="field code-input" inputmode=${recovery ? "text" : "numeric"} autocomplete="one-time-code" maxlength="16"
    placeholder=${recovery ? t("tfa.codeOrRecovery") : t("tfa.code")} value=${value}
    onInput=${e => onInput(e.currentTarget.value)} onKeyDown=${e => { if (e.key === "Enter") { e.preventDefault(); onEnter?.(); } }} />`;
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t("tfa.copied"));
  } catch {
    tell(t("donate.copyByHand"));
  }
}

function RecoveryCodes({ codes }) {
  const text = codes.join("\n");
  const save = () => {
    const url = URL.createObjectURL(new Blob([text + "\n"], { type: "text/plain" }));
    Object.assign(document.createElement("a"), { href: url, download: "budget-recovery-codes.txt" }).click();
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
  };
  return html`<div class="form">
    <div class="muted small">${t("tfa.codesHint")}</div>
    <div class="recovery-codes">${codes.map(c => html`<code>${c}</code>`)}</div>
    <button type="button" class="btn" onClick=${() => copy(text)}>${t("donate.copy")}</button>
    <button type="button" class="btn" onClick=${save}>${t("tfa.saveFile")}</button>
    <button type="button" class="btn btn-primary" onClick=${closeSheet}>${t("tfa.savedThem")}</button>
  </div>`;
}

function TwoFactor() {
  const on = !!state.me?.totp;
  const [setup, setSetup] = useState(null);
  const [code, setCode] = useState("");
  const [codes, setCodes] = useState(null);
  const [busy, setBusy] = useState(false);
  const act = async fn => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      haptic("error");
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const begin = () => act(async () => setSetup(await data.twoFactorSetup()));
  const enable = () => act(async () => {
    const res = await data.twoFactorEnable(code);
    haptic("success");
    update(s => ({ me: { ...s.me, totp: true } }));
    setCodes(res.recovery_codes);
  });
  const disable = () => act(async () => {
    await data.twoFactorDisable(code);
    update(s => ({ me: { ...s.me, totp: false } }));
    toast(t("tfa.turnedOff"));
    closeSheet();
  });
  const regenerate = () => act(async () => {
    const res = await data.newRecoveryCodes(code);
    setCode("");
    setCodes(res.recovery_codes);
  });

  if (codes) return html`<${Sheet} title=${t("tfa.codesTitle")}><${RecoveryCodes} codes=${codes} /><//>`;
  return html`<${Sheet} title=${t("tfa.title")}>
    <div class="form">
      ${on ? html`
        <div class="tfa-state">✅ ${t("tfa.isOn")}</div>
        <div class="muted small">${t("tfa.confirmHint")}</div>
        <${CodeInput} value=${code} onInput=${setCode} recovery />
        <button type="button" class="btn" disabled=${busy || !code.trim()} onClick=${regenerate}>${t("tfa.newCodes")}</button>
        <button type="button" class="btn btn-danger" disabled=${busy || !code.trim()} onClick=${disable}>${t("tfa.turnOff")}</button>
      ` : !setup ? html`
        <div class="muted small">${t("tfa.about")}</div>
        <button type="button" class="btn btn-primary" disabled=${busy} onClick=${begin}>${t("tfa.turnOn")}</button>
      ` : html`
        <div class="muted small">${t("tfa.step1")}</div>
        <div class="tfa-qr" dangerouslySetInnerHTML=${{ __html: setup.qr_svg }}></div>
        <a class="btn" href=${setup.uri}>${t("tfa.openApp")}</a>
        <div class="muted small">${t("tfa.orType")}</div>
        <input class="field donate-address" readonly value=${setup.secret.replace(/(.{4})/g, "$1 ").trim()} onFocus=${e => e.currentTarget.select()} />
        <button type="button" class="btn" onClick=${() => copy(setup.secret)}>${t("donate.copy")}</button>
        <div class="muted small">${t("tfa.step2")}</div>
        <${CodeInput} value=${code} onInput=${setCode} onEnter=${enable} />
        <button type="button" class="btn btn-primary" disabled=${busy || code.replace(/\D/g, "").length !== 6} onClick=${enable}>${t("tfa.confirm")}</button>
      `}
    </div>
  <//>`;
}

export const TOOL_SHEETS = { recurring: Recurring, import: ImportSheet, twoFactor: TwoFactor };
