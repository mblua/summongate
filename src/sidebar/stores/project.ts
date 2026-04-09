import { createSignal } from "solid-js";
import type { AcWorkgroup, AcAgentMatrix, AcTeam, ProjectSettings, AgentConfig } from "../../shared/types";
import { ProjectAPI, ProjectSettingsAPI, SettingsAPI, AgentCreatorAPI } from "../../shared/ipc";

export interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  projectSettings: ProjectSettings | null;
}

const [projects, setProjects] = createSignal<ProjectState[]>([]);
const [loading, setLoading] = createSignal(false);
let loadingCount = 0;

function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase();
}

export const projectStore = {
  /** All loaded projects */
  get projects() {
    return projects();
  },

  /** Legacy single-project accessor (first project or null) */
  get current() {
    return projects()[0] ?? null;
  },

  get isLoading() {
    return loading();
  },

  /** Load a project from a path: discover and append to the list (skip if already loaded) */
  async loadProject(path: string) {
    const normalized = normalizePath(path);
    if (projects().some((p) => normalizePath(p.path) === normalized)) return;

    loadingCount++;
    setLoading(true);
    try {
      const [result, projectSettings] = await Promise.all([
        ProjectAPI.discover(path),
        ProjectSettingsAPI.get(path).catch(() => null),
      ]);
      const folderName = path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
      setProjects((prev) => [
        ...prev,
        {
          path,
          folderName,
          workgroups: result.workgroups,
          agents: result.agents,
          teams: result.teams,
          projectSettings,
        },
      ]);
      await persistProjectPaths();
    } catch (e) {
      console.error("Failed to load project:", e);
    } finally {
      loadingCount--;
      if (loadingCount === 0) setLoading(false);
    }
  },

  /** Initialize from saved settings (call on mount) */
  async initFromSettings(projectPaths: string[], legacyPath: string | null) {
    // Merge legacy single path into the array (deduplicated)
    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }
    for (const path of paths) {
      await projectStore.loadProject(path);
    }
  },

  /** Create .ac-new in path and add as project */
  async createAndLoad(path: string) {
    await ProjectAPI.createAcProject(path);
    await projectStore.loadProject(path);
  },

  /** Full open flow: pick folder, check .ac-new, auto-load if found */
  async pickAndCheck(): Promise<{ picked: string | null; hasAcNew: boolean }> {
    const picked = await AgentCreatorAPI.pickFolder();
    if (!picked) return { picked: null, hasAcNew: false };

    const hasAcNew = await ProjectAPI.checkPath(picked);
    if (hasAcNew) {
      await projectStore.loadProject(picked);
    }
    return { picked, hasAcNew };
  },

  /** Update a replica's branch from the discovery branch watcher */
  updateReplicaBranch(replicaPath: string, branch: string | null) {
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) => ({
          ...wg,
          agents: wg.agents.map((a) =>
            a.path === replicaPath
              ? { ...a, repoBranch: branch ?? undefined }
              : a
          ),
        })),
      }))
    );
  },

  /** Re-discover a single project and update its data in place */
  async reloadProject(path: string) {
    const normalized = normalizePath(path);
    try {
      const [result, projectSettings] = await Promise.all([
        ProjectAPI.discover(path),
        ProjectSettingsAPI.get(path).catch(() => null),
      ]);
      setProjects((prev) =>
        prev.map((p) =>
          normalizePath(p.path) === normalized
            ? { ...p, workgroups: result.workgroups, agents: result.agents, teams: result.teams, projectSettings }
            : p
        )
      );
    } catch (e) {
      console.error("Failed to reload project:", e);
    }
  },

  /** Remove a project from the list by path */
  async removeProject(path: string) {
    const normalized = normalizePath(path);
    setProjects((prev) => prev.filter((p) => normalizePath(p.path) !== normalized));
    await persistProjectPaths();
  },

  /** Check if a project has custom agents configured */
  hasCustomAgents(path: string): boolean {
    const norm = normalizePath(path);
    return projects().some((p) => normalizePath(p.path) === norm && p.projectSettings != null);
  },

  /** Get resolved agents for a project: project-level overrides global (null = use global) */
  getResolvedAgents(path: string): AgentConfig[] | null {
    const norm = normalizePath(path);
    const proj = projects().find((p) => normalizePath(p.path) === norm);
    return proj?.projectSettings?.agents ?? null;
  },

  clear() {
    setProjects([]);
  },
};

/** Persist current project paths to settings */
async function persistProjectPaths() {
  const fresh = await SettingsAPI.get();
  const paths = projects().map((p) => p.path);
  await SettingsAPI.update({
    ...fresh,
    projectPaths: paths,
    projectPath: paths[0] ?? null, // backward compat
  });
}
