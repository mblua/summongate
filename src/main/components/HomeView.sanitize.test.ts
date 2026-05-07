// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";

const md = MarkdownIt({ html: false, linkify: true, typographer: false, breaks: false });
const render = (src: string) =>
  DOMPurify.sanitize(md.render(src), { USE_PROFILES: { html: true } });

describe("HomeView Markdown sanitization", () => {
  it("escapes raw HTML in Markdown source (html:false)", () => {
    const out = render('Hello <script>alert(1)</script> world');
    expect(out).not.toContain("<script>");
  });

  it("strips javascript: URLs in links", () => {
    // markdown-it's link-rule validator rejects `javascript:` and emits the
    // bracketed source as plain text (no anchor at all). DOMPurify is the
    // second line of defence in case a future plugin slips a dangerous href
    // through. The security property is "no clickable javascript: link",
    // not "literal string absent" (the escaped text legitimately remains).
    const out = render('[click](javascript:alert(1))');
    expect(out).not.toMatch(/<a[^>]*href=["']?\s*javascript:/i);
  });

  it("preserves http(s) anchors", () => {
    const out = render('[ok](https://example.com)');
    expect(out).toContain('href="https://example.com"');
  });

  it("renders code blocks", () => {
    const out = render("```\nlet x = 1;\n```");
    expect(out).toMatch(/<pre><code>[\s\S]*let x = 1[\s\S]*<\/code><\/pre>/);
  });
});
