// Reading CSV files for the import: splitting into cells, guessing which column is which,
// and turning each line into the form the server imports (src/import.rs). Exports of this
// app, Firefly III, Actual Budget, ezBookkeeping and banks differ in names, separators,
// date and number formats; all of that is handled here, so the server sees one shape.

export const FIELDS = ["date", "amount", "type", "category", "subcategory", "currency", "note"];

// Lowercased header names each field is recognised by, best first.
const NAMES = {
  date: ["date", "дата", "tarih", "time", "время", "transaction date", "booking date", "дата операции", "işlem tarihi"],
  amount: ["amount", "сумма", "tutar", "sum", "сумма операции", "miktar"],
  type: ["type", "тип", "tür", "işlem türü", "transaction type"],
  category: ["category", "категория", "kategori", "category_name"],
  subcategory: ["subcategory", "sub category", "подкатегория", "alt kategori"],
  currency: ["currency", "currency_code", "валюта", "para birimi", "account currency", "валюта операции"],
  note: ["note", "description", "описание", "notes", "комментарий", "payee", "açıklama", "назначение платежа"],
};

/** Text of a file: UTF-8, or the Windows Cyrillic encoding many Russian banks still use. */
export async function readText(file) {
  const bytes = await file.arrayBuffer();
  const utf8 = new TextDecoder("utf-8").decode(bytes);
  return utf8.includes("�") ? new TextDecoder("windows-1251").decode(bytes) : utf8;
}

/** Lines of cells. The separator is whichever of , ; or tab the first line has most of. */
export function parse(text) {
  text = text.replace(/^﻿/, "");
  const first = text.slice(0, text.search(/\r?\n|$/));
  const sep = [",", ";", "\t"].sort((a, b) => first.split(b).length - first.split(a).length)[0];
  const rows = [];
  let row = [], cell = "", quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { cell += '"'; i++; }
      else if (c === '"') quoted = false;
      else cell += c;
    } else if (c === '"') quoted = true;
    else if (c === sep) { row.push(cell); cell = ""; }
    else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(cell); cell = "";
      if (row.some(x => x.trim() !== "")) rows.push(row);
      row = [];
    } else cell += c;
  }
  row.push(cell);
  if (row.some(x => x.trim() !== "")) rows.push(row);
  return rows;
}

/** This app's own export, whose types are read as they are. */
export const isOwnExport = header => ["date", "type", "category", "subcategory", "amount", "currency", "note", "author"].every((n, i) => header[i]?.trim().toLowerCase() === n);

/** For each field, the index of its column, or -1. */
export function guessColumns(header) {
  const names = header.map(h => h.trim().toLowerCase());
  const taken = new Set();
  const out = {};
  for (const f of FIELDS) {
    const at = NAMES[f].map(n => names.findIndex((h, i) => h === n && !taken.has(i))).find(i => i >= 0) ?? -1;
    out[f] = at;
    if (at >= 0) taken.add(at);
  }
  return out;
}

/** A number as people write it: "1 234,56", "1,234.56", "-50", "(50)", "₺120". */
export function parseAmount(raw) {
  let s = String(raw ?? "").trim().replace(/[\s  ']/g, "");
  let negative = /^\(.*\)$/.test(s) || /^[-−]/.test(s) || /[-−]$/.test(s);
  s = s.replace(/[^\d.,]/g, "");
  if (!s) return NaN;
  const lastComma = s.lastIndexOf(","), lastDot = s.lastIndexOf(".");
  if (lastComma >= 0 && lastDot >= 0) {
    // Whichever comes last separates the decimals; the other groups thousands.
    s = lastComma > lastDot ? s.replace(/\./g, "").replace(",", ".") : s.replace(/,/g, "");
  } else if (lastComma >= 0) {
    s = s.split(",").length > 2 ? s.replace(/,/g, "") : s.replace(",", ".");
  } else if (s.split(".").length > 2) {
    s = s.replace(/\./g, "");
  }
  const v = Number(s);
  return negative ? -v : v;
}

/**
 * Dates of a column as `YYYY-MM-DD`. Day-first or month-first is decided once for the whole
 * column: a part above 12 tells which; otherwise day-first, as in Europe, Russia and Türkiye.
 */
export function dateReader(values) {
  let monthFirst = false;
  for (const v of values) {
    const m = String(v).trim().match(/^(\d{1,2})[./-](\d{1,2})[./-](\d{2,4})/);
    if (m && +m[1] <= 12 && +m[2] > 12) monthFirst = true;
    if (m && +m[1] > 12) { monthFirst = false; break; }
  }
  return raw => {
    const s = String(raw ?? "").trim();
    let y, mo, d, m;
    if ((m = s.match(/^(\d{4})[-./](\d{1,2})[-./](\d{1,2})/))) [, y, mo, d] = m;
    else if ((m = s.match(/^(\d{1,2})[./-](\d{1,2})[./-](\d{2,4})/))) {
      [, d, mo, y] = m;
      if (monthFirst) [d, mo] = [mo, d];
      if (y.length === 2) y = "20" + y;
    } else return null;
    const date = new Date(Date.UTC(+y, +mo - 1, +d));
    if (date.getUTCMonth() !== +mo - 1 || date.getUTCDate() !== +d) return null;
    return `${y}-${String(mo).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
  };
}

// Codes become the symbols the app shows; the server does the same (src/import.rs).
const SYMBOLS = { TRY: "₺", TL: "₺", USD: "$", EUR: "€", RUB: "₽", RUR: "₽", "РУБ": "₽" };
const symbol = c => SYMBOLS[c.toUpperCase()] || c;

const OWN_TYPES = new Set(["expense", "income", "savings", "withdrawal", "interest", "transfer", "opening"]);
// Columns only this app's export has: accounts, and a transfer's other end.
const OWN_EXTRA = ["account", "to_account", "to_amount", "to_currency"];
const EXPENSE = new Set(["expense", "withdrawal", "расход", "расходы", "gider", "списание", "payment"]);
const INCOME = new Set(["income", "deposit", "доход", "доходы", "gelir", "зачисление", "пополнение"]);
// Moves between one's own accounts and balance corrections aren't spending or income here.
const SKIP = new Set(["transfer", "перевод", "transfer in", "transfer out", "balance modification", "opening balance", "reconciliation", "transfer"]);

/**
 * The rows to import and the problems found. `cols` maps fields to column indexes;
 * `opts.currency` is used without a currency column, and `opts.bySign` decides the type
 * without a type column: a minus is an expense and a plus an income, or else all are expenses.
 */
export function toRows(lines, cols, { own, currency, bySign }) {
  const body = lines.slice(1);
  const readDate = dateReader(cols.date >= 0 ? body.map(l => l[cols.date]) : []);
  const get = (l, f) => (cols[f] >= 0 ? String(l[cols[f]] ?? "").trim() : "");
  const header = lines[0].map(h => String(h).trim().toLowerCase());
  const extra = (l, f) => {
    const i = own ? header.indexOf(f) : -1;
    return i >= 0 ? String(l[i] ?? "").trim() : "";
  };
  const rows = [], errors = [];
  let skipped = 0;
  body.forEach((l, i) => {
    const line = i + 2;
    const date = readDate(get(l, "date"));
    const amount = parseAmount(get(l, "amount"));
    if (!date) return errors.push({ line, field: "date", value: get(l, "date") });
    const rawType = get(l, "type").toLowerCase();
    // An account's starting balance may be nothing, or below zero (a card in debt).
    const opening = own && rawType === "opening";
    if (!Number.isFinite(amount) || (amount === 0 && !opening)) return errors.push({ line, field: "amount", value: get(l, "amount") });
    let type;
    if (own && OWN_TYPES.has(rawType)) type = rawType;
    else if (SKIP.has(rawType)) return skipped++;
    else if (EXPENSE.has(rawType)) type = "expense";
    else if (INCOME.has(rawType)) type = "income";
    else type = bySign && amount > 0 ? "income" : "expense";
    const row = {
      date, type, amount: opening ? amount : Math.abs(amount),
      category: get(l, "category"), subcategory: get(l, "subcategory"),
      currency: symbol(get(l, "currency")) || currency, note: get(l, "note"),
    };
    if (own) {
      for (const f of OWN_EXTRA) row[f] = extra(l, f);
      row.to_amount = row.to_amount ? parseAmount(row.to_amount) : null;
      if (row.to_currency) row.to_currency = symbol(row.to_currency);
    }
    rows.push(row);
  });
  return { rows, errors, skipped };
}
