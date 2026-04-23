import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import { SettingsAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import SidebarApp from "../sidebar/App";
import TerminalApp from "../terminal/App";
import Titlebar from "../sidebar/components/Titlebar";
import QuitConfirmModal from "./components/QuitConfirmModal";
import "./styles/main.css";

const SIDEBAR_MIN_WIDTH = 200;
const SIDEBAR_MAX_WIDTH = 600;
const TERMINAL_MIN_WIDTH = 300;
const DEFAULT_SIDEBAR_WIDTH = 280;

function clampSidebarWidth(raw: number, windowWidth: number): number {
  const upper = Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, windowWidth - TERMINAL_MIN_WIDTH));
  return Math.max(SIDEBAR_MIN_WIDTH, Math.min(upper, raw));
}

const MainApp: Component = () => {
  const [sidebarWidth, setSidebarWidth] = createSignal(DEFAULT_SIDEBAR_WIDTH);
  const [dragging, setDragging] = createSignal(false);
  const [quitModalCount, setQuitModalCount] = createSignal<number | null>(null);

  const unlisteners: UnlistenFn[] = [];
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let quitInProgress = false;
  let splitterSaveTimeout: ReturnType<typeof setTimeout> | null = null;

  const persistWidth = (w: number) => {
    if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout);
    splitterSaveTimeout = setTimeout(async () => {
      try {
        const settings = await SettingsAPI.get();
        await SettingsAPI.update({ ...settings, mainSidebarWidth: w });
      } catch (e) {
        console.error("Failed to persist splitter width:", e);
      }
    }, 500);
  };

  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const divider = e.currentTarget as HTMLElement;
    try { divider.setPointerCapture(e.pointerId); } catch { /* some targets refuse capture */ }
    document.body.style.cursor = "col-resize";
    setDragging(true);

    const onMove = (m: PointerEvent) => {
      setSidebarWidth(clampSidebarWidth(m.clientX, window.innerWidth));
    };
    const onUp = (u: PointerEvent) => {
      try { divider.releasePointerCapture(u.pointerId); } catch { /* already released */ }
      document.body.style.cursor = "";
      setDragging(false);
      divider.removeEventListener("pointermove", onMove);
      divider.removeEventListener("pointerup", onUp);
      divider.removeEventListener("pointercancel", onUp);
      persistWidth(sidebarWidth());
    };
    divider.addEventListener("pointermove", onMove);
    divider.addEventListener("pointerup", onUp);
    divider.addEventListener("pointercancel", onUp);
  };

  // Stateless detached-window count (plan §A3B.3 / G3-B1 — must NOT read
  // sessionsStore because the store is Phase-2 and not authoritative anyway).
  async function countDetachedWindows(): Promise<number> {
    if (!isTauri) return 0;
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const all = await WebviewWindow.getAll();
    return all.filter((w) => w.label.startsWith("terminal-")).length;
  }

  const onModalCancel = () => setQuitModalCount(null);

  const onModalQuit = async () => {
    quitInProgress = true;
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      for (const w of await WebviewWindow.getAll()) {
        if (w.label.startsWith("terminal-")) {
          try { await w.destroy(); }
          catch (err) { console.warn("[quit] destroy of", w.label, "failed:", err); }
        }
      }
      try { await getCurrentWindow().destroy(); }
      catch (err) { console.warn("[quit] destroy of main failed:", err); }
    } finally {
      quitInProgress = false;
      setQuitModalCount(null);
    }
  };

  // Re-clamp splitter width when the OS resizes the window (e.g. monitor
  // disconnect, Win+Arrow snap). Without this the saved width can exceed
  // windowWidth - 300 and the terminal pane collapses (R2.5).
  const onWindowResize = () => {
    setSidebarWidth((w) => clampSidebarWidth(w, window.innerWidth));
  };

  onMount(async () => {
    document.documentElement.classList.add("light-theme");

    // Main window owns zoom + geometry persistence. Embedded Sidebar+Terminal
    // skip these initializers per DW.2.
    cleanupZoom = await initZoom("main");
    cleanupGeometry = await initWindowGeometry("main");

    // Load splitter width + always-on-top from settings.
    try {
      const settings = await SettingsAPI.get();
      const saved = settings.mainSidebarWidth ?? DEFAULT_SIDEBAR_WIDTH;
      setSidebarWidth(clampSidebarWidth(saved, window.innerWidth));
      if (isTauri && settings.mainAlwaysOnTop) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().setAlwaysOnTop(true);
      }
    } catch (e) {
      console.error("Failed to load main-window settings:", e);
    }

    window.addEventListener("resize", onWindowResize);

    // Quit-confirmation guard (plan §A3B.3 / G.13 / G3-M1).
    // - If 0 detached windows → let the close proceed (Tauri exits normally).
    // - If ≥1 detached → preventDefault, open custom modal.
    // Re-entry guard covers double-X / Alt+F4 while modal is already open.
    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlistenClose = await win.onCloseRequested(async (e) => {
        if (quitInProgress || quitModalCount() !== null) {
          e.preventDefault();
          return;
        }
        const count = await countDetachedWindows();
        if (count === 0) return; // silent quit path
        e.preventDefault();
        setQuitModalCount(count);
      });
      unlisteners.push(unlistenClose);
    }
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
    if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout);
    window.removeEventListener("resize", onWindowResize);
  });

  return (
    <div class="main-root" classList={{ "main-dragging": dragging() }}>
      <Titlebar />
      <div class="main-body">
        <div class="main-sidebar-pane" style={{ width: `${sidebarWidth()}px` }}>
          <SidebarApp embedded />
        </div>
        <div
          class="main-divider"
          classList={{ dragging: dragging() }}
          onPointerDown={onPointerDown}
          role="separator"
          aria-orientation="vertical"
        />
        <div class="main-terminal-pane">
          <TerminalApp embedded />
        </div>
      </div>
      <Show when={quitModalCount() !== null}>
        <QuitConfirmModal
          detachedCount={quitModalCount()!}
          onCancel={onModalCancel}
          onQuit={onModalQuit}
        />
      </Show>
    </div>
  );
};

export default MainApp;
