import { Component, createSignal, onMount, onCleanup } from "solid-js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const LastPrompt: Component = () => {
  const [lastPrompt, setLastPrompt] = createSignal("");
  let unlisten: UnlistenFn | null = null;

  onMount(async () => {
    unlisten = await listen<{ text: string }>("last_prompt", (event) => {
      setLastPrompt(event.payload.text);
    });
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
