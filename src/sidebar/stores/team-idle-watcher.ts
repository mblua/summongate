// Per-workgroup busy→all-idle transition detector for Feature #110.
//
// **Spec deviation:** issue #110 calls for per-*team* aggregation, but
// `sessionsStore.state.teams` is currently dead code — `setTeams()` is
// defined but never called from anywhere in the repo. We aggregate by
// workgroup instead, sourcing from `projectStore.projects[].workgroups[]`
// (the live data path that ProjectPanel/TeamFilter actually use).
// **If teams become live (someone wires up `setTeams`), revisit this
// aggregation** — the right unit may be the team-config rather than the
// workgroup instance.
//
// **Why per-session previous-busy tracking** (instead of a busy *set*
// diff): "session destroyed", "session exited", and "session renamed"
// all collapse to "session left the aggregation" under a set-diff
// model, which would fire spurious beeps on user kills, exit-0
// processes, and rename events. Tracking each session's busy flag from
// the previous tick lets us require a *genuine* busy→idle flip on a
// still-alive bound session before we consider beeping.
//
// **Why a stable sessionId→wgPath binding**: sessions match replicas
// by name `${wg.name}/${replica.name}` at creation, but can be renamed
// later. Resolving the binding on every effect run by name means a
// busy session whose name changes drops out of its workgroup's
// aggregation, which is exactly the bug above. We bind once on first
// observation and never unbind, so rename-while-busy is harmless.
//
// **Why createRoot**: the watcher is started from inside an async
// onMount in SidebarApp; by the time we reach `await
// settingsStore.load()` the synchronous reactive owner is gone.
// createRoot gives the effect its own owner and an explicit dispose
// we can call from onCleanup.

import { createEffect, createRoot } from "solid-js";
import type { Session } from "../../shared/types";
import { playTeamIdleBeep } from "../../shared/sound";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";

function isExited(status: Session["status"]): boolean {
  return typeof status === "object" && status !== null && "exited" in status;
}

function isBusy(session: Session): boolean {
  if (isExited(session.status)) return false;
  return !session.waitingForInput;
}

/**
 * Start the watcher. Returns a dispose function; call from SidebarApp's
 * onCleanup.
 *
 * Behavior on first effect run: snapshot only, no beep (the "no beep
 * at startup" rule). Subsequent runs fire `playTeamIdleBeep()` for any
 * workgroup that meets ALL of:
 *   - At least one session bound to the workgroup, alive last tick,
 *     was busy then and is alive + idle now (genuine transition).
 *   - All currently-bound, currently-alive sessions in the workgroup
 *     are idle.
 *   - The user setting `teamIdleBeepEnabled` is true.
 */
export function startTeamIdleWatcher(): () => void {
  return createRoot((dispose) => {
    // sessionId -> wg.path. Populated on first observation of each
    // session and never overwritten — protects against rename.
    const sessionToWg = new Map<string, string>();

    // wg.path -> Map<sessionId, wasBusy>. The inner map records each
    // bound, alive session's isBusy from the previous tick. Sessions
    // that were not alive last tick (destroyed/exited) won't appear.
    const previousByWg = new Map<string, Map<string, boolean>>();

    let initialized = false;

    createEffect(() => {
      const sessions = sessionsStore.sessions;
      const projects = projectStore.projects;
      const enabled = settingsStore.current?.teamIdleBeepEnabled ?? true;

      // 1. Augment bindings (never unbind). Iterate workgroups and
      //    bind any newly-discovered replica-session pairs by name.
      for (const project of projects) {
        for (const wg of project.workgroups) {
          for (const replica of wg.agents) {
            const session = sessionsStore.findSessionByName(
              `${wg.name}/${replica.name}`,
            );
            if (session && !sessionToWg.has(session.id)) {
              sessionToWg.set(session.id, wg.path);
            }
          }
        }
      }

      // 2. Build current per-wg busy state from the alive bound
      //    sessions only. Destroyed sessions (not in `sessions`) and
      //    exited sessions are excluded — they don't contribute to
      //    aggregation per spec.
      const sessionsById = new Map<string, Session>();
      for (const s of sessions) sessionsById.set(s.id, s);

      const currentByWg = new Map<string, Map<string, boolean>>();
      for (const [sessionId, wgPath] of sessionToWg) {
        const session = sessionsById.get(sessionId);
        if (!session) continue;
        if (isExited(session.status)) continue;
        let inner = currentByWg.get(wgPath);
        if (!inner) {
          inner = new Map<string, boolean>();
          currentByWg.set(wgPath, inner);
        }
        inner.set(sessionId, isBusy(session));
      }

      // 3. First run is snapshot-only — see header comment.
      if (!initialized) {
        initialized = true;
        for (const [wgPath, perSession] of currentByWg) {
          previousByWg.set(wgPath, new Map(perSession));
        }
        return;
      }

      // 4. Detect genuine busy→idle transitions per workgroup.
      if (enabled) {
        for (const [wgPath, currentBusy] of currentByWg) {
          const previousBusy = previousByWg.get(wgPath);
          if (!previousBusy) continue;

          // Genuine transition: a session that was busy last tick is
          // still alive (still in current map) and is now idle.
          let hadTransition = false;
          for (const [sessionId, wasBusy] of previousBusy) {
            if (!wasBusy) continue;
            const isBusyNow = currentBusy.get(sessionId);
            if (isBusyNow === false) {
              hadTransition = true;
              break;
            }
          }
          if (!hadTransition) continue;

          // All currently-alive bound sessions are idle?
          let allIdle = currentBusy.size > 0;
          if (allIdle) {
            for (const isBusyNow of currentBusy.values()) {
              if (isBusyNow) {
                allIdle = false;
                break;
              }
            }
          }
          if (!allIdle) continue;

          void playTeamIdleBeep();
        }
      }

      // 5. Persist current state for the next tick. Workgroups that
      //    no longer have any alive bound sessions drop out, so a
      //    later resurrection starts from a clean snapshot.
      previousByWg.clear();
      for (const [wgPath, perSession] of currentByWg) {
        previousByWg.set(wgPath, new Map(perSession));
      }
    });

    return dispose;
  });
}
