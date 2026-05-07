import { Component, Show, createMemo, onMount } from "solid-js";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import { homeStore } from "../stores/home";
import { WindowAPI } from "../../shared/ipc";

const md = MarkdownIt({
  html: false,
  linkify: true,
  typographer: false,
  breaks: false,
});

const HomeView: Component = () => {
  onMount(() => {
    if (homeStore.content === null && homeStore.error === null && !homeStore.loading) {
      homeStore.fetch();
    }
  });

  const html = createMemo(() => {
    const src = homeStore.content;
    if (!src) return "";
    return DOMPurify.sanitize(md.render(src), {
      USE_PROFILES: { html: true },
    });
  });

  const onContainerClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    const anchor = target.closest("a") as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute("href") ?? "";
    if (!href) return;
    e.preventDefault();
    WindowAPI.openExternal(href).catch((err) => console.error("openExternal failed:", err));
  };

  return (
    <div class="home-view">
      <div class="home-toolbar">
        <button
          class="home-refresh-btn"
          title="Refresh"
          disabled={homeStore.loading}
          onClick={() => homeStore.refresh()}
        >
          ↻
        </button>
      </div>
      <Show when={homeStore.loading && homeStore.content === null}>
        <div class="home-status">Loading Home…</div>
      </Show>
      <Show when={homeStore.error && homeStore.content === null}>
        <div class="home-status home-status-error">
          <p>Could not load Home: {homeStore.error}</p>
          <button class="home-retry-btn" onClick={() => homeStore.fetch()}>
            Try again
          </button>
        </div>
      </Show>
      <Show when={homeStore.content !== null}>
        <div
          class="home-markdown"
          onClick={onContainerClick}
          // eslint-disable-next-line solid/no-innerhtml
          innerHTML={html()}
        />
      </Show>
    </div>
  );
};

export default HomeView;
