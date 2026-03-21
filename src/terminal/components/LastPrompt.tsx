import { Component, createSignal, onMount, onCleanup } from "solid-js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { terminalStore } from "../stores/terminal";

interface LastPromptProps {
  sessionId?: string;
}

const LastPrompt: Component<LastPromptProps> = (props) => {
  const [lastPrompt, setLastPrompt] = createSignal("");
  let unlisten: UnlistenFn | null = null;

  const getSessionId = () => props.sessionId ?? terminalStore.activeSessionId;

  onMount(async () => {
    unlisten = await listen<{ text: string; sessionId: string }>(
      "last_prompt",
      (event) => {
        if (event.payload.sessionId === getSessionId()) {
          setLastPrompt(event.payload.text);
        }
      }
    );
  });

  onCleanup(() => {
    unlisten?.();
  });

  return (
    <div class="last-prompt-panel">
      <div class="last-prompt-label">LAST PROMPT</div>
      <div class="last-prompt-text">{lastPrompt() || "..."}</div>
    </div>
  );
};

export default LastPrompt;
