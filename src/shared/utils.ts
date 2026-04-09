/**
 * Extract the project root path from a working directory inside .ac-new.
 * Returns null if the path doesn't contain .ac-new.
 */
export function extractProjectPath(workDir: string): string | null {
  const norm = workDir.replace(/\\/g, "/");
  const idx = norm.indexOf("/.ac-new");
  return idx > 0 ? norm.substring(0, idx) : null;
}
