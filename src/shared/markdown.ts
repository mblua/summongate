const FRONTMATTER_RE = /^---[ \t]*\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/;
const BOM = 0xfeff;

export function stripFrontmatter(text: string): string {
  if (!text) return text;
  const stripped = text.charCodeAt(0) === BOM ? text.slice(1) : text;
  return stripped.replace(FRONTMATTER_RE, '').replace(/^[ \t]*\r?\n+/, '');
}
