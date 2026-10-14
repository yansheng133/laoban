// Small helpers shared by the version list (components/VersionList.vue) and
// the timeline of the enlarged 3D view (pages/AppDetail.vue), so both name
// and order versions the same way.
import type { VersionEntry } from './backend';

const KNOWN_KINDS = ['create', 'edit', 'restore', 'manual', 'layout'];

/** l10n key under laoban.versions.kind for a version's kind. */
export function versionKindKey(kind: string | null | undefined): string {
  return kind && KNOWN_KINDS.includes(kind) ? kind : 'commit';
}

// What a version is compared with in the 3D view.
export type CompareMode = 'previous' | 'current' | 'none';

export const COMPARE_MODES: CompareMode[] = ['previous', 'current', 'none'];

export function isCompareMode(v: unknown): v is CompareMode {
  return typeof v === 'string' && (COMPARE_MODES as string[]).includes(v);
}

/**
 * The version before `commit` (null: the current one) in a newest-first
 * list. `first` when it is the app's first version (nothing before it);
 * `unknown` when the list does not go back far enough.
 */
export function previousOf(versions: VersionEntry[] | null | undefined, commit: string | null, truncated = false):
  { kind: 'version'; entry: VersionEntry } | { kind: 'first' } | { kind: 'unknown' } {
  const list = versions || [];
  const i = commit ? list.findIndex((v) => v.commit === commit) : list.findIndex((v) => v.current);

  if (i < 0) {
    return { kind: 'unknown' };
  }
  const prev = list[i + 1];

  if (prev) {
    return { kind: 'version', entry: prev };
  }

  return truncated ? { kind: 'unknown' } : { kind: 'first' };
}
