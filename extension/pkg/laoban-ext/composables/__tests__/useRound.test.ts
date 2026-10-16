/**
 * @jest-environment jsdom
 */
// Report 18, B8: a request that runs the AI was cut off after about 60 s
// and the page said it could not reach the backend, though the backend
// went on and published. Now the round itself decides.
import {
  describe, it, expect, jest, beforeEach
} from '@jest/globals';
import { effectScope } from 'vue';
import { RequestError } from '../../utils/backend';

const mockGetRound = jest.fn<(...a: unknown[]) => Promise<unknown>>();
const mockRoundAction = jest.fn<(...a: unknown[]) => Promise<unknown>>();

jest.mock('../../utils/backend', () => {
  const actual = jest.requireActual('../../utils/backend') as Record<string, unknown>;

  return {
    ...actual,
    getRound:    (...a: unknown[]) => mockGetRound(...a),
    roundAction: (...a: unknown[]) => mockRoundAction(...a),
  };
});

// eslint-disable-next-line import/first
import { useRound } from '../useRound';

const round = (state: string, id = 'r1') => ({
  id, state, app: 'todo', initiator: 'u-a', scope: ['web'], deadline: 0, opinions: [], confirmers: [], plan: null, message: '', result: state === 'done' ? { commit: 'c' } : null, log: [], you: { user: 'u-a', isInitiator: true, mustConfirm: true, canAddOpinion: false }
});

function setup() {
  const applied: unknown[] = [];
  const scope = effectScope();
  const flow = scope.run(() => useRound({} as never, {
    onApplied:   (a) => applied.push(a),
    messageFor:  (e) => `error: ${ (e as Error).message }`,
    aiTimeoutMs: () => 1000,
    lostMessage: () => 'lost',
    slowMessage: () => 'slow',
  }))!;

  return { flow, applied, scope };
}

describe('useRound when a slow request gets no answer', () => {
  beforeEach(() => {
    // Used outside a component here: Vue warns that onBeforeUnmount has no
    // instance, which is expected.
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
    mockGetRound.mockReset();
    mockRoundAction.mockReset();
  });

  it('a cut-off confirm while the round is applying is not an error', async() => {
    const { flow, scope } = setup();

    mockGetRound.mockResolvedValue({ round: round('confirming') });
    flow.start('todo');
    await flow.refresh();
    mockRoundAction.mockRejectedValue(new RequestError('timeout', null));
    mockGetRound.mockResolvedValue({ round: round('applying') });

    expect(await flow.act('confirm')).toBe(true);
    expect(flow.error.value).toBe('');
    expect(flow.notice.value).toBe('slow');
    scope.stop();
    flow.stop();
  });

  it('the notice goes and the deploy is followed once the round is done', async() => {
    const { flow, applied, scope } = setup();

    mockGetRound.mockResolvedValue({ round: round('confirming') });
    flow.start('todo');
    await flow.refresh();
    mockRoundAction.mockRejectedValue(new RequestError('network', null));
    mockGetRound.mockResolvedValue({ round: round('applying') });
    await flow.act('confirm');
    mockGetRound.mockResolvedValue({ round: round('done') });
    await flow.refresh();

    expect(flow.notice.value).toBe('');
    expect(applied).toEqual([{ commit: 'c' }]);
    scope.stop();
    flow.stop();
  });

  it('still an error when the round did not move', async() => {
    const { flow, scope } = setup();

    mockGetRound.mockResolvedValue({ round: round('confirming') });
    flow.start('todo');
    await flow.refresh();
    mockRoundAction.mockRejectedValue(new RequestError('timeout', null));

    expect(await flow.act('confirm')).toBe(false);
    expect(flow.error.value).toContain('error');
    scope.stop();
    flow.stop();
  });

  it("the backend's own error is shown as is", async() => {
    const { flow, scope } = setup();

    mockGetRound.mockResolvedValue({ round: round('confirming') });
    flow.start('todo');
    await flow.refresh();
    mockRoundAction.mockRejectedValue(new RequestError('not_confirmer', '你不在確認名單裡。', 403));
    mockGetRound.mockResolvedValue({ round: round('applying') });

    expect(await flow.act('confirm')).toBe(false);
    expect(flow.error.value).toContain('你不在確認名單裡');
    scope.stop();
    flow.stop();
  });
});

describe('useRound limits and availability', () => {
  beforeEach(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
    mockGetRound.mockReset();
  });

  it('takes the limits and whether rounds can be stored from the backend', async() => {
    const { flow, scope } = setup();

    expect(flow.limits.value.opinionChars).toBe(1000);
    expect(flow.available.value).toBe(true);
    mockGetRound.mockResolvedValue({
      round: null, roundsAvailable: false, limits: {
        opinionChars: 500, replyChars: 100, roundChars: 4000, opinions: 10, replies: 5, batch: 10
      }
    });
    flow.start('todo');
    await flow.refresh();
    expect(flow.available.value).toBe(false);
    expect(flow.limits.value.opinionChars).toBe(500);
    // A round's own limits win.
    mockGetRound.mockResolvedValue({
      round: { ...round('collecting'), limits: { replyChars: 50 } }, roundsAvailable: true
    });
    await flow.refresh();
    expect(flow.available.value).toBe(true);
    expect([flow.limits.value.opinionChars, flow.limits.value.replyChars]).toEqual([1000, 50]);
    scope.stop();
    flow.stop();
  });
});
