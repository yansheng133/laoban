import { describe, it, expect } from '@jest/globals';
import {
  groupOpinions, opinionCounts, opinionItems, putDraft
} from '../opinions';
import type { RoundOpinion } from '../backend';

const op = (id: string, target?: string): RoundOpinion => ({
  id, by: 'u-a', at: 't', text: id, ...(target ? { target: { id: target, name: `${ target }-name` } } : {})
});

describe('opinions on components', () => {
  it('groups general opinions first, then by component', () => {
    const g = groupOpinions([op('o1', 'app-web'), op('o2'), op('o3', 'app-api'), op('o4', 'app-web')]);

    expect(g.map((x) => [x.key, x.opinions.map((o) => o.id)])).toEqual([
      [null, ['o2']], ['app-web', ['o1', 'o4']], ['app-api', ['o3']],
    ]);
    expect(g[1].name).toBe('app-web-name');
    expect(groupOpinions(null)).toEqual([]);
  });

  it('counts opinions per component for the 3D badges', () => {
    expect(opinionCounts([op('o1', 'a'), op('o2'), op('o3', 'a'), op('o4', 'b')])).toEqual({ a: 2, b: 1 });
  });

  it('keeps one draft per component and builds what to send', () => {
    let d = putDraft([], { target: 'a', targetName: 'A', text: ' x ' });

    d = putDraft(d, { target: 'b', targetName: 'B', text: 'y' });
    d = putDraft(d, { target: 'a', targetName: 'A', text: 'x2' });
    expect(d.map((x) => [x.target, x.text])).toEqual([['b', 'y'], ['a', 'x2']]);
    expect(putDraft(d, { target: 'b', targetName: 'B', text: ' ' }).map((x) => x.target)).toEqual(['a']);
    expect(opinionItems(' 一般 ', d)).toEqual([{ text: '一般' }, { text: 'y', target: 'b' }, { text: 'x2', target: 'a' }]);
    expect(opinionItems('', [])).toEqual([]);
  });
});
