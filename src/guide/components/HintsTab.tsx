import { Component, For } from "solid-js";

interface Hint {
  title: string;
  body: string;
  link?: { label: string; url: string };
}

const hints: Hint[] = [
  {
    title: "--enable-auto-mode",
    body: "Claude Code has an \"Auto\" mode that replaces permission prompts with an intelligent safety classifier. It auto-approves safe actions (reads, local edits) and blocks risky ones (force push, mass deletion, data exfiltration). It's the ideal middle ground between asking permission for everything and the dangerous --dangerously-skip-permissions.",
    link: {
      label: "Learn more in the official docs",
      url: "https://docs.anthropic.com/en/docs/claude-code/security#auto-accept-mode",
    },
  },
];

const HintsTab: Component = () => {
  return (
    <div class="guide-tab-content">
      <For each={hints}>
        {(hint) => (
          <div class="guide-card">
            <div class="guide-card-title">{hint.title}</div>
            <div class="guide-card-body">{hint.body}</div>
            {hint.link && (
              <a
                class="guide-card-link"
                href={hint.link.url}
                target="_blank"
                rel="noopener noreferrer"
              >
                {hint.link.label} &rarr;
              </a>
            )}
          </div>
        )}
      </For>
    </div>
  );
};

export default HintsTab;
