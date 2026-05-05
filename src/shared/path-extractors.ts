function pathParts(workDir: string): string[] {
  return workDir.replace(/\\/g, '/').split('/').filter(s => s.length > 0);
}

export function extractProjectName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = parts.lastIndexOf('.ac-new');
  return idx > 0 ? parts[idx - 1] : null;
}

export function extractWorkgroupName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = parts.lastIndexOf('.ac-new');
  if (idx < 0 || idx + 1 >= parts.length) return null;
  const wg = parts[idx + 1];
  return /^wg-\d+/.test(wg) ? wg.toUpperCase() : null;
}

export function extractAgentName(workDir: string): string | null {
  const parts = pathParts(workDir);
  const idx = parts.lastIndexOf('.ac-new');
  if (idx < 0 || idx + 2 >= parts.length) return null;
  const seg = parts[idx + 2];
  if (!seg.startsWith('__agent_')) return null;
  const name = seg.slice('__agent_'.length);
  return name.length > 0 ? name : null;
}
