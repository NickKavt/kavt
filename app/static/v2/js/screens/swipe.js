// The period bar: drag the title sideways to flip to the previous (right) or next (left) period.
// Only the bar listens, so a sideways gesture anywhere else on the screen does nothing. The faint
// arrows are a hint that it moves; a click on them still flips it, for a mouse.

import { html, useRef } from "../../vendor/preact-htm.js";
import { t } from "../i18n.js";
import { haptic } from "../tg.js";

const FLIP = 50; // px of drag that flips the period
const START = 8; // px before the gesture counts as a drag either way

/** `hint`: a line under the title; false keeps its place empty, so the screen does not jump. */
export function PeriodBar({ title, hint, onTitle, onShift }) {
  const label = useRef(null);
  const drag = useRef(null);

  const place = dx => {
    const el = label.current;
    if (!el) return;
    el.style.transform = dx ? `translateX(${dx}px)` : "";
    el.style.opacity = dx ? String(Math.max(0.35, 1 - Math.abs(dx) / 220)) : "";
  };
  const slide = (from, to) => label.current?.animate?.([from, to], { duration: 180, easing: "ease-out" });

  const flip = n => {
    haptic();
    onShift(n);
    // The new title comes in from the side it was pushed to.
    slide({ transform: `translateX(${n * 36}px)`, opacity: 0 }, { transform: "none", opacity: 1 });
  };

  const down = e => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    drag.current = { id: e.pointerId, x: e.clientX, y: e.clientY, dx: 0, on: false, done: false };
  };
  const move = e => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId || d.done) return;
    const dx = e.clientX - d.x, dy = e.clientY - d.y;
    if (!d.on) {
      if (Math.abs(dy) > START && Math.abs(dy) > Math.abs(dx)) return (d.done = true);
      if (Math.abs(dx) < START) return;
      d.on = true;
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        // The pointer is already gone: the drag still works while it stays on the bar.
      }
    }
    d.dx = dx;
    place(dx * 0.6);
  };
  const up = e => {
    const d = drag.current;
    if (!d || d.id !== e.pointerId) return;
    if (!d.on) return (drag.current = null);
    // A drag ends with a click on whatever is under the finger: that one must not count.
    drag.current = { moved: true };
    place(0);
    if (Math.abs(d.dx) > FLIP) flip(d.dx < 0 ? 1 : -1);
    else slide({ transform: `translateX(${d.dx * 0.6}px)` }, { transform: "none" });
  };
  const click = fn => () => {
    if (drag.current?.moved) return (drag.current = null);
    fn?.();
  };

  return html`<div class="monthbar" onPointerDown=${down} onPointerMove=${move} onPointerUp=${up} onPointerCancel=${up}>
    <button type="button" class="monthbar-arrow" tabindex="-1" onClick=${click(() => flip(-1))} aria-label=${t("nav.prev")}>‹</button>
    <button type="button" class="monthbar-title" ref=${label} onClick=${click(onTitle)}>
      ${title}${hint !== undefined && html`<span class=${"monthbar-hint" + (hint ? "" : " off")}>${hint || "\u00a0"}</span>`}
    </button>
    <button type="button" class="monthbar-arrow" tabindex="-1" onClick=${click(() => flip(1))} aria-label=${t("nav.next")}>›</button>
  </div>`;
}
