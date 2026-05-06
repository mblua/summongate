const FRONTMATTER_RE = /^---[ \t]*\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/;

export function stripFrontmatter(text: string): string {
  if (!text) return text;
  return text.replace(FRONTMATTER_RE, '').replace(/^[ \t]*\r?\n+/, '');
}
