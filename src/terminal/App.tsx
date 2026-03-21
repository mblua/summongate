import { Component, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  SessionAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { terminalStore } from "./stores/terminal";
import Titlebar from "./components/Titlebar";
import TerminalView from "./components/TerminalView";
import StatusBar from "./components/StatusBar";
import "./styles/terminal.css";

const TerminalApp: Component = () => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;

  const loadActiveSession = async () => {
    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell);
      }
    } else {
      terminalStore.setActiveSession(null, "", "");
    }
  };

  onMount(async () => {
    shortcutHandler = registerShortcuts();
    await loadActiveSession();

    unlisteners.push(
      await onSessionSwitched(async ({ id }) => {
        const sessions = await SessionAPI.list();
        const session = sessions.find((s) => s.id === id);
        if (session) {
          terminalStore.setActiveSession(session.id, session.name, session.shell);
        }
      })
    );

    unlisteners.push(
      await onSessionCreated((session) => {
        // If no active session, activate this one
        if (!terminalStore.activeSessionId) {
          terminalStore.setActiveSession(
            session.id,
            session.name,
            session.shell
          );
        }
      })
    );

    unlisteners.push(
      await onSessionDestroyed(async () => {
        await loadActiveSession();
      })
    );

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        if (id === terminalStore.activeSessionId) {
          terminalStore.setActiveSession(id, name);
        }
      })
    );
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
  });

  return (
    <div class="terminal-layout">
      <Titlebar />
      <Show
        when={terminalStore.activeSessionId}
        fallback={
          <div class="terminal-empty">
            <span>No active session</span>
            <button
              class="terminal-empty-btn"
              onClick={() => SessionAPI.create()}
            >
              + New Session
            </button>
          </div>
        }
      >
        <TerminalView />
      </Show>
      <StatusBar />
    </div>
  );
};

export default TerminalApp;
