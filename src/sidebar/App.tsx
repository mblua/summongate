import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  ReposAPI,
  WindowAPI,
  onSessionCreated,
  onSessionDestroyed,
  onSessionSwitched,
  onSessionRenamed,
  onSessionIdle,
  onSessionBusy,
  onSessionGitRepos,
  onSessionCoordinatorChanged,
  onTelegramBridgeAttached,
  onTelegramBridgeDetached,
  onTelegramBridgeError,
  onTerminalDetached,
  onTerminalAttached,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { sessionsStore } from "./stores/sessions";
import { bridgesStore } from "./stores/bridges";
import { projectStore } from "./stores/project";
import { settingsStore } from "../shared/stores/settings";
import Titlebar from "./components/Titlebar";
import ActionBar from "./components/ActionBar";
import RootAgentBanner from "./components/RootAgentBanner";
import ProjectPanel from "./components/ProjectPanel";
import OnboardingModal from "./components/OnboardingModal";
import "./styles/sidebar.css";

interface SidebarAppProps {
  /**
   * True when mounted inside MainApp's unified layout. Skips window-level
   * initializers (titlebar render, zoom, geometry, always-on-top, raise-
   * terminal click-handler) — those are main-window concerns.
   */
  embedded?: boolean;
}

const SidebarApp: Component<SidebarAppProps> = (props) => {
  const [showOnboarding, setShowOnboarding] = createSignal(false);
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  const blockContextMenu = (e: Event) => e.preventDefault();

  onMount(async () => {
    document.documentElement.classList.add("light-theme");
    shortcutHandler = registerShortcuts();

    // Window-level initializers — skipped when embedded (main owns these).
    if (!props.embedded) {
      cleanupZoom = await initZoom("sidebar");
      cleanupGeometry = await initWindowGeometry("sidebar");
    }

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    // Apply sidebar style from settings (remap removed themes to default)
    const style = appSettings.sidebarStyle;
    const removedThemes = ["classic", "signal-grid"];
    document.documentElement.dataset.sidebarStyle = (!style || removedThemes.includes(style)) ? "noir-minimal" : style;
    if (!props.embedded && appSettings.sidebarAlwaysOnTop && isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(true);
    }

    // Block the default browser context menu globally — custom menus are used instead
    document.addEventListener("contextmenu", blockContextMenu);

    // Load settings into reactive store (for voice-to-text visibility etc.)
    await settingsStore.load();

    // First-run: show onboarding if no coding agents configured and not previously dismissed
    if (
      (!appSettings.agents || appSettings.agents.length === 0) &&
      !appSettings.onboardingDismissed
    ) {
      setShowOnboarding(true);
    }

    // Load saved project if any
    await projectStore.initFromSettings(
      appSettings.projectPaths ?? [],
      appSettings.projectPath ?? null,
    );

    // Load all repos for inactive agent display
    try {
      const allRepos = await ReposAPI.search("");
      sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
    } catch {}

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
        // Detached-window cleanup: if the session had a detached window,
        // its destroy also closes that window. Clear the store flag so
        // UI (icons, menu items) doesn't linger in detached state.
        sessionsStore.setDetached(id, false);
      })
    );

    unlisteners.push(
      await onTerminalDetached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, true)
      )
    );

    unlisteners.push(
      await onTerminalAttached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, false)
      )
    );

    // Hydrate detachedIds from backend (G.8 race safety — covers detach
    // events that fired before this component mounted, e.g. from the
    // Phase-3 restore path or from a prior detach survived across a
    // SidebarApp re-mount in the unified window).
    try {
      const ids = await WindowAPI.listDetached();
      ids.forEach((id) => sessionsStore.setDetached(id, true));
    } catch (e) {
      console.warn("[sidebar] listDetached hydration failed:", e);
    }

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

    unlisteners.push(
      await onSessionGitRepos(({ sessionId, repos }) => {
        sessionsStore.setGitRepos(sessionId, repos);
      })
    );

    unlisteners.push(
      await onSessionCoordinatorChanged(({ sessionId, isCoordinator }) => {
        sessionsStore.setIsCoordinator(sessionId, isCoordinator);
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
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
    document.removeEventListener("contextmenu", blockContextMenu);
  });

  return (
    <>
      <div class="sidebar-layout">
        <Show when={!props.embedded}>
          <Titlebar />
        </Show>
        <ActionBar />
        <RootAgentBanner />
        <div class="sidebar-scrollable">
          <ProjectPanel />
        </div>
      </div>
      <Show when={showOnboarding()}>
        <OnboardingModal onClose={() => setShowOnboarding(false)} />
      </Show>
    </>
  );
};

export default SidebarApp;
