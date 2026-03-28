/// Runtime environment detection.
/// True when running inside a Tauri WebView, false in a plain browser.
export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const isBrowser = !isTauri;
