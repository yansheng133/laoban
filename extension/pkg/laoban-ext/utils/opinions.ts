// Grouping and drafts for opinions on components (意見收斂).
import type { RoundOpinion, OpinionItem } from './backend';

export interface OpinionGroup {
  // Node id, or null for opinions about the app in general.
  key: string | null;
  name: string;
  opinions: RoundOpinion[];
}

// General opinions first, then one group per component in the order its
// first opinion came.
export function groupOpinions(opinions: RoundOpinion[] | null | undefined): OpinionGroup[] {
  const general: OpinionGroup = { key: null, name: '', opinions: [] };
  const byTarget = new Map<string, OpinionGroup>();

  for (const o of opinions || []) {
    const t = o.target;

    if (!t) {
      general.opinions.push(o);
      continue;
    }
    let g = byTarget.get(t.id);

    if (!g) {
      g = { key: t.id, name: t.name || t.id, opinions: [] };
      byTarget.set(t.id, g);
    }
    g.opinions.push(o);
  }

  return [...(general.opinions.length ? [general] : []), ...byTarget.values()];
}

// Opinions per node id, for the badges in the 3D view.
export function opinionCounts(opinions: RoundOpinion[] | null | undefined): Record<string, number> {
  const out: Record<string, number> = {};

  for (const o of opinions || []) {
    if (o.target && !o.withdrawn) {
      out[o.target.id] = (out[o.target.id] || 0) + 1;
    }
  }

  return out;
}

// An opinion being written for one component, before it is sent.
export interface OpinionDraft {
  target: string;
  targetName: string;
  text: string;
}

export const MAX_BATCH = 10;

// What to send: the general text (if any) and the component drafts, in
// that order; empty texts left out.
export function opinionItems(general: string, drafts: OpinionDraft[]): OpinionItem[] {
  const items: OpinionItem[] = [];

  if (general.trim()) {
    items.push({ text: general.trim() });
  }
  for (const d of drafts) {
    if (d.text.trim()) {
      items.push({ text: d.text.trim(), target: d.target });
    }
  }

  return items;
}

// Add or replace the draft for one component (one draft per component).
// A changed draft keeps its place in the list (report 18: moving it to the
// end made it hard to find); an empty text removes it.
export function putDraft(drafts: OpinionDraft[], d: OpinionDraft): OpinionDraft[] {
  const i = drafts.findIndex((x) => x.target === d.target);

  if (!d.text.trim()) {
    return i < 0 ? drafts : drafts.filter((_, k) => k !== i);
  }
  const next = { ...d, text: d.text.trim() };

  return i < 0 ? [...drafts, next] : drafts.map((x, k) => (k === i ? next : x));
}

// A removed draft and where it was, so it can be put back.
export interface RemovedDraft {
  draft: OpinionDraft;
  index: number;
}

export function removeDraftAt(drafts: OpinionDraft[], target: string): { drafts: OpinionDraft[]; removed: RemovedDraft | null } {
  const index = drafts.findIndex((x) => x.target === target);

  if (index < 0) {
    return { drafts, removed: null };
  }

  return { drafts: drafts.filter((_, k) => k !== index), removed: { draft: drafts[index], index } };
}

// Put a removed draft back where it was, unless that component has a new
// draft by now.
export function restoreDraft(drafts: OpinionDraft[], r: RemovedDraft): OpinionDraft[] {
  if (drafts.some((x) => x.target === r.draft.target)) {
    return drafts;
  }
  const out = [...drafts];

  out.splice(Math.min(r.index, out.length), 0, r.draft);

  return out;
}
