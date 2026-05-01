// Per-workgroup busy→all-idle transition detector for Feature #110.
// Why createRoot: the watcher is started from inside an async onMount in
// SidebarApp; by the time we reach `await settingsStore.load()` the
// synchronous reactive owner is gone. createRoot gives the effect its
// own owner and an explicit dispose we can call from onCleanup.

import { createEffect, createRoot } from "solid-js";
import type { Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";

function isBusy(session: Session): boolean {
  const status = session.status;
  if (typeof status === "object" && status !== null && "exited" in status) {
    return false;
  }
  return !session.waitingForInput;
}

// "Workgroup" is the granular unit users see in the sidebar — each
// workgroup is a concrete instance of a team config (e.g. wg-6-dev-team)
// with its own replica set. Sessions match replicas by name
// `${wg.name}/${replica.name}` (the same scheme ProjectPanel uses).
// We key the busy map by `wg.path` (filesystem path), which is unique
// across all projects and stable across re-discovery.
function computeBusyByWorkgroup(): Map<string, Set<string>> {
  const result = new Map<string, Set<string>>();
  for (const project of projectStore.projects) {
    for (const wg of project.workgroups) {
      const busy = new Set<string>();
      for (const replica of wg.agents) {
        const session = sessionsStore.findSessionByName(
          `${wg.name}/${replica.name}`,
        );
        if (session && isBusy(session)) {
          busy.add(session.id);
        }
      }
      result.set(wg.path, busy);
    }
  }
  return result;
}

/**
 * Start the watcher. Returns a dispose function; call from SidebarApp's
 * onCleanup. The first effect run snapshots state without firing — this
 * is the "no beep at startup" rule from #110. Subsequent runs compare
 * the busy set per workgroup against the previous snapshot and fire the
 * beep on any non-empty → empty transition (when the setting is enabled).
 */
export function startTeamIdleWatcher(): () => void {
  return createRoot((dispose) => {
    const previousBusyByWg = new Map<string, Set<string>>();
    let initialized = false;

    createEffect(() => {
      // Touch reactive sources so the effect re-runs on each change.
      // computeBusyByWorkgroup reads them too, but reading here makes
      // the dependency explicit for future maintainers.
      void sessionsStore.sessions;
      void projectStore.projects;
      const enabled = settingsStore.current?.teamIdleBeepEnabled ?? true;

      const currentBusyByWg = computeBusyByWorkgroup();

      if (!initialized) {
        initialized = true;
        for (const [key, busy] of currentBusyByWg) {
          previousBusyByWg.set(key, busy);
        }
        return;
      }

      if (enabled) {
        for (const [key, currentBusy] of currentBusyByWg) {
          const previousBusy = previousBusyByWg.get(key);
          if (previousBusy && previousBusy.size > 0 && currentBusy.size === 0) {
            void playTeamIdleBeep();
          }
        }
      }

      previousBusyByWg.clear();
      for (const [key, busy] of currentBusyByWg) {
        previousBusyByWg.set(key, busy);
      }
    });

    return dispose;
  });
}
