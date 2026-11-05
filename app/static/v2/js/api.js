// Requests to the budget server. The session lives in an HttpOnly cookie. Inside Telegram a
// missing or expired session is renewed with Telegram's initData and the request repeated
// once; in a browser the app shows the sign-in screen instead.

import { initData } from "./tg.js";
import { state, update } from "./store.js";

export class ApiError extends Error {
  constructor(status, message) {
    super(message || `HTTP ${status}`);
    this.status = status;
  }
}

async function request(path, { method = "GET", body } = {}) {
  if (!navigator.onLine) throw new ApiError(0, "offline");
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new ApiError(res.status, (await res.text()).trim());
  if (res.status === 204) return null;
  const type = res.headers.get("content-type") || "";
  return type.includes("json") ? res.json() : res.text();
}

let signingIn = null;

export function signIn() {
  const data = initData();
  if (!data) return Promise.reject(new ApiError(401, "need-login"));
  signingIn ||= request("/api/auth/telegram", { method: "POST", body: { init_data: data } }).finally(() => (signingIn = null));
  return signingIn;
}

export async function api(path, options) {
  try {
    return await request(path, options);
  } catch (e) {
    // Signing in itself answers 401 for a wrong password; that is not an expired session.
    if (e.status !== 401 || /^\/api\/(auth|setup|invite)\b/.test(path)) throw e;
    try {
      await signIn();
    } catch (err) {
      // In a browser an expired session leads to the sign-in screen.
      if (err.message === "need-login") update({ auth: { mode: state.server?.demo ? "demo" : "login" } });
      throw err;
    }
    return request(path, options);
  }
}
