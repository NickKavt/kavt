// Checks the new interface's texts: every key used in the code exists in every
// language, and no language has keys the others lack. Exits with 1 on any gap,
// which stops deploy/redeploy.sh.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "app", "static", "v2", "js");
const langs = readdirSync(join(root, "locales")).filter(f => f.endsWith(".js")).map(f => f.slice(0, -3));
const dicts = {};
for (const l of langs) dicts[l] = (await import(pathToFileURL(join(root, "locales", `${l}.js`)))).default;

const files = [];
(function walk(dir) {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) { if (f !== "locales") walk(p); } else if (f.endsWith(".js")) files.push(p);
  }
})(root);

const used = new Set();
const prefixes = new Set();
for (const f of files) {
  const src = readFileSync(f, "utf8");
  for (const m of src.matchAll(/\bt\("([\w.]*\w)"/g)) used.add(m[1]);
  for (const m of src.matchAll(/\bt\("([\w.]+\.)"\s*\+/g)) prefixes.add(m[1]);
  for (const m of src.matchAll(/"(err\.\w+)"/g)) used.add(m[1]);
}

const problems = [];
// Every language the interface offers has a file, and every file is offered.
const offered = [...readFileSync(join(root, "i18n.js"), "utf8").matchAll(/^  (\w\w): \{ name:/gm)].map(m => m[1]);
for (const l of offered) if (!langs.includes(l)) problems.push(`${l}: язык есть в i18n.js, но нет locales/${l}.js`);
for (const l of langs) if (!offered.includes(l)) problems.push(`${l}: locales/${l}.js не подключён в i18n.js`);
// A plural text has the forms its language needs. "many" in Spanish, Portuguese, French and
// Italian is for millions ("1 millón de"); "other" serves there.
for (const l of langs) {
  const forms = new Intl.PluralRules(l).resolvedOptions().pluralCategories.filter(f => !(f === "many" && ["es", "pt", "fr", "it"].includes(l)));
  for (const [key, v] of Object.entries(dicts[l])) {
    if (typeof v === "object") for (const f of forms) if (!(f in v)) problems.push(`${l}: в "${key}" нет формы "${f}"`);
  }
}
for (const key of used) for (const l of langs) if (!(key in dicts[l])) problems.push(`${l}: нет перевода для "${key}"`);
const all = new Set(langs.flatMap(l => Object.keys(dicts[l])));
for (const key of all) {
  for (const l of langs) if (!(key in dicts[l])) problems.push(`${l}: нет ключа "${key}", который есть в других языках`);
  if (!used.has(key) && ![...prefixes].some(p => key.startsWith(p))) problems.push(`ключ "${key}" нигде не используется`);
}

if (problems.length) {
  console.error("Проверка переводов не прошла:\n  " + [...new Set(problems)].join("\n  "));
  process.exit(1);
}
console.log(`Переводы в порядке: ${used.size} ключей в ${langs.length} языках`);
