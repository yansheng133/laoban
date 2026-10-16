import { describe, it, expect } from '@jest/globals';
import { confirmCount } from '../waiting';

describe('waiting rounds', () => {
  it('counts only what is the caller\'s to confirm', () => {
    expect(confirmCount([
      {
        app: 'a', roundId: 'r1', state: 'confirming', reason: 'confirm', pending: 2
      },
      {
        app: 'b', roundId: 'r2', state: 'confirming', reason: 'othersConfirm', pending: 1
      },
      {
        app: 'c', roundId: 'r3', state: 'failed', reason: 'retry', pending: 0
      },
    ])).toBe(1);
    expect(confirmCount([])).toBe(0);
  });
});
