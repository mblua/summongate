import { Component, onMount, onCleanup } from "solid-js";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, Window } from "@tauri-apps/api/window";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  onSessionCreated,
  onSessionDestroyed,
  onSessionSwitched,
  onSessionRenamed,
  onSessionIdle,
  onSessionBusy,
  onTelegramBridgeAttached,
  onTelegramBridgeDetached,
  onTelegramBridgeError,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { sessionsStore } from "./stores/sessions";
import { bridgesStore } from "./stores/bridges";
import Titlebar from "./components/Titlebar";
import SessionList from "./components/SessionList";
import Toolbar from "./components/Toolbar";
import "./styles/sidebar.css";

const SidebarApp: Component = () => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let raiseTerminalEnabled = true;
  let lastRaiseTime = 0;

  const handleRaiseTerminal = async () => {
    if (!raiseTerminalEnabled) return;
    const now = Date.now();
    if (now - lastRaiseTime < 500) return;
    lastRaiseTime = now;
    try {
      const terminal = await Window.getByLabel("terminal");
      if (terminal) {
        await terminal.setFocus();
        await getCurrentWindow().setFocus();
      }
    } catch {}
  };

  onMount(async () => {
    shortcutHandler = registerShortcuts();

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    if (appSettings.sidebarAlwaysOnTop) {
      await getCurrentWindow().setAlwaysOnTop(true);
    }
    document.addEventListener("mousedown", handleRaiseTerminal);

    // Load initial sessions
    const sessions = await SessionAPI.list();
    sessionsStore.setSessions(sessions);

    const activeId = await SessionAPI.getActive();
    sessionsStore.setActiveId(activeId);

    // Listen for events
    unlisteners.push(
      await onSessionCreated((session) => {
        sessionsStore.addSession(session);
        // New session is auto-activated if it's the first one
        if (sessionsStore.sessions.length === 1) {
          sessionsStore.setActiveId(session.id);
        }
      })
    );

    unlisteners.push(
      await onSessionDestroyed(({ id }) => {
        sessionsStore.removeSession(id);
      })
    );

    unlisteners.push(
      await onSessionSwitched(({ id }) => {
        sessionsStore.setActiveId(id);
      })
    );

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        sessionsStore.renameSession(id, name);
      })
    );

    unlisteners.push(
      await onSessionIdle(({ id }) => {
        sessionsStore.setSessionWaiting(id, true);
      })
    );

    unlisteners.push(
      await onSessionBusy(({ id }) => {
        sessionsStore.setSessionWaiting(id, false);
      })
    );

    // Load initial bridge state
    const bridges = await TelegramAPI.listBridges();
    bridgesStore.setBridges(bridges);

    // Telegram bridge events
    unlisteners.push(
      await onTelegramBridgeAttached((info) => {
        bridgesStore.addBridge(info);
      })
    );

    unlisteners.push(
      await onTelegramBridgeDetached(({ sessionId }) => {
        bridgesStore.removeBridge(sessionId);
      })
    );

    unlisteners.push(
      await onTelegramBridgeError(({ sessionId, error }) => {
        console.error(`Bridge error for ${sessionId}: ${error}`);
      })
    );
  });

  onCleanup(() => {
    unlisteners.forEach((unlisten) => unlisten());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    document.removeEventListener("mousedown", handleRaiseTerminal);
  });

  return (
    <div class="sidebar-layout">
      <Titlebar />
      <SessionList />
      <Toolbar />
    </div>
  );
};

export default SidebarApp;
