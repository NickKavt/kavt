// Sideways swipe on a screen flips to the previous (right swipe) or next (left) period.

import { haptic } from "../tg.js";

export function useSwipe(onShift) {
  let x0 = null, y0 = 0;
  return {
    onTouchStart: e => {
      const el = e.target;
      if (e.touches.length !== 1 || el.closest("input, textarea, .chips, .seg")) return (x0 = null);
      x0 = e.touches[0].clientX;
      y0 = e.touches[0].clientY;
    },
    onTouchEnd: e => {
      if (x0 === null) return;
      const p = e.changedTouches[0], dx = p.clientX - x0, dy = p.clientY - y0;
      x0 = null;
      if (Math.abs(dx) > 70 && Math.abs(dx) > Math.abs(dy) * 1.8) {
        haptic();
        onShift(dx < 0 ? 1 : -1);
      }
    },
  };
}
