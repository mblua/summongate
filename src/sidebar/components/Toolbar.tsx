import { Component } from "solid-js";
import { SessionAPI } from "../../shared/ipc";

const Toolbar: Component = () => {
  const handleNewSession = () => {
    SessionAPI.create();
  };

  return (
    <div class="toolbar">
      <button class="toolbar-btn" onClick={handleNewSession}>
        + New Session
      </button>
    </div>
  );
};

export default Toolbar;
