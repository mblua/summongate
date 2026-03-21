import { Component } from "solid-js";
import type { Session, SessionStatus } from "../../shared/types";
import { SessionAPI } from "../../shared/ipc";

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
}> = (props) => {
  const handleClick = () => {
    SessionAPI.switch(props.session.id);
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    SessionAPI.destroy(props.session.id);
  };

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""}`}
      onClick={handleClick}
    >
      <div
        class={`session-item-status ${statusClass(props.session.status)}`}
      />
      <div class="session-item-info">
        <div class="session-item-name">{props.session.name}</div>
        <div class="session-item-shell">{props.session.shell}</div>
      </div>
      <button class="session-item-close" onClick={handleClose} title="Close session">
        &#x2715;
      </button>
    </div>
  );
};

export default SessionItem;
