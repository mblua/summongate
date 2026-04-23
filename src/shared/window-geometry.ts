import { SettingsAPI } from "./ipc";
import type { AppSettings, WindowGeometry } from "./types";
import { isTauri } from "./platform";

type WindowType = "sidebar" | "terminal" | "main";

const geometryKeyMap: Record<WindowType, keyof AppSettings> = {
  sidebar: "sidebarGeometry",
  terminal: "terminalGeometry",
  main: "mainGeometry",
};

async function readGeometry(): Promise<WindowGeometry> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  const pos = await win.outerPosition();
  const size = await win.outerSize();
  return {
    x: pos.x,
    y: pos.y,
    width: size.width,
    height: size.height,
  };
}

/**
 * Track window move/resize and persist geometry.
 * Returns a cleanup function for onCleanup.
 *
 * Per DW.7: saveTimeout is closure-local so a main window can run this
 * alongside an independent splitter-width debouncer without racing.
 */
export async function initWindowGeometry(
  windowType: WindowType
): Promise<() => void> {
  if (!isTauri) {
    // Browser: no window geometry tracking
    return () => {};
  }

  let saveTimeout: ReturnType<typeof setTimeout> | null = null;
  const key = geometryKeyMap[windowType];

  const debouncedSave = () => {
    if (saveTimeout) clearTimeout(saveTimeout);
    saveTimeout = setTimeout(async () => {
      try {
        const geo = await readGeometry();
        const settings = await SettingsAPI.get();
        await SettingsAPI.update({ ...settings, [key]: geo });
      } catch (e) {
        console.error("Failed to save window geometry:", e);
      }
    }, 500);
  };

  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();

  const unlistenMove = await win.onMoved(() => debouncedSave());
  const unlistenResize = await win.onResized(() => debouncedSave());

  return () => {
    unlistenMove();
    unlistenResize();
    if (saveTimeout) clearTimeout(saveTimeout);
  };
}

