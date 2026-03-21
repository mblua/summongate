import { Component } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";

const Titlebar: Component = () => {
  const appWindow = getCurrentWindow();

  const handleMinimize = () => appWindow.minimize();
  const handleClose = () => appWindow.close();

  return (
    <div class="titlebar" data-tauri-drag-region>
      <span class="titlebar-title" data-tauri-drag-region>
        win-nerds-tab
      </span>
      <div class="titlebar-controls">
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
      </div>
    </div>
  );
};

export default Titlebar;
