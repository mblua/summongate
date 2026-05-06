import { Component, createMemo } from "solid-js";
import { terminalStore } from "../stores/terminal";
import { stripFrontmatter } from "../../shared/markdown";

const WorkgroupBrief: Component = () => {
  const currentBrief = createMemo(() => stripFrontmatter(terminalStore.activeWorkgroupBrief ?? "").trim());

  return (
    <div class="workgroup-brief-panel">
      <div class="workgroup-brief-label">BRIEF</div>
      <div class="workgroup-brief-text">{currentBrief() || "..."}</div>
    </div>
  );
};

export default WorkgroupBrief;
