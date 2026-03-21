import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Session, PtyOutputEvent, AppSettings, RepoMatch } from "./types";

export interface CreateSessionOptions {
  shell?: string;
  shellArgs?: string[];
  cwd?: string;
  sessionName?: string;
}

export const SessionAPI = {
  create: (opts?: CreateSessionOptions) =>
    invoke<Session>("create_session", {
      shell: opts?.shell ?? null,
      shellArgs: opts?.shellArgs ?? null,
      cwd: opts?.cwd ?? null,
      sessionName: opts?.sessionName ?? null,
    }),

  destroy: (id: string) => invoke<void>("destroy_session", { id }),

  switch: (id: string) => invoke<void>("switch_session", { id }),

  rename: (id: string, name: string) =>
    invoke<void>("rename_session", { id, name }),

  list: () => invoke<Session[]>("list_sessions"),

  getActive: () => invoke<string | null>("get_active_session"),
};

export const PtyAPI = {
  write: (sessionId: string, data: Uint8Array) =>
    invoke<void>("pty_write", { sessionId, data: Array.from(data) }),

  resize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>("pty_resize", { sessionId, cols, rows }),
};

export const SettingsAPI = {
  get: () => invoke<AppSettings>("get_settings"),
  update: (settings: AppSettings) =>
    invoke<void>("update_settings", { newSettings: settings }),
};

export const ReposAPI = {
  search: (query: string) =>
    invoke<RepoMatch[]>("search_repos", { query }),
};

export function onPtyOutput(
  callback: (data: PtyOutputEvent) => void
): Promise<UnlistenFn> {
  return listen<PtyOutputEvent>("pty_output", (event) => {
    callback(event.payload);
  });
}

export function onSessionCreated(
  callback: (session: Session) => void
): Promise<UnlistenFn> {
  return listen<Session>("session_created", (e) => callback(e.payload));
}

export function onSessionDestroyed(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_destroyed", (e) =>
    callback(e.payload)
  );
}

export function onSessionSwitched(
  callback: (data: { id: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string }>("session_switched", (e) =>
    callback(e.payload)
  );
}

export function onSessionRenamed(
  callback: (data: { id: string; name: string }) => void
): Promise<UnlistenFn> {
  return listen<{ id: string; name: string }>("session_renamed", (e) =>
    callback(e.payload)
  );
}
