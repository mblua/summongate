import { Component, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import { isTauri } from "../shared/platform";
import {
  SessionAPI,
  WindowAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
  onThemeChanged,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry, initDetachedWindowGeometry } from "../shared/window-geometry";
import { settingsStore } from "../shared/stores/settings";
import { terminalStore } from "./stores/terminal";
import Titlebar from "./components/Titlebar";
import LastPrompt from "./components/LastPrompt";
import TerminalView from "./components/TerminalView";
import StatusBar from "./components/StatusBar";
import "./styles/terminal.css";

interface TerminalAppProps {
  lockedSessionId?: string;
  detached?: boolean;
  /**
   * True when mounted inside MainApp's unified layout. Skips titlebar
   * render, window-level initializers, and redundant theme listener
   * (DW.2 + DW.5 + Arb-4).
   */
  embedded?: boolean;
}

const TerminalApp: Component<TerminalAppProps> = (props) => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;

  const loadActiveSession = async () => {
    if (props.lockedSessionId) {
      // Detached mode: lock to specific session
      const sessions = await SessionAPI.list();
      const session = sessions.find((s) => s.id === props.lockedSessionId);
      if (session) {
        terminalStore.setActiveSession(session.id, session.name, session.shell, session.effectiveShellArgs, session.workingDirectory);
      } else {
        // Session no longer exists, close this window
        terminalStore.setActiveSession(null, "", "", null, "");
      }
      return;
    }

    // Normal mode: follow active session
    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell, active.effectiveShellArgs, active.workingDirectory);
      }
    } else {
      terminalStore.setActiveSession(null, "", "", null, "");
    }
  };

  onMount(async () => {
    document.documentElement.classList.add("light-theme");
    shortcutHandler = registerShortcuts();

    // Register destroy listener FIRST to catch any destroy event fired
    // during the async awaits below (A2.3.G7 mount-race window).
    unlisteners.push(
      await onSessionDestroyed(async ({ id }) => {
        if (props.lockedSessionId && id === props.lockedSessionId) {
          // Our locked session was destroyed, close this detached window.
          // R.2 discipline: destroy() not close() so onCloseRequested is
          // not fired (avoids looping into attach_terminal on a dead session).
          if (isTauri) {
            const { getCurrentWindow } = await import("@tauri-apps/api/window");
            getCurrentWindow().destroy();
          }
          return;
        }
        if (!props.lockedSessionId) {
          await loadActiveSession();
        }
      })
    );

    // Detached-window X → re-attach to main, not destroy (plan §A2.2.G4 / G.13).
    // Register as early as possible in onMount so the race window from first
    // paint to handler-registered is minimized. If attach fails (session
    // destroyed mid-flight, backend command error, etc.), fall back to
    // destroying the window so the user isn't stuck.
    if (isTauri && props.detached && props.lockedSessionId) {
      const sessionId = props.lockedSessionId;
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlistenCloseRequested = await win.onCloseRequested(async (e) => {
        e.preventDefault();
        try {
          await WindowAPI.attach(sessionId);
        } catch (err) {
          console.error("[detached] attach failed during close; destroying window:", err);
          try { await win.destroy(); } catch { /* best-effort */ }
        }
      });
      unlisteners.push(unlistenCloseRequested);
    }

    // Window-level initializers — skipped when embedded (main owns these).
    if (!props.embedded) {
      // Detached windows use "detached" (mapped to terminalZoom in zoomKeyMap).
      cleanupZoom = await initZoom(props.detached ? "detached" : "terminal");
      if (props.detached && props.lockedSessionId) {
        // Per-session geometry persistence (plan §A2.4.Arb1).
        cleanupGeometry = await initDetachedWindowGeometry(props.lockedSessionId);
      } else {
        cleanupGeometry = await initWindowGeometry("terminal");
      }
    }
    settingsStore.load();
    await loadActiveSession();

    if (!props.lockedSessionId) {
      // Normal mode: respond to session switches
      unlisteners.push(
        await onSessionSwitched(async ({ id }) => {
          if (!id) {
            terminalStore.setActiveSession(null, "", "", null, "");
            return;
          }
          const sessions = await SessionAPI.list();
          const session = sessions.find((s) => s.id === id);
          if (session) {
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.effectiveShellArgs,
              session.workingDirectory
            );
          }
        })
      );

      unlisteners.push(
        await onSessionCreated((session) => {
          if (!terminalStore.activeSessionId) {
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.effectiveShellArgs,
              session.workingDirectory
            );
          }
        })
      );
    }

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        if (id === terminalStore.activeSessionId) {
          terminalStore.setActiveSession(id, name);
        }
      })
    );

    // Theme sync: follow sidebar theme toggle (redundant in embedded mode —
    // sidebar's toggle already flips the shared documentElement classList).
    if (!props.embedded) {
      unlisteners.push(
        await onThemeChanged(({ light }) => {
          if (light) {
            document.documentElement.classList.add("light-theme");
          } else {
            document.documentElement.classList.remove("light-theme");
          }
        })
      );
    }
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
  });

  return (
    <div class="terminal-layout">
      <Show when={!props.embedded}>
        <Titlebar detached={props.detached} lockedSessionId={props.lockedSessionId} />
      </Show>
      <LastPrompt sessionId={props.lockedSessionId} />
      <Show
        when={terminalStore.activeSessionId}
        fallback={
          <div class="terminal-empty">
            <span>
              {props.detached
                ? "Session closed"
                : "No active session"}
            </span>
            <Show when={!props.detached}>
              <button
                class="terminal-empty-btn"
                onClick={() => SessionAPI.create()}
              >
                + New Session
              </button>
            </Show>
          </div>
        }
      >
        <TerminalView lockedSessionId={props.lockedSessionId} />
      </Show>
      <StatusBar detached={props.detached} />
    </div>
  );
};

export default TerminalApp;
