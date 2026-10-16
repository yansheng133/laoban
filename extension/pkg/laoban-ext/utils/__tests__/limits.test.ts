import { describe, it, expect } from '@jest/globals';
import {
  DEFAULT_LIMITS, chars, textState, roundSpace, limitsOf
} from '../limits';
import type { RoundView } from '../backend';

function round(opinions: { text: string; replies?: string[] }[], used?: { chars: number; opinions: number }): RoundView {
  return {
    id:         'r-1',
    app:        'todo',
    initiator:  'u-a',
    scope:      ['web'],
    state:      'collecting',
    deadline:   0,
    opinions:   opinions.map((o, i) => ({
      id: `o${ i + 1 }`, by: 'u-a', at: '', text: o.text, replies: (o.replies || []).map((text) => ({ by: 'u-b', at: '', text }))
    })),
    confirmers: [],
    plan:       null,
    message:    '',
    result:     null,
    log:        [],
    you:        {
      user: 'u-a', isInitiator: true, mustConfirm: false, canAddOpinion: true
    },
    used,
  };
}

describe('limits', () => {
  it('counts characters as the backend does', () => {
    expect(chars('  字字  ')).toBe(2);
    // An emoji is one character, not two UTF-16 units.
    expect(chars('😀😀')).toBe(2);
    expect(chars(null)).toBe(0);
  });

  it('marks text near and over the limit without cutting it', () => {
    expect(textState('x'.repeat(899), 1000)).toEqual({
      count: 899, max: 1000, near: false, over: false
    });
    expect(textState('x'.repeat(900), 1000).near).toBe(true);
    expect(textState('x'.repeat(1000), 1000).over).toBe(false);
    const pasted = '字'.repeat(1001);
    const s = textState(pasted, 1000);

    expect(s.over).toBe(true);
    expect(s.count).toBe(1001);
  });

  it('shows the round space from 70% and stops what does not fit', () => {
    const empty = roundSpace(null, DEFAULT_LIMITS, { chars: 10, opinions: 1 });

    expect(empty.shown).toBe(false);
    expect(empty.over).toBe(false);
    // The backend's count is used when given.
    const r = round([{ text: 'x' }], { chars: 5000, opinions: 6 });

    // Shown from 50% in a neutral colour, a warning from 70% (report 20).
    expect(roundSpace(round([], { chars: 3999, opinions: 1 }), DEFAULT_LIMITS, { chars: 0, opinions: 0 }).shown).toBe(false);
    const half = roundSpace(r, DEFAULT_LIMITS, { chars: 0, opinions: 0 });

    expect([half.shown, half.warn]).toEqual([true, false]);
    const near = roundSpace(r, DEFAULT_LIMITS, { chars: 700, opinions: 1 });

    expect([near.shown, near.warn]).toEqual([true, true]);
    expect(near.over).toBe(false);
    expect(near.left).toBe(3000);
    const over = roundSpace(r, DEFAULT_LIMITS, { chars: 3001, opinions: 1 });

    expect(over.over).toBe(true);
    expect(over.full).toBe(false);
    const full = roundSpace(round([], { chars: 8000, opinions: 9 }), DEFAULT_LIMITS, { chars: 0, opinions: 0 });

    expect(full.full).toBe(true);
    expect(full.left).toBe(0);
    // Too many opinions is full as well.
    const many = roundSpace(round([], { chars: 10, opinions: 30 }), DEFAULT_LIMITS, { chars: 1, opinions: 1 });

    expect(many.full && many.over).toBe(true);
  });

  it('counts replies too when the backend gives no count', () => {
    const r = round([{ text: 'abc', replies: ['de', '😀'] }]);

    expect(roundSpace(r, DEFAULT_LIMITS, { chars: 0, opinions: 0 }).used).toBe(6);
  });

  it('takes the round\'s limits, then the reported ones, then the defaults', () => {
    expect(limitsOf(null)).toEqual(DEFAULT_LIMITS);
    expect(limitsOf(null, { opinionChars: 500 }).opinionChars).toBe(500);
    const r = round([]);

    r.limits = { ...DEFAULT_LIMITS, replyChars: 100 };
    const l = limitsOf(r, { opinionChars: 500 });

    expect([l.opinionChars, l.replyChars]).toEqual([1000, 100]);
  });
});
