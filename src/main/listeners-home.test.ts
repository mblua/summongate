import { describe, it, expect, vi, beforeEach } from "vitest";

type SwitchedPayload = { id: string | null; userInitiated?: boolean };
type DestroyedPayload = { id: string };

// Capture the listener callbacks so individual cases can fire crafted
// payloads through the same code path the backend would use.
const m = vi.hoisted(() => ({
  switchedCb: null as ((data: SwitchedPayload) => void) | null,
  destroyedCb: null as ((data: DestroyedPayload) => void | Promise<void>) | null,
  list: vi.fn(),
}));

vi.mock("../shared/ipc", () => ({
  SessionAPI: {
    list: m.list,
  },
  onSessionSwitched: vi.fn((cb: (data: SwitchedPayload) => void) => {
    m.switchedCb = cb;
    return Promise.resolve(() => {});
  }),
  onSessionDestroyed: vi.fn((cb: (data: DestroyedPayload) => void | Promise<void>) => {
    m.destroyedCb = cb;
    return Promise.resolve(() => {});
  }),
  // homeStore imports HomeAPI; stub it so the module graph loads.
  HomeAPI: { fetchMarkdown: vi.fn() },
}));

import { wireHomeListeners } from "./listeners-home";
import { homeStore, __resetHomeStoreForTests } from "./stores/home";

describe("wireHomeListeners (issue #183)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetHomeStoreForTests();
    m.switchedCb = null;
    m.destroyedCb = null;
  });

  it("shows Home unconditionally on wire-up", async () => {
    expect(homeStore.visible).toBe(false);
    await wireHomeListeners();
    expect(homeStore.visible).toBe(true);
  });

  it("session_switched with userInitiated=true and a real id hides Home", async () => {
    await wireHomeListeners();
    expect(homeStore.visible).toBe(true);
    m.switchedCb!({ id: "abc", userInitiated: true });
    expect(homeStore.visible).toBe(false);
  });

  it("session_switched WITHOUT userInitiated leaves Home visible (restore / auto-promote)", async () => {
    await wireHomeListeners();
    expect(homeStore.visible).toBe(true);
    m.switchedCb!({ id: "abc" });
    expect(homeStore.visible).toBe(true);
  });

  it("session_switched with userInitiated=false leaves Home visible", async () => {
    await wireHomeListeners();
    expect(homeStore.visible).toBe(true);
    m.switchedCb!({ id: "abc", userInitiated: false });
    expect(homeStore.visible).toBe(true);
  });

  it("session_switched with id=null leaves Home visible even when userInitiated=true", async () => {
    await wireHomeListeners();
    expect(homeStore.visible).toBe(true);
    m.switchedCb!({ id: null, userInitiated: true });
    expect(homeStore.visible).toBe(true);
  });

  it("session_destroyed shows Home when no sessions remain (#164 contract)", async () => {
    m.list.mockResolvedValueOnce([]);
    await wireHomeListeners();
    homeStore.hide();
    expect(homeStore.visible).toBe(false);
    await m.destroyedCb!({ id: "abc" });
    expect(homeStore.visible).toBe(true);
  });

  it("session_destroyed leaves Home hidden when other sessions remain", async () => {
    m.list.mockResolvedValueOnce([{ id: "x" }]);
    await wireHomeListeners();
    homeStore.hide();
    expect(homeStore.visible).toBe(false);
    await m.destroyedCb!({ id: "abc" });
    expect(homeStore.visible).toBe(false);
  });
});
