// UI scale (settings → appearance) as a real page zoom.
//
// It used to be CSS `zoom` on <html>, which leaves the page with two coordinate systems: pointer
// events and getBoundingClientRect() report one, while layout, scroll offsets and every px value
// the code works with (beat widths, row heights) use another. Any `clientX - rect.left` then
// lands in the wrong place — at 150% a click in the piano roll put the note off from the cursor.
// The webview's own zoom (WKWebView pageZoom, WebView2 ZoomFactor, WebKitGTK zoom level) is the
// same zoom a browser's ⌘+ applies: one coordinate system, so nothing downstream has to know.
//
// In a plain browser (`pnpm ui`) a page can't set its own zoom, so the setting is stored but the
// browser's zoom is the way to scale there.

import { useCallback, useSyncExternalStore } from "react";

/** True when the app runs in the Tauri webview, where the scale can be applied natively. */
export const UI_SCALE_NATIVE = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** The zoom this page has applied; null until the first apply (the webview may already be zoomed
 *  from before a reload, so the first apply always goes through). */
let scale: number | null = null;
/**
 * Whether `devicePixelRatio` already includes the page zoom. Chromium (WebView2 on Windows) folds
 * it in; WebKit (macOS, Linux) keeps reporting the screen's ratio. Guessed from the engine, then
 * measured the first time the zoom really changes.
 */
let dprIncludesZoom = typeof navigator !== "undefined" && /Chrome\//.test(navigator.userAgent);
let measured = false;
let applying: Promise<void> = Promise.resolve();
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

/** Apply a UI scale (1 = 100%). Safe to call repeatedly; calls are applied in order. */
export function applyUiScale(next: number): Promise<void> {
  applying = applying.then(() => setScale(next)).catch((e) => console.warn("ui scale unavailable", e));
  return applying;
}

async function setScale(next: number) {
  if (!UI_SCALE_NATIVE || next === scale) return;
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const before = window.devicePixelRatio || 1;
  const prev = scale;
  await getCurrentWebview().setZoom(next);
  scale = next;
  if (prev !== null && !measured) {
    dprIncludesZoom = await dprFollows(before, next / prev);
    measured = true;
  }
  notify();
}

/** Wait a few frames for devicePixelRatio to move by `ratio`; false if it never does. */
async function dprFollows(before: number, ratio: number): Promise<boolean> {
  for (let i = 0; i < 12; i++) {
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    const now = window.devicePixelRatio || 1;
    if (Math.abs(now / before - ratio) < 0.02) return true;
  }
  return false;
}

/**
 * Device pixels per CSS pixel, page zoom included: the factor a canvas's backing store needs to
 * stay sharp at any UI scale.
 */
export function canvasPixelRatio(): number {
  const dpr = window.devicePixelRatio || 1;
  return dprIncludesZoom ? dpr : dpr * (scale ?? 1);
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  // devicePixelRatio also changes when the window moves to another screen.
  let mq: MediaQueryList | null = null;
  const watch = () => {
    mq?.removeEventListener("change", onChange);
    mq = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    mq.addEventListener("change", onChange);
  };
  const onChange = () => {
    watch();
    cb();
  };
  watch();
  return () => {
    listeners.delete(cb);
    mq?.removeEventListener("change", onChange);
  };
}

/** The current canvasPixelRatio(), re-rendering when the UI scale or the screen changes. */
export function usePixelRatio(): number {
  return useSyncExternalStore(subscribe, canvasPixelRatio, () => 1);
}

/** Whether a media query matches, re-rendering when it changes (window resize, ui scale). */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (cb: () => void) => {
      const mq = window.matchMedia(query);
      mq.addEventListener("change", cb);
      return () => mq.removeEventListener("change", cb);
    },
    [query],
  );
  return useSyncExternalStore(subscribe, () => window.matchMedia(query).matches, () => false);
}
