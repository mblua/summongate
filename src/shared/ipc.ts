import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Session, PtyOutputEvent } from "./types";

export const SessionAPI = {
  create: (profileName?: string) =>
    invoke<Session>("create_session", { profileName: profileName ?? null }),

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
