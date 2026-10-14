// A round of 意見收斂 on one app (backend: laoban-m0 rounds.rs).
//
// The detail page reads the round every POLL_MS, so everyone sees new
// opinions, the converged plan and the confirmations. Actions return the
// round as it is after them. When a round publishes the change (on this
// page's confirm/skip, or seen later by polling), `onApplied` gets the
// apply response once, so the page waits for Fleet like for any change.
import {
  ref, computed, onBeforeUnmount, Ref, ComputedRef
} from 'vue';
import {
  getRound, openRound, roundAction, RequestError, StoreLike, RoundView, RoundResponse, RoundAction, ApplyResponse
} from '../utils/backend';

export const POLL_MS = 5 * 1000;

export interface RoundHooks {
  onApplied?: (applied: ApplyResponse) => void;
  // A request failed; the page shows the message.
  messageFor: (e: unknown) => string;
  // Timeout for calls that run the AI (open with deadline 0, converge,
  // the last confirm or skip).
  aiTimeoutMs: () => number;
  // Shown when a running round is gone (the backend restarted).
  lostMessage: () => string;
}

export interface RoundFlow {
  round: Ref<RoundView | null>;
  error: Ref<string>;
  notice: Ref<string>;
  // The action in progress ("open", "confirm", ...), for spinners.
  pending: Ref<string | null>;
  elapsed: Ref<number>;
  // A round runs: direct changes in its scope wait.
  active: ComputedRef<boolean>;
  refresh: () => Promise<void>;
  open: (opinion: string, deadlineMinutes: number, scope: string) => Promise<boolean>;
  act: (action: RoundAction, body?: Record<string, string>) => Promise<boolean>;
  start: (app: string) => void;
  stop: () => void;
}

const ACTIVE = ['collecting', 'converging', 'confirming', 'applying', 'failed'];
// Actions that may run the AI.
const SLOW: RoundAction[] = ['confirm', 'skip', 'converge'];

export function useRound(store: StoreLike, hooks: RoundHooks): RoundFlow {
  const round = ref<RoundView | null>(null);
  const error = ref('');
  const notice = ref('');
  const pending = ref<string | null>(null);
  const elapsed = ref(0);
  let app = '';
  let poll: ReturnType<typeof setInterval> | null = null;
  let tick: ReturnType<typeof setInterval> | null = null;
  // Round IDs whose published change was already handed to the page.
  const seenApplied = new Set<string>();
  // Set after the first read: a round already finished then is only shown.
  let started = false;

  const active = computed(() => !!round.value && ACTIVE.includes(round.value.state));

  function take(res: RoundResponse, appName: string) {
    if (appName !== app) {
      return;
    }
    const r = res.round || null;
    const wasActive = !!round.value && ACTIVE.includes(round.value.state);

    // A running round that is gone: the backend keeps rounds in memory.
    if (wasActive && !r) {
      notice.value = hooks.lostMessage();
    } else if (r && r.id !== round.value?.id) {
      notice.value = '';
    }
    round.value = r;
    const applied = res.applied || (r?.state === 'done' ? r.result : null);

    if (r && applied && !seenApplied.has(r.id)) {
      seenApplied.add(r.id);
      // Only a round finished while this page is open is followed; one
      // finished before (found on first load) is just shown.
      if (res.applied || started) {
        hooks.onApplied?.(applied);
      }
    }
  }

  async function refresh() {
    const name = app;

    if (!name) {
      return;
    }
    try {
      take(await getRound(store, name), name);
      started = true;
    } catch (e) {
      // Polling: keep the last state; actions report their own errors.
      if (!(e instanceof RequestError)) {
        error.value = hooks.messageFor(e);
      }
    }
  }

  function startElapsed() {
    elapsed.value = 0;
    const t0 = Date.now();

    tick = setInterval(() => {
      elapsed.value = Math.floor((Date.now() - t0) / 1000);
    }, 1000);
  }

  function stopElapsed() {
    if (tick) {
      clearInterval(tick);
      tick = null;
    }
  }

  async function run(kind: string, call: () => Promise<RoundResponse>): Promise<boolean> {
    if (pending.value) {
      return false;
    }
    const name = app;

    pending.value = kind;
    error.value = '';
    startElapsed();
    try {
      take(await call(), name);

      return true;
    } catch (e) {
      error.value = hooks.messageFor(e);
      refresh();

      return false;
    } finally {
      pending.value = null;
      stopElapsed();
    }
  }

  function open(opinion: string, deadlineMinutes: number, scope: string) {
    const name = app;

    return run('open', () => openRound(store, name, opinion, deadlineMinutes, scope, hooks.aiTimeoutMs()));
  }

  function act(action: RoundAction, body: Record<string, string> = {}) {
    const name = app;

    return run(action, () => roundAction(store, name, action, body, SLOW.includes(action) ? hooks.aiTimeoutMs() : undefined));
  }

  function stop() {
    if (poll) {
      clearInterval(poll);
      poll = null;
    }
  }

  function start(name: string) {
    stop();
    app = name;
    started = false;
    round.value = null;
    error.value = '';
    notice.value = '';
    seenApplied.clear();
    refresh();
    poll = setInterval(() => {
      if (!pending.value && document.visibilityState !== 'hidden') {
        refresh();
      }
    }, POLL_MS);
  }

  onBeforeUnmount(() => {
    stop();
    stopElapsed();
  });

  return {
    round, error, notice, pending, elapsed, active, refresh, open, act, start, stop
  };
}
