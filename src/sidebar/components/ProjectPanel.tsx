import { Component, For, Show, createSignal, onMount, onCleanup } from "solid-js";
import type { AcWorkgroup, AcAgentReplica, Session } from "../../shared/types";
import { SessionAPI, WindowAPI, onDiscoveryBranchUpdated } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import SessionItem from "./SessionItem";

/** Derive the repo name from a replica's repoPaths (strip 'repo-' prefix) */
function replicaRepoName(replica: AcAgentReplica): string | undefined {
  if (!replica.repoPaths?.length) return undefined;
  const dirName = replica.repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
  return dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
}

/** Build the session name used to link a replica to its session */
function replicaSessionName(wg: AcWorkgroup, replica: AcAgentReplica): string {
  return `${wg.name}/${replica.name}`;
}

/** Find existing session for a replica, if any */
function replicaSession(wg: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  return sessionsStore.findSessionByName(replicaSessionName(wg, replica));
}

/** Compute CSS class for replica status dot */
function replicaDotClass(wg: AcWorkgroup, replica: AcAgentReplica): string {
  const session = replicaSession(wg, replica);
  if (!session) return "offline";
  if (session.pendingReview) return "pending";
  if (session.waitingForInput) return "waiting";
  if (typeof session.status === "string") return session.status;
  return "exited";
}

const ProjectPanel: Component = () => {
  // Listen for replica branch updates from the discovery branch watcher
  let unlistenBranch: (() => void) | null = null;
  onMount(async () => {
    unlistenBranch = await onDiscoveryBranchUpdated((data) => {
      projectStore.updateReplicaBranch(data.replicaPath, data.branch);
    });
  });
  onCleanup(() => unlistenBranch?.());

  const handleReplicaClick = async (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const existing = replicaSession(wg, replica);
    if (existing) {
      // Already instantiated — just switch to it
      await SessionAPI.switch(existing.id);
      if (isTauri) {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const detachedLabel = `terminal-${existing.id.replace(/-/g, "")}`;
        const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
        if (!detachedWin) {
          await WindowAPI.ensureTerminal();
        }
      }
      return;
    }

    // Not instantiated — create session in-place
    const repoPaths = replica.repoPaths ?? [];
    let gitBranchSource: string | undefined;
    let gitBranchPrefix: string | undefined;

    if (repoPaths.length === 1) {
      gitBranchSource = repoPaths[0];
      const dirName = repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
      gitBranchPrefix = dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
    } else if (repoPaths.length > 1) {
      gitBranchPrefix = "multi-repo";
    }

    SessionAPI.create({
      cwd: replica.path,
      sessionName: replicaSessionName(wg, replica),
      agentId: replica.preferredAgentId,
      gitBranchSource,
      gitBranchPrefix,
    });
  };

  const handleAgentClick = async (agent: { name: string; path: string; preferredAgentId?: string }) => {
    const existing = sessionsStore.findSessionByName(agent.name);
    if (existing) {
      await SessionAPI.switch(existing.id);
      if (isTauri) {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const detachedLabel = `terminal-${existing.id.replace(/-/g, "")}`;
        const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
        if (!detachedWin) {
          await WindowAPI.ensureTerminal();
        }
      }
      return;
    }
    SessionAPI.create({
      cwd: agent.path,
      sessionName: agent.name,
      agentId: agent.preferredAgentId,
    });
  };

  return (
    <For each={projectStore.projects}>
      {(proj) => {
        const [collapsed, setCollapsed] = createSignal(false);
        return (
          <div class="project-panel">
            <button
              class="project-header"
              onClick={() => setCollapsed((c) => !c)}
            >
              <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
                &#x25BE;
              </span>
              <span class="project-title">Project: {proj.folderName}</span>
            </button>
            <Show when={!collapsed()}>
              <div class="project-content">
                {/* Workgroups */}
                <For each={proj.workgroups}>
                  {(wg) => {
                    const [wgCollapsed, setWgCollapsed] = createSignal(false);
                    return (
                      <div class="ac-wg-group">
                        <div
                          class="ac-wg-header ac-wg-header--collapsible"
                          title={wg.path}
                          onClick={() => setWgCollapsed((c) => !c)}
                        >
                          <span class="ac-discovery-chevron" classList={{ collapsed: wgCollapsed() }}>
                            &#x25BE;
                          </span>
                          <div class="ac-wg-header-text">
                            <span class="ac-wg-name">{wg.name}</span>
                            <Show when={wg.brief}>
                              <span class="ac-wg-brief">{wg.brief}</span>
                            </Show>
                          </div>
                        </div>
                        <Show when={!wgCollapsed()}>
                          <For each={wg.agents}>
                            {(replica) => {
                              const session = () => replicaSession(wg, replica);
                              return (
                                <Show
                                  when={session()}
                                  fallback={
                                    (() => {
                                      const dotClass = () => replicaDotClass(wg, replica);
                                      const repoName = () => replicaRepoName(replica);
                                      const branchLabel = () => {
                                        const name = repoName();
                                        if (!name) return null;
                                        return replica.repoBranch ? `${name}/${replica.repoBranch}` : name;
                                      };
                                      return (
                                        <div
                                          class="ac-discovery-item"
                                          onClick={() => handleReplicaClick(replica, wg)}
                                          title={replica.path}
                                        >
                                          <div class={`session-item-status ${dotClass()}`} />
                                          <div class="ac-discovery-item-info">
                                            <span class="ac-discovery-item-name">{replica.name}</span>
                                            <div class="ac-discovery-badges">
                                              <Show when={branchLabel()}>
                                                <span class="ac-discovery-badge branch">{branchLabel()}</span>
                                              </Show>
                                            </div>
                                          </div>
                                        </div>
                                      );
                                    })()
                                  }
                                >
                                  {(s) => {
                                    const patched = () => {
                                      const sess = s();
                                      if (sess.gitBranch && !sess.gitBranch.includes("/")) {
                                        const repoName = replicaRepoName(replica);
                                        if (repoName) {
                                          return { ...sess, gitBranch: `${repoName}/${sess.gitBranch}` };
                                        }
                                      }
                                      return sess;
                                    };
                                    return (
                                      <SessionItem
                                        session={patched()}
                                        isActive={s().id === sessionsStore.activeId}
                                      />
                                    );
                                  }}
                                </Show>
                              );
                            }}
                          </For>
                        </Show>
                      </div>
                    );
                  }}
                </For>
                {/* Agent Matrix */}
                <Show when={proj.agents.length > 0}>
                  {(() => {
                    const [matrixCollapsed, setMatrixCollapsed] = createSignal(false);
                    return (
                      <div class="ac-wg-group">
                        <div
                          class="ac-wg-header ac-wg-header--collapsible"
                          onClick={() => setMatrixCollapsed((c) => !c)}
                        >
                          <span class="ac-discovery-chevron" classList={{ collapsed: matrixCollapsed() }}>
                            &#x25BE;
                          </span>
                          <div class="ac-wg-header-text">
                            <span class="ac-wg-name">Agent Matrix</span>
                          </div>
                        </div>
                        <Show when={!matrixCollapsed()}>
                          <For each={proj.agents}>
                            {(agent) => {
                              const session = () => sessionsStore.findSessionByName(agent.name);
                              return (
                                <Show
                                  when={session()}
                                  fallback={
                                    <div
                                      class="ac-discovery-item"
                                      onClick={() => handleAgentClick(agent)}
                                      title={agent.path}
                                    >
                                      <div class="session-item-status offline" />
                                      <div class="ac-discovery-item-info">
                                        <span class="ac-discovery-item-name">
                                          {agent.name.slice(agent.name.lastIndexOf("/") + 1)}
                                        </span>
                                      </div>
                                    </div>
                                  }
                                >
                                  {(s) => (
                                    <SessionItem
                                      session={s()}
                                      isActive={s().id === sessionsStore.activeId}
                                    />
                                  )}
                                </Show>
                              );
                            }}
                          </For>
                        </Show>
                      </div>
                    );
                  })()}
                </Show>
              </div>
            </Show>
          </div>
        );
      }}
    </For>
  );
};

export default ProjectPanel;
