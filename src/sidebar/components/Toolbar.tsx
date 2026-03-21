import { Component, createSignal } from "solid-js";
import { SessionAPI } from "../../shared/ipc";
import SettingsModal from "./SettingsModal";
import OpenAgentModal from "./OpenAgentModal";

const Toolbar: Component = () => {
  const [showSettings, setShowSettings] = createSignal(false);
  const [showOpenAgent, setShowOpenAgent] = createSignal(false);

  const handleNewSession = () => {
    SessionAPI.create();
  };

  return (
    <>
      <div class="toolbar-section">
        <button
          class="toolbar-action-btn toolbar-open-agent-btn"
          onClick={() => setShowOpenAgent(true)}
        >
          &#x25B6; Open Agent
        </button>
      </div>
      <div class="toolbar">
        <button class="toolbar-btn" onClick={handleNewSession}>
          + New Session
        </button>
        <button
          class="toolbar-gear-btn"
          onClick={() => setShowSettings(true)}
          title="Settings"
        >
          &#x2699;
        </button>
      </div>
      {showSettings() && (
        <SettingsModal onClose={() => setShowSettings(false)} />
      )}
      {showOpenAgent() && (
        <OpenAgentModal onClose={() => setShowOpenAgent(false)} />
      )}
    </>
  );
};

export default Toolbar;
