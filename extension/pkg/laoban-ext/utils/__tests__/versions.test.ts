import { describe, it, expect } from '@jest/globals';
import {
  versionKindKey, previousOf, isCompareMode, changedFiles
} from '../versions';
import type { VersionEntry } from '../backend';

const v = (version: number, current = false): VersionEntry => ({
  version, commit: `c${ version }`, commitUrl: '', at: '', kind: 'edit', request: '', summary: '', model: '', current
});

describe('versions', () => {
  it('names unknown kinds "other change", the same in list and timeline', () => {
    expect(versionKindKey('manual')).toBe('manual');
    expect(versionKindKey('layout')).toBe('layout');
    expect(versionKindKey('commit')).toBe('commit');
    expect(versionKindKey('something')).toBe('commit');
    expect(versionKindKey(undefined)).toBe('commit');
  });

  it('finds the version before a selected one, or before the current one', () => {
    const list = [v(3, true), v(2), v(1)];

    expect(previousOf(list, 'c2')).toEqual({ kind: 'version', entry: list[2] });
    expect(previousOf(list, null)).toEqual({ kind: 'version', entry: list[1] });
    expect(previousOf(list, 'c1')).toEqual({ kind: 'first' });
    // The list stops before the app's start: not known.
    expect(previousOf(list, 'c1', true)).toEqual({ kind: 'unknown' });
    expect(previousOf(list, 'nope')).toEqual({ kind: 'unknown' });
    expect(previousOf(null, null)).toEqual({ kind: 'unknown' });
  });

  it('accepts only the three compare modes', () => {
    expect(isCompareMode('previous')).toBe(true);
    expect(isCompareMode('none')).toBe(true);
    expect(isCompareMode('all')).toBe(false);
  });
});

describe('changedFiles', () => {
  const d = (index: string) => ({
    'web/index.html': index, 'web/app.js': 'a', 'web/style.css': 's', 'api/handler.py': 'h'
  });

  it('lists the files whose digests differ', () => {
    // Report 16b: version 9 only changed index.html by hand.
    expect(changedFiles(d('1'), d('2'))).toEqual(['web/index.html']);
    expect(changedFiles(d('1'), d('1'))).toEqual([]);
  });

  it('does not know without digests on both sides', () => {
    expect(changedFiles(null, d('1'))).toBeNull();
    expect(changedFiles(d('1'), undefined)).toBeNull();
  });
});
