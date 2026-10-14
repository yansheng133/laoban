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
    if (o.target) {
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
export function putDraft(drafts: OpinionDraft[], d: OpinionDraft): OpinionDraft[] {
  const rest = drafts.filter((x) => x.target !== d.target);

  return d.text.trim() ? [...rest, { ...d, text: d.text.trim() }] : rest;
}
