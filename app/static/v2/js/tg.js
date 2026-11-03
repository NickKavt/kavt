// Telegram Mini App: start-up, haptics, the Back button and native dialogs.
// Outside Telegram (a browser during development) every call falls back to the web.

export const tg = window.Telegram?.WebApp || null;

const BG = "#1a1b26";

export function initTelegram() {
  if (!tg) return;
  try {
    tg.ready();
    tg.expand();
    tg.setHeaderColor?.(BG);
    tg.setBackgroundColor?.(BG);
    // Scrolling a long list must not close the app.
    if (tg.isVersionAtLeast?.("7.7")) tg.disableVerticalSwipes?.();
  } catch (e) {
    console.warn("Telegram init failed", e);
  }
}

export function initData() {
  try {
    return tg?.initData || "";
  } catch {
    return "";
  }
}

export function telegramUser() {
  try {
    return tg?.initDataUnsafe?.user || null;
  } catch {
    return null;
  }
}

export function haptic(kind = "light") {
  try {
    if (kind === "success" || kind === "error" || kind === "warning") tg?.HapticFeedback?.notificationOccurred(kind);
    else tg?.HapticFeedback?.impactOccurred(kind);
  } catch {}
}

const nativeDialogs = () => !!tg?.isVersionAtLeast?.("6.2");

/** Yes/no question; resolves to true when confirmed. */
export function ask(message) {
  if (nativeDialogs()) return new Promise(resolve => tg.showConfirm(message, ok => resolve(!!ok)));
  return Promise.resolve(window.confirm(message));
}

export function tell(message) {
  if (nativeDialogs()) return new Promise(resolve => tg.showAlert(message, () => resolve()));
  window.alert(message);
  return Promise.resolve();
}

let backHandler = null;

/** Shows Telegram's Back button with this handler, or hides it for null. */
export function setBack(handler) {
  if (!tg?.BackButton) return;
  if (backHandler) tg.BackButton.offClick(backHandler);
  backHandler = handler;
  if (handler) {
    tg.BackButton.onClick(handler);
    tg.BackButton.show();
  } else {
    tg.BackButton.hide();
  }
}

/** Opens a web page outside the app: Telegram's browser inside Telegram, a new tab elsewhere. */
export function openLink(url) {
  if (tg?.initData && tg.openLink) tg.openLink(url);
  else window.open(url, "_blank", "noopener");
}

/** Opens a t.me link inside Telegram itself (a bot, a chat, a Mini App). */
export function openTelegramLink(url) {
  if (tg?.initData && tg.openTelegramLink) tg.openTelegramLink(url);
  else window.open(url, "_blank", "noopener");
}
