import { createStore } from "solid-js/store";
import type { Session } from "../../shared/types";

interface SessionsState {
  sessions: Session[];
  activeId: string | null;
}

const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
});

export const sessionsStore = {
  get sessions() {
    return state.sessions;
  },
  get activeId() {
    return state.activeId;
  },

  setSessions(sessions: Session[]) {
    setState("sessions", sessions);
  },

  addSession(session: Session) {
    setState("sessions", (prev) =>
      prev.some((s) => s.id === session.id) ? prev : [...prev, session]
    );
  },

  removeSession(id: string) {
    setState("sessions", (prev) => prev.filter((s) => s.id !== id));
  },

  setActiveId(id: string | null) {
    setState("activeId", id);
    // Update statuses
    setState("sessions", (s) => s.id === id, "status", "active");
    setState(
      "sessions",
      (s) => s.id !== id && s.status === "active",
      "status",
      "running"
    );
  },

  renameSession(id: string, name: string) {
    setState("sessions", (s) => s.id === id, "name", name);
  },

  setSessionWaiting(id: string, waiting: boolean) {
    setState("sessions", (s) => s.id === id, "waitingForInput", waiting);
  },

  setGitBranch(sessionId: string, branch: string | null) {
    setState("sessions", (s) => s.id === sessionId, "gitBranch", branch);
  },
};
