// Interface language. Code refers to texts by key, t("home.addExpense"), and each
// language has its own file in locales/. A text that varies with a number is an object
// of plural forms ({ one, few, many, other }), chosen by Intl.PluralRules.

import ru from "./locales/ru.js";

// The same languages as the server's (src/db.rs, LANGS), each named in itself. Only the chosen
// one is downloaded; Russian is always here, for a text another language lacks.
export const LANGS = {
  en: { name: "English", locale: "en-GB", load: () => import("./locales/en.js") },
  es: { name: "Español", locale: "es-ES", load: () => import("./locales/es.js") },
  pt: { name: "Português (Brasil)", locale: "pt-BR", load: () => import("./locales/pt.js") },
  fr: { name: "Français", locale: "fr-FR", load: () => import("./locales/fr.js") },
  de: { name: "Deutsch", locale: "de-DE", load: () => import("./locales/de.js") },
  it: { name: "Italiano", locale: "it-IT", load: () => import("./locales/it.js") },
  pl: { name: "Polski", locale: "pl-PL", load: () => import("./locales/pl.js") },
  tr: { name: "Türkçe", locale: "tr-TR", load: () => import("./locales/tr.js") },
  id: { name: "Bahasa Indonesia", locale: "id-ID", load: () => import("./locales/id.js") },
  ru: { name: "Русский", locale: "ru-RU", load: () => import("./locales/ru.js") },
  uk: { name: "Українська", locale: "uk-UA", load: () => import("./locales/uk.js") },
  be: { name: "Беларуская", locale: "be-BY", load: () => import("./locales/be.js") },
  hi: { name: "हिन्दी", locale: "hi-IN", load: () => import("./locales/hi.js") },
  zh: { name: "中文（简体）", locale: "zh-CN", load: () => import("./locales/zh.js") },
  ja: { name: "日本語", locale: "ja-JP", load: () => import("./locales/ja.js") },
};
const dicts = { ru };

/** Downloads a language's texts once. */
export async function loadLang(code) {
  if (!dicts[code]) dicts[code] = (await LANGS[code].load()).default;
}

// The choice is kept in this browser.
const KEY = "budget:lang";

function saved() {
  try {
    const v = localStorage.getItem(KEY);
    if (v && LANGS[v]) return v;
  } catch {}
  return browserLang();
}

/** The browser's language when the app has it, else English. */
function browserLang() {
  for (const code of navigator.languages?.length ? navigator.languages : [navigator.language || ""]) {
    const l = code.slice(0, 2).toLowerCase();
    if (LANGS[l]) return l;
  }
  return "en";
}

let lang = saved();
document.documentElement.lang = lang;

export const currentLang = () => lang;
export const locale = () => LANGS[lang].locale;

/** Switches the interface to `next` once its texts are here. */
export async function setLang(next) {
  if (!LANGS[next]) return;
  await loadLang(next);
  lang = next;
  document.documentElement.lang = next;
  try {
    localStorage.setItem(KEY, next);
  } catch {}
}

export function t(key, vars) {
  let text = dicts[lang]?.[key] ?? ru[key];
  if (text === undefined) {
    console.warn("missing text", key);
    return key;
  }
  if (typeof text === "object") {
    const form = new Intl.PluralRules(locale()).select(Number(vars?.n ?? 0));
    text = text[form] ?? text.other;
  }
  if (vars) for (const [k, v] of Object.entries(vars)) text = text.split(`{${k}}`).join(String(v));
  return text;
}

// The server answers errors in Russian; known ones are shown in the chosen language.
const SERVER_ERRORS = {
  "Категория не найдена": "err.categoryNotFound",
  "Подкатегория не найдена": "err.subcategoryNotFound",
  "Операция не найдена": "err.operationNotFound",
  "Пользователь не найден": "err.userNotFound",
  "Сессия не найдена": "err.sessionNotFound",
  "Неизвестная роль": "err.unknownRole",
  "В демо это недоступно": "err.demoOff",
  "Демо сейчас занято, попробуйте позже": "err.demoFull",
  "Недостаточно прав": "err.noRights",
  "Нельзя снять права с хозяина бюджета": "err.ownerKeepsRights",
  "Нельзя удалить owner": "err.cantRemoveOwner",
  "Нельзя удалить самого себя": "err.cantRemoveSelf",
  "Нельзя удалить хозяина бюджета": "err.cantRemoveBudgetOwner",
  "Нельзя перенести owner в другой бюджет": "err.cantMoveOwner",
  "Owner уже существует": "err.ownerExists",
  "Системную категорию удалять нельзя": "err.systemCategory",
  "Такая категория уже есть": "err.categoryExists",
  "Название категории не может быть пустым": "err.emptyName",
  "Неизвестный тип категории": "err.unknownType",
  "Сумма должна быть больше нуля": "err.amountPositive",
  "Неизвестная валюта": "err.unknownCurrency",
  "Начало периода позже конца": "err.periodOrder",
  "Период больше 10 лет": "err.periodTooLong",
  "Счёт не найден": "err.accountNotFound",
  "Логин: от 3 до 32 латинских букв, цифр и знаков . _ -": "err.badLogin",
  "Пароль — не короче 8 символов": "err.shortPassword",
  "Неверный логин или пароль": "err.wrongLogin",
  "Такой логин уже занят": "err.loginTaken",
  "Приглашение не найдено или уже использовано": "err.inviteGone",
  "Текущий пароль неверный": "err.wrongCurrent",
  "У вас вход через Telegram, пароля нет": "err.noPassword",
  "На счёте нет такой суммы": "err.notEnough",
  "Выберите два разных счёта": "err.sameAccount",
  "Ставка должна быть от 0 до 1000%": "err.rateRange",
  "Налог должен быть от 0 до 100%": "err.taxRange",
  "Срок вклада — от 1 до 3660 дней": "err.termRange",
  "Дата не может быть в будущем": "err.futureDate",
  "Дата начала — не раньше чем 10 лет назад": "err.startTooOld",
  "Введите @username (латиница, цифры и _, от 4 до 32 символов), Telegram ID или ссылку t.me": "err.badUsername",
  "Нужен код из приложения": "err.codeNeeded",
  "Неверный код": "err.wrongCode",
  "Двухэтапный вход уже включён": "err.tfaOn",
  "Сначала добавьте ключ в приложение": "err.tfaPending",
  "Двухэтапный вход выключен": "err.tfaOff",
  "У вас вход через Telegram, второй шаг не нужен": "err.tfaTelegram",
  "Регулярная операция не найдена": "err.recurringNotFound",
  "Неизвестный период": "err.unknownPeriod",
  "Слишком много строк, разделите файл": "err.tooManyRows",
};

/** A message for any failed request, in the chosen language. */
export function errorText(e) {
  if (!e) return t("err.unknown");
  if (e.status === 0) return t("err.offline");
  if (e.status === 429) return t("err.tooOften");
  // A known answer says more than its status: a wrong password is a 401 too.
  const known = SERVER_ERRORS[e.message];
  if (known) return t(known);
  if (e.status === 401) return t("err.sessionExpired");
  if (e.status >= 500 || !e.message) return t("err.server");
  return e.message;
}
