export const WINDOW_TYPE = (() => {
  const params = new URLSearchParams(window.location.search);
  return params.get("window") || "sidebar";
})();

export const IS_SIDEBAR = WINDOW_TYPE === "sidebar";
export const IS_TERMINAL = WINDOW_TYPE === "terminal";
