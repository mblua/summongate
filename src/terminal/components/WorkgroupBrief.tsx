import { Component, createMemo, createSignal, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { terminalStore } from "../stores/terminal";
import { BriefAPI } from "../../shared/ipc";
import BriefCleanConfirmModal from "./BriefCleanConfirmModal";

function parseBriefTitle(content: string | null): string | null {
  if (!content) return null;
  if (!content.startsWith("---")) return null;
  const closer = content.indexOf("\n---", 3);
  if (closer < 0) return null;
  const fm = content.slice(3, closer);
  for (const line of fm.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.toLowerCase().startsWith("title:")) continue;
    const after = trimmed.slice(6).trim();
    if (after.startsWith("'") && after.endsWith("'") && after.length >= 2) {
      return after.slice(1, -1).replace(/''/g, "'");
    }
    if (after.startsWith('"') && after.endsWith('"') && after.length >= 2) {
      return after.slice(1, -1);
    }
    return after;
  }
  return null;
}

// Backend uses byte-exact `name.starts_with("wg-")` (session/session.rs).
// Keep this regex case-sensitive so the UX gate matches; matching `WG-19`
// here would render the buttons enabled but every click would fail.
function hasWorkgroupContext(cwd: string): boolean {
  return /[\/\\]wg-/.test(cwd);
}

const WorkgroupBrief: Component = () => {
  const [editing, setEditing] = createSignal(false);
  const [titleDraft, setTitleDraft] = createSignal("");
  const [confirmingClean, setConfirmingClean] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [capturedSessionId, setCapturedSessionId] = createSignal<string | null>(null);

  const currentBrief = createMemo(() => terminalStore.activeWorkgroupBrief?.trim() ?? "");
  const sessionId = createMemo(() => terminalStore.activeSessionId);
  const cwd = createMemo(() => terminalStore.activeWorkingDirectory);
  const baseDisabled = createMemo(
    () => !sessionId() || !hasWorkgroupContext(cwd()) || busy()
  );
  const editDisabled = createMemo(() => baseDisabled() || confirmingClean());
  const cleanDisabled = createMemo(() => baseDisabled() || editing());

  const startEditing = async () => {
    if (editDisabled()) return;
    setError(null);
    const id = sessionId();
    if (!id) {
      setError("Session no longer available.");
      return;
    }
    // Lock out Clean while we await getTitle; otherwise the clean modal
    // could open in parallel with the editor (NB-1 race).
    setCapturedSessionId(id);
    setBusy(true);
    let prefill = parseBriefTitle(terminalStore.activeWorkgroupBrief) ?? "";
    try {
      const fromBackend = await BriefAPI.getTitle(id);
      if (fromBackend !== null && fromBackend !== undefined) {
        prefill = fromBackend;
      }
    } catch (err) {
      setError(String(err));
      setCapturedSessionId(null);
      setBusy(false);
      return;
    }
    if (sessionId() !== id) {
      setCapturedSessionId(null);
      setBusy(false);
      setError("Session changed; please retry.");
      return;
    }
    setTitleDraft(prefill);
    setEditing(true);
    setBusy(false);
  };

  const cancelEditing = () => {
    setEditing(false);
    setTitleDraft("");
    setCapturedSessionId(null);
    setError(null);
  };

  const saveTitle = async () => {
    const id = sessionId();
    if (!id) {
      setError("Session no longer available.");
      return;
    }
    if (capturedSessionId() !== id) {
      setError("Session changed; cancel and retry.");
      return;
    }
    const title = titleDraft().trim();
    if (!title) {
      setError("Title cannot be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await BriefAPI.setTitle(id, title);
      terminalStore.setActiveWorkgroupBrief(result.brief);
      setEditing(false);
      setTitleDraft("");
      setCapturedSessionId(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      saveTitle();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEditing();
    }
  };

  const requestClean = () => {
    if (cleanDisabled()) return;
    const id = sessionId();
    if (!id) return;
    setError(null);
    setCapturedSessionId(id);
    setConfirmingClean(true);
  };

  const performClean = async () => {
    const id = sessionId();
    setConfirmingClean(false);
    if (!id) {
      setCapturedSessionId(null);
      setError("Session no longer available.");
      return;
    }
    if (capturedSessionId() !== id) {
      setCapturedSessionId(null);
      setError("Session changed; cancel and retry.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await BriefAPI.clean(id);
      terminalStore.setActiveWorkgroupBrief(result.brief);
      setEditing(false);
      setTitleDraft("");
    } catch (err) {
      setError(String(err));
    } finally {
      setCapturedSessionId(null);
      setBusy(false);
    }
  };

  const onInputRef = (el: HTMLInputElement) => {
    requestAnimationFrame(() => {
      el.focus();
      el.select();
    });
  };

  return (
    <div class="workgroup-brief-panel">
      <div class="workgroup-brief-header">
        <div class="workgroup-brief-label">BRIEF</div>
        <div class="workgroup-brief-actions">
          <button
            class="workgroup-brief-action"
            onClick={startEditing}
            disabled={editDisabled()}
            title="Edit BRIEF title"
            type="button"
          >
            &#x270E;
          </button>
          <button
            class="workgroup-brief-action"
            onClick={requestClean}
            disabled={cleanDisabled()}
            title="Clean BRIEF (reset to Limpio)"
            type="button"
          >
            &#x1F9F9;
          </button>
        </div>
      </div>
      <Show when={editing()}>
        <div class="workgroup-brief-title-edit">
          <input
            ref={onInputRef}
            class="workgroup-brief-title-input"
            value={titleDraft()}
            onInput={(e) => setTitleDraft(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
            placeholder="Title"
            disabled={busy()}
          />
          <button
            class="workgroup-brief-title-btn save"
            onClick={saveTitle}
            disabled={busy() || !titleDraft().trim()}
            type="button"
          >
            Save
          </button>
          <button
            class="workgroup-brief-title-btn cancel"
            onClick={cancelEditing}
            disabled={busy()}
            type="button"
          >
            Cancel
          </button>
        </div>
      </Show>
      <Show when={error()}>
        <div class="workgroup-brief-error">{error()}</div>
      </Show>
      <div class="workgroup-brief-text">{currentBrief() || "..."}</div>
      <Show when={confirmingClean()}>
        <Portal>
          <BriefCleanConfirmModal
            onCancel={() => {
              setConfirmingClean(false);
              setCapturedSessionId(null);
            }}
            onConfirm={performClean}
          />
        </Portal>
      </Show>
    </div>
  );
};

export default WorkgroupBrief;
