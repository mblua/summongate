import { Component, Show, createSignal, onMount, onCleanup } from "solid-js";
import iconUrl from "../../assets/icon-16.png";
import { applyWindowLayout } from "../../shared/window-layout";
import { isTauri } from "../../shared/platform";

declare const __APP_VERSION__: string;
const APP_VERSION = __APP_VERSION__;

const Titlebar: Component = () => {
  const [layoutOpen, setLayoutOpen] = createSignal(false);
  const [instanceLabel, setInstanceLabel] = createSignal("");

  const handleMinimize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().minimize();
  };
  const handleClose = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    getCurrentWindow().close();
  };

  const handleLayout = async (side: "left" | "right") => {
    setLayoutOpen(false);
    try {
      await applyWindowLayout(side);
    } catch (err) {
      console.error("applyLayout failed:", err);
    }
  };

  const handleClickOutside = (e: MouseEvent) => {
    if (layoutOpen() && !(e.target as HTMLElement).closest(".layout-dropdown-wrapper")) {
      setLayoutOpen(false);
    }
  };

  onMount(async () => {
    document.addEventListener("click", handleClickOutside);
    onCleanup(() => document.removeEventListener("click", handleClickOutside));
    if (isTauri) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const label = await invoke<string>("get_instance_label");
        if (label) setInstanceLabel(label);
      } catch { /* non-Tauri or command unavailable */ }
    }
  });

  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="titlebar-brand" data-tauri-drag-region>
        <img src={iconUrl} class="titlebar-icon" alt="" draggable={false} />
        <span class="titlebar-title" data-tauri-drag-region>
          agents commander
        </span>
        <span class="titlebar-version" data-tauri-drag-region>
          v{APP_VERSION}
        </span>
        {import.meta.env.DEV && (
          <span class="titlebar-dev-badge" data-tauri-drag-region>DEV</span>
        )}
        {instanceLabel() && (
          <span class="titlebar-stage-badge" data-tauri-drag-region>{instanceLabel()}</span>
        )}
      </div>
      <div class="titlebar-controls">
        <div class="layout-dropdown-wrapper">
          <button
            class={`titlebar-btn titlebar-btn-layout ${layoutOpen() ? "open" : ""}`}
            onClick={(e) => { e.stopPropagation(); setLayoutOpen(!layoutOpen()); }}
            title="Layout"
          >
            &#x2637;
          </button>
          {layoutOpen() && (
            <div class="layout-dropdown">
              <button class="layout-option" onClick={() => handleLayout("right")}>
                <span class="layout-option-icon">&#x25E8;</span>
                Sidebar Right
              </button>
              <button class="layout-option" onClick={() => handleLayout("left")}>
                <span class="layout-option-icon">&#x25E7;</span>
                Sidebar Left
              </button>
            </div>
          )}
        </div>
        <Show when={isTauri}>
          <button class="titlebar-btn" onClick={handleMinimize} title="Minimize">
            &#x2014;
          </button>
          <button
            class="titlebar-btn titlebar-btn-close"
            onClick={handleClose}
            title="Close"
          >
            &#x2715;
          </button>
        </Show>
      </div>
    </div>
  );
};

export default Titlebar;
