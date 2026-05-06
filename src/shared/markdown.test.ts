import { describe, it, expect } from 'vitest';
import { stripFrontmatter } from './markdown';

describe('stripFrontmatter', () => {
  it('empty_input_returns_empty', () => {
    expect(stripFrontmatter('')).toBe('');
  });

  it('no_frontmatter_returns_unchanged', () => {
    const body = 'Hello world\nLine two';
    expect(stripFrontmatter(body)).toBe(body);
  });

  it('strips_basic_frontmatter', () => {
    const input = "---\ntitle: 'Foo'\n---\n\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('strips_multiline_frontmatter', () => {
    const input = "---\ntitle: 'Foo'\nstatus: active\nowner: alice\n---\n\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('handles_crlf_line_endings', () => {
    const input = "---\r\ntitle: 'Foo'\r\n---\r\n\r\nBody text";
    expect(stripFrontmatter(input)).toBe('Body text');
  });

  it('preserves_inline_dashes_in_body', () => {
    const input = "---\ntitle: x\n---\n\nA --- separator in the body\n---\nshould stay";
    expect(stripFrontmatter(input)).toBe('A --- separator in the body\n---\nshould stay');
  });

  it('unclosed_frontmatter_returns_unchanged', () => {
    const input = "---\ntitle: 'never closed'\nbody continues here";
    expect(stripFrontmatter(input)).toBe(input);
  });

  it('frontmatter_only_no_body_returns_empty', () => {
    expect(stripFrontmatter("---\ntitle: x\n---")).toBe('');
    expect(stripFrontmatter("---\ntitle: x\n---\n")).toBe('');
  });

  it('triple_dash_not_at_start_is_not_frontmatter', () => {
    const input = "Heading\n---\nfield: value\n---\nBody";
    expect(stripFrontmatter(input)).toBe(input);
  });

  it('trailing_whitespace_after_closing_delimiter_handled', () => {
    const input = "---\ntitle: x\n---   \n\nBody";
    expect(stripFrontmatter(input)).toBe('Body');
  });
});
