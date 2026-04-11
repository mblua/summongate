import type { AgentConfig } from "./types";

export interface AgentPreset {
  key: string;
  label: string;
  description: string;
  color: string;
  config: Omit<AgentConfig, "id">;
}

export const AGENT_PRESETS: AgentPreset[] = [
  {
    key: "claude",
    label: "Claude Code",
    description: "Coding Agent by Anthropic",
    color: "#d97706",
    config: {
      label: "Claude Code",
      command: "claude",
      color: "#d97706",
      gitPullBefore: false,
      excludeGlobalClaudeMd: true,
    },
  },
  {
    key: "codex",
    label: "Codex",
    description: "Coding Agent by OpenAI",
    color: "#10b981",
    config: {
      label: "Codex",
      command: "codex",
      color: "#10b981",
      gitPullBefore: false,
      excludeGlobalClaudeMd: false,
    },
  },
  {
    key: "gemini",
    label: "Gemini CLI",
    description: "Coding Agent by Google",
    color: "#4285f4",
    config: {
      label: "Gemini CLI",
      command: "gemini",
      color: "#4285f4",
      gitPullBefore: false,
      excludeGlobalClaudeMd: false,
    },
  },
];

/** Record-based lookup for SettingsModal quick-add buttons */
export const AGENT_PRESET_MAP: Record<string, Omit<AgentConfig, "id">> =
  Object.fromEntries(AGENT_PRESETS.map((p) => [p.key, p.config]));

/** Known config directories by binary basename */
export const KNOWN_CONFIG_DIRS: Record<string, string> = {
  claude: "~/.claude",
};

/** Extract basename without extension from a path token */
function extractBasename(token: string): string {
  return (
    token
      .replace(/\\/g, "/")
      .split("/")
      .pop()
      ?.replace(/\.(exe|cmd|bat)$/i, "") ?? ""
  );
}

/** Get the default config directory for a command's binary, if known */
export function getDefaultConfigDir(command: string): string | undefined {
  // Check all tokens to handle paths with spaces (G2 fix)
  for (const token of command.split(/\s+/)) {
    const basename = extractBasename(token);
    if (KNOWN_CONFIG_DIRS[basename]) return KNOWN_CONFIG_DIRS[basename];
  }
  return undefined;
}

/** Check if a command appears to be Claude-based (any token's basename starts with "claude") */
export function isClaudeBased(command: string): boolean {
  return command.split(/\s+/).some((token) => {
    const basename = extractBasename(token);
    return basename.startsWith("claude");
  });
}

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
