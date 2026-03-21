import { Component, For, Show } from "solid-js";
import { sessionsStore } from "../stores/sessions";
import SessionItem from "./SessionItem";

const SessionList: Component = () => {
  return (
    <div class="session-list-container">
      <Show
        when={sessionsStore.sessions.length > 0}
        fallback={
          <div class="empty-state">
            <span>No sessions</span>
            <span>Click + to create one</span>
          </div>
        }
      >
        <For each={sessionsStore.sessions}>
          {(session) => (
            <SessionItem
              session={session}
              isActive={session.id === sessionsStore.activeId}
            />
          )}
        </For>
      </Show>
    </div>
  );
};

export default SessionList;
