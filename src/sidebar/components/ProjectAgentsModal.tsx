import { Component, createSignal, For, Show, onMount } from "solid-js";
import { createStore } from "solid-js/store";
import type { AgentConfig, ProjectSettings } from "../../shared/types";
import { ProjectSettingsAPI, SettingsAPI } from "../../shared/ipc";
import { AGENT_PRESET_MAP, newAgentId } from "../../shared/agent-presets";

const ProjectAgentsModal: Component<{
  projectPath: string;
  projectName: string;
  initialSettings: ProjectSettings | null;
  onClose: () => void;
  onSaved: () => void;
}> = (props) => {
  const [localAgents, setLocalAgents] = createStore<{ list: AgentConfig[] }>({ list: [] });
  const [customEnabled, setCustomEnabled] = createSignal(props.initialSettings !== null);
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);

  onMount(() => {
    if (props.initialSettings) {
      setLocalAgents("list", props.initialSettings.agents.map((a) => ({ ...a })));
    }
  });

  // ── Agent mutations ──

  const updateAgent = (index: number, field: keyof AgentConfig, value: string | boolean) => {
    setLocalAgents("list", index, field as any, value as any);
  };

  const addAgent = (preset?: Omit<AgentConfig, "id">) => {
    const agent: AgentConfig = preset
      ? { id: newAgentId(), ...preset }
      : {
          id: newAgentId(),
          label: "",
          command: "",
          color: "#6366f1",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
        };
    setLocalAgents("list", (prev) => [...prev, agent]);
  };

  const removeAgent = (index: number) => {
    setLocalAgents("list", (prev) => prev.filter((_, i) => i !== index));
  };

  const hasAgentByCommand = (command: string): boolean =>
    localAgents.list.some((a) => a.command.startsWith(command));

  // ── Copy from Global ──

  const handleCopyFromGlobal = async () => {
    const settings = await SettingsAPI.get();
    if (settings.agents.length === 0) return;
    const copied = settings.agents.map((a) => ({ ...a, id: newAgentId() }));
    setLocalAgents("list", copied);
  };

  // ── Validation ──

  const validateAgents = (): string | null => {
    for (const agent of localAgents.list) {
      const cmd = agent.command.toLowerCase();
      if (cmd.includes("claude")) {
        const flags = cmd.split(/\s+/);
        if (flags.includes("--continue") || flags.includes("-c")) {
          return `Agent "${agent.label || "Unnamed"}": Claude commands must not include --continue or -c`;
        }
      }
    }
    return null;
  };

  // ── Save ──

  const handleSave = async () => {
    if (customEnabled()) {
      const err = validateAgents();
      if (err) { setSaveError(err); return; }
      setSaving(true);
      try {
        await ProjectSettingsAPI.update(props.projectPath, { agents: [...localAgents.list] });
        props.onSaved();
      } catch (e) { setSaveError(String(e)); }
      finally { setSaving(false); }
    } else {
      setSaving(true);
      try {
        await ProjectSettingsAPI.delete(props.projectPath);
        props.onSaved();
      } catch (e) { setSaveError(String(e)); }
      finally { setSaving(false); }
    }
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) props.onClose();
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick}>
      <div class="modal-container">
        {/* Header */}
        <div class="modal-header">
          <span class="modal-title">Coding Agents &mdash; {props.projectName}</span>
          <button class="modal-close" onClick={() => props.onClose()}>&#x2715;</button>
        </div>

        {/* Body */}
        <div class="modal-body">
          {/* Toggle */}
          <div class="project-agents-toggle">
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                class="settings-checkbox"
                checked={customEnabled()}
                onChange={(e) => setCustomEnabled(e.currentTarget.checked)}
              />
              <span>Use custom agents for this project</span>
            </label>
          </div>

          <Show
            when={customEnabled()}
            fallback={
              <div class="project-agents-global-info">
                Using global agents
              </div>
            }
          >
            <div class="settings-section">
              {/* Copy from Global */}
              <div class="settings-agent-actions" style={{ "margin-bottom": "8px" }}>
                <button class="settings-preset-btn" onClick={handleCopyFromGlobal}>
                  Copy from Global
                </button>
              </div>

              {/* Agent cards */}
              <For each={localAgents.list}>
                {(agent, i) => (
                  <div class="settings-button-card">
                    <div class="settings-button-card-header">
                      <div class="settings-color-dot" style={{ background: agent.color }} />
                      <span>{agent.label || "New Agent"}</span>
                      <button
                        class="settings-agent-remove"
                        onClick={() => removeAgent(i())}
                        title="Remove agent"
                      >
                        &#x2715;
                      </button>
                    </div>
                    <label class="settings-field">
                      <span class="settings-label">Label</span>
                      <input
                        class="settings-input"
                        value={agent.label}
                        onInput={(e) => updateAgent(i(), "label", e.currentTarget.value)}
                        placeholder="My Agent"
                      />
                    </label>
                    <label class="settings-field">
                      <span class="settings-label">Command</span>
                      <input
                        class="settings-input"
                        value={agent.command}
                        onInput={(e) => updateAgent(i(), "command", e.currentTarget.value)}
                        placeholder="agent-cli"
                      />
                    </label>
                    <label class="settings-field">
                      <span class="settings-label">Color</span>
                      <div class="settings-color-row">
                        <input
                          type="color"
                          class="settings-color-picker"
                          value={agent.color}
                          onInput={(e) => updateAgent(i(), "color", e.currentTarget.value)}
                        />
                        <input
                          class="settings-input settings-input-sm"
                          value={agent.color}
                          onInput={(e) => updateAgent(i(), "color", e.currentTarget.value)}
                        />
                      </div>
                    </label>
                    <label class="settings-checkbox-field">
                      <input
                        type="checkbox"
                        class="settings-checkbox"
                        checked={agent.gitPullBefore}
                        onChange={(e) => updateAgent(i(), "gitPullBefore", e.currentTarget.checked)}
                      />
                      <span>Run git pull before launch</span>
                    </label>
                    <label class="settings-checkbox-field">
                      <input
                        type="checkbox"
                        class="settings-checkbox"
                        checked={agent.excludeGlobalClaudeMd}
                        onChange={(e) => updateAgent(i(), "excludeGlobalClaudeMd", e.currentTarget.checked)}
                      />
                      <span>Exclude global CLAUDE.md on agent creation</span>
                    </label>
                  </div>
                )}
              </For>

              {/* Preset & custom buttons */}
              <div class="settings-agent-actions">
                <Show when={!hasAgentByCommand("claude")}>
                  <button class="settings-preset-btn" onClick={() => addAgent(AGENT_PRESET_MAP.claude)}>
                    <span class="settings-color-dot" style={{ background: AGENT_PRESET_MAP.claude.color }} />
                    + Claude Code
                  </button>
                </Show>
                <Show when={!hasAgentByCommand("codex")}>
                  <button class="settings-preset-btn" onClick={() => addAgent(AGENT_PRESET_MAP.codex)}>
                    <span class="settings-color-dot" style={{ background: AGENT_PRESET_MAP.codex.color }} />
                    + Codex
                  </button>
                </Show>
                <Show when={!hasAgentByCommand("gemini")}>
                  <button class="settings-preset-btn" onClick={() => addAgent(AGENT_PRESET_MAP.gemini)}>
                    <span class="settings-color-dot" style={{ background: AGENT_PRESET_MAP.gemini.color }} />
                    + Gemini CLI
                  </button>
                </Show>
                <button class="settings-add-btn" onClick={() => addAgent()}>
                  + Custom Agent
                </button>
              </div>
            </div>
          </Show>
        </div>

        {/* Footer */}
        <div class="modal-footer">
          <Show when={saveError()}>
            <span class="modal-save-error">{saveError()}</span>
          </Show>
          <button class="modal-btn modal-btn-cancel" onClick={() => props.onClose()}>Cancel</button>
          <button class="modal-btn modal-btn-save" disabled={saving()} onClick={handleSave}>
            {saving() ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default ProjectAgentsModal;
