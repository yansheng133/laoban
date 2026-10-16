// Size limits of a round of 意見收斂, as the backend reports them
// (rounds.rs Limits). The page checks the same limits before sending, so
// nobody finds out only after pressing the button; the backend checks
// again. Text that is too long is never cut: it stays as typed, marked,
// and cannot be sent until it is shorter.
import type { RoundView } from './backend';

export interface RoundLimits {
  opinionChars: number;
  replyChars: number;
  roundChars: number;
  opinions: number;
  replies: number;
  batch: number;
}

// The backend's built-in (and highest) values, for a backend that does not
// report its limits.
export const DEFAULT_LIMITS: RoundLimits = {
  opinionChars: 1000, replyChars: 300, roundChars: 8000, opinions: 30, replies: 20, batch: 10
};

// Show how full the round is from this share on.
export const SPACE_SHOWN_AT = 0.7;
// The counter turns to a warning from this share on.
export const NEAR_AT = 0.9;

// Characters as the backend counts them: Unicode code points of the
// trimmed text (an emoji is one, as is a CJK character).
export function chars(text: string | null | undefined): number {
  return Array.from((text || '').trim()).length;
}

export interface TextState {
  count: number;
  max: number;
  near: boolean;
  over: boolean;
}

export function textState(text: string | null | undefined, max: number): TextState {
  const count = chars(text);

  return {
    count, max, near: count >= max * NEAR_AT, over: count > max
  };
}

export interface RoundSpace {
  // Characters used by the round's opinions and replies.
  used: number;
  // Characters about to be sent.
  adding: number;
  max: number;
  // Characters left before what is about to be sent.
  left: number;
  // (used + adding) / max, for the bar.
  share: number;
  // Show the bar.
  shown: boolean;
  // Nothing more fits (characters or number of opinions).
  full: boolean;
  // What is about to be sent does not fit.
  over: boolean;
}

// What the round has used: the backend's count when it gives one.
function usedOf(round: RoundView | null): { chars: number; opinions: number } {
  if (!round) {
    return { chars: 0, opinions: 0 };
  }
  if (round.used) {
    return round.used;
  }
  const n = (round.opinions || []).reduce((sum, o) => sum + chars(o.text) + (o.replies || []).reduce((s, r) => s + chars(r.text), 0), 0);

  return { chars: n, opinions: (round.opinions || []).length };
}

export function roundSpace(round: RoundView | null, limits: RoundLimits, adding: { chars: number; opinions: number }): RoundSpace {
  const used = usedOf(round);
  const max = limits.roundChars;
  const total = used.chars + adding.chars;
  const full = used.chars >= max || used.opinions >= limits.opinions;
  const over = total > max || used.opinions + adding.opinions > limits.opinions;
  const share = max > 0 ? Math.min(1, total / max) : 1;

  return {
    used:   used.chars,
    adding: adding.chars,
    max,
    left:   Math.max(0, max - used.chars),
    share,
    shown:  share >= SPACE_SHOWN_AT || full || over,
    full,
    over,
  };
}

// The round's own limits, else the backend's reported ones, else the
// defaults.
export function limitsOf(round: RoundView | null | undefined, reported?: Partial<RoundLimits> | null): RoundLimits {
  return { ...DEFAULT_LIMITS, ...(reported || {}), ...(round?.limits || {}) };
}
