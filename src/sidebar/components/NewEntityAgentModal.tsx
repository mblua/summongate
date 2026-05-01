import { Component, createSignal, createMemo, Show } from "solid-js";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";

const NewEntityAgentModal: Component<{
  projectPath: string;
  onClose: () => void;
}> = (props) => {
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  let nameRef!: HTMLInputElement;

  const canCreate = createMemo(() => {
    const n = name().trim();
    return n.length > 0 && !n.includes("/") && !n.includes("\\") && !n.includes(" ");
  });

  const handleCreate = async () => {
    if (!canCreate() || creating()) return;
    setCreating(true);
    setError("");
    try {
      await EntityAPI.createAgentMatrix(props.projectPath, name().trim(), description().trim());
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
      // intentionally do NOT clear creating() — modal unmounts; any in-flight
      // keydown event in the close transition stays guarded.
    } catch (e: any) {
      console.error("create_agent_matrix failed:", e);
      setError(typeof e === "string" ? e : e.message || "Failed to create agent");
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      handleCreate();
    }
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) props.onClose();
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal new-agent-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Agent</span>
        </div>

        <div class="new-agent-form">
          <div class="new-agent-field">
            <label class="new-agent-label">Name</label>
            <input
              ref={nameRef!}
              class="agent-search-input"
              value={name()}
              onInput={(e) => { setName(e.currentTarget.value); setError(""); }}
              placeholder="my-agent"
              autofocus
            />
          </div>

          <div class="new-agent-field">
            <label class="new-agent-label">Description</label>
            <textarea
              class="entity-textarea"
              value={description()}
              onInput={(e) => setDescription(e.currentTarget.value)}
              placeholder="What does this agent do? (optional, max 250 chars)"
              maxLength={250}
              rows={3}
              aria-describedby="description-keyhint"
            />
            <div class="entity-textarea-meta">
              <span id="description-keyhint" class="entity-textarea-hint">Enter to create · Shift+Enter for newline</span>
              <Show when={description().length > 0}>
                <span class="entity-char-count">{description().length}/250</span>
              </Show>
            </div>
          </div>

          <Show when={error()}>
            <div class="new-agent-error">{error()}</div>
          </Show>
        </div>

        <div class="new-agent-footer">
          <button type="button" class="new-agent-cancel-btn" onClick={() => props.onClose()}>Cancel</button>
          <button
            class="new-agent-create-btn"
            disabled={!canCreate() || creating()}
            onClick={handleCreate}
          >
            {creating() ? "Creating..." : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default NewEntityAgentModal;
