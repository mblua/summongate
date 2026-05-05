import { describe, it, expect } from 'vitest';
import { extractProjectName, extractWorkgroupName, extractAgentName } from './path-extractors';

describe('path-extractors', () => {
  it('empty_input_returns_all_null', () => {
    const w = '';
    expect(extractProjectName(w)).toBeNull();
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('path_without_ac_new_returns_all_null', () => {
    const w = 'C:\\foo\\bar';
    expect(extractProjectName(w)).toBeNull();
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('ac_new_only_returns_project_only', () => {
    const w = 'C:\\foo\\.ac-new';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBeNull();
  });

  it('agent_in_wg_returns_all_three', () => {
    const w = 'C:\\foo\\.ac-new\\wg-19-dev-team\\__agent_tech-lead';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-19-DEV-TEAM');
    expect(extractAgentName(w)).toBe('tech-lead');
  });

  it('repo_in_wg_returns_project_and_wg_no_agent', () => {
    const w = 'C:\\foo\\.ac-new\\wg-19-dev-team\\repo-X';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-19-DEV-TEAM');
    expect(extractAgentName(w)).toBeNull();
  });

  it('bare_underscore_agent_returns_no_agent', () => {
    const w = 'C:\\foo\\.ac-new\\wg-1\\__agent_';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBeNull();
  });

  it('nested_ac_new_uses_innermost', () => {
    const w = 'C:\\proj\\.ac-new\\wg-1-outer\\repo-AC\\.ac-new\\wg-2-inner\\__agent_alice';
    expect(extractProjectName(w)).toBe('repo-AC');
    expect(extractWorkgroupName(w)).toBe('WG-2-INNER');
    expect(extractAgentName(w)).toBe('alice');
  });

  it('unc_prefix_handled', () => {
    const w = '\\\\?\\C:\\proj\\.ac-new\\wg-1\\__agent_x';
    expect(extractProjectName(w)).toBe('proj');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });

  it('trailing_slash_handled', () => {
    const w = 'C:\\foo\\.ac-new\\wg-1\\__agent_x\\';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });

  it('lax_wg_segment_rejected_no_digits', () => {
    const w = 'C:\\foo\\.ac-new\\wg-foo\\__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('x');
  });

  it('lax_wg_segment_rejected_bare_dash', () => {
    const w = 'C:\\foo\\.ac-new\\wg-\\__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBeNull();
    expect(extractAgentName(w)).toBe('x');
  });

  it('forward_slashes_handled', () => {
    const w = '/foo/.ac-new/wg-1/__agent_x';
    expect(extractProjectName(w)).toBe('foo');
    expect(extractWorkgroupName(w)).toBe('WG-1');
    expect(extractAgentName(w)).toBe('x');
  });
});
