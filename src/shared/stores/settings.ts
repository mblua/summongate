import { createSignal } from "solid-js";
import type { AppSettings } from "../types";
import { SettingsAPI } from "../ipc";

const [settings, setSettings] = createSignal<AppSettings | null>(null);

export const settingsStore = {
  get current() {
    return settings();
  },

  get voiceEnabled() {
    const s = settings();
    return !!s?.voiceToTextEnabled && !!s?.geminiApiKey;
  },

  async load() {
    const s = await SettingsAPI.get();
    setSettings(s);
  },

  refresh() {
    void settingsStore.load();
  },
};
