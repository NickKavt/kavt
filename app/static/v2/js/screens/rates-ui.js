// What exchange rates add to the screens when they are on: amounts in other currencies and a
// line with today's rates. Both stay hidden while rates are off or not loaded.

import { html } from "../../vendor/preact-htm.js";
import { t, locale } from "../i18n.js";
import { money, parseYmd } from "../format.js";
import { useStore } from "../store.js";
import { convert, googleLink } from "../rates.js";
import { openLink } from "../tg.js";

/** "≈ 30 600 ₽ · ≈ 1 000 $" for an amount, in the currencies chosen in Settings. */
export function Converted({ amount, currency }) {
  const { ratesPrefs: p, rates } = useStore();
  if (!p.on || !rates) return null;
  const parts = p.show.filter(c => c !== currency).map(c => [c, convert(amount, currency, c, rates)]).filter(([, v]) => v !== null);
  if (!parts.length) return null;
  return html`<span class="converted">${parts.map(([c, v]) => `≈ ${money(Math.round(v), c)}`).join(" · ")}</span>`;
}

/** "1 $ = 49,95 ₺ · 1 ₽ = 0,61 ₺", the source, its date and a link to compare on Google. */
export function RatesLine({ base }) {
  const { ratesPrefs: p, rates, ratesError } = useStore();
  if (!p.on) return null;
  if (!rates) return ratesError ? html`<div class="rates-line muted">${t("rates.unavailable")}</div>` : null;
  const list = p.show.filter(c => c !== base).map(c => [c, convert(1, c, base, rates)]).filter(([, v]) => v !== null);
  if (!list.length) return null;
  const date = parseYmd(rates.date).toLocaleDateString(locale(), { day: "numeric", month: "long" });
  const fmt = v => v.toLocaleString(locale(), { maximumFractionDigits: v < 10 ? 4 : 2 });
  return html`<div class="rates-line">
    <span>${list.map(([c, v]) => `1 ${c} = ${fmt(v)} ${base}`).join(" · ")}</span>
    <span class="muted">${t("rates.src." + rates.source)}, ${date}${rates.stale ? ` · ${t("rates.stale")}` : ""}</span>
    <button type="button" class="link-btn" onClick=${e => { e.stopPropagation(); openLink(googleLink(list[0][0], base)); }}>Google ›</button>
  </div>`;
}
