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
  getRound, openRound, roundAction, addOpinions, supportOpinion, replyToOpinion, RequestError, StoreLike, RoundView,
  RoundResponse, RoundAction, ApplyResponse, OpinionItem, editOpinion, withdrawOpinion, setRoundDeadline,
  saveRoundDrafts, commentOnPlan, supportPlanComment
} from '../utils/backend';
import { DEFAULT_LIMITS, RoundLimits } from '../utils/limits';

export const POLL_MS = 5 * 1000;

export interface RoundHooks {
  onApplied?: (applied: ApplyResponse) => void;
  // A request failed; the page shows the message.
  messageFor: (e: unknown) => string;
  // Timeout for calls that run the AI (open with deadline 0, converge,
  // the last confirm or skip).
  aiTimeoutMs: () => number;
  // Shown when a running round is gone.
  lostMessage: () => string;
  // Shown when a request that runs the AI got no answer but the round
  // shows the work going on (see run()).
  slowMessage: () => string;
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
  // Size limits (of the round, or for a new one), as the backend reports.
  limits: Ref<RoundLimits>;
  // False when the backend cannot store rounds (the Round CRD is missing).
  available: Ref<boolean>;
  refresh: () => Promise<void>;
  open: (items: OpinionItem[], deadlineMinutes: number, scope: string) => Promise<boolean>;
  act: (action: RoundAction, body?: Record<string, string>) => Promise<boolean>;
  add: (items: OpinionItem[]) => Promise<boolean>;
  support: (opinionId: string) => Promise<boolean>;
  reply: (opinionId: string, text: string) => Promise<boolean>;
  edit: (opinionId: string, text: string) => Promise<boolean>;
  withdraw: (opinionId: string) => Promise<boolean>;
  deadline: (minutes: number) => Promise<boolean>;
  comment: (step: number, text: string) => Promise<boolean>;
  supportComment: (commentId: string) => Promise<boolean>;
  // Keep drafts in the round; quiet (no spinner, errors only noted).
  saveDrafts: (items: OpinionItem[], shared: boolean) => Promise<boolean>;
  start: (app: string) => void;
  stop: () => void;
}

const ACTIVE = ['collecting', 'converging', 'confirming', 'applying', 'failed'];
// Actions that may run the AI.
const SLOW: RoundAction[] = ['confirm', 'skip', 'converge'];

// The backend now answers these at once and runs the AI in the background
// (report 18, B8). An older backend, or a slow network, may still cut the
// request off (the Kubernetes API server ends requests after about 60 s).
// Then the round itself tells whether the action happened: these are the
// states it can be in after each one.
const AFTER: Record<string, string[]> = {
  confirm:  ['applying', 'done'],
  skip:     ['applying', 'done'],
  converge: ['converging', 'confirming'],
  open:     ['collecting', 'converging', 'confirming'],
};

// No answer from the backend itself: a timeout, a network error, or a
// proxy / API server error without the backend's own message.
function noAnswer(e: unknown): boolean {
  return !(e instanceof RequestError) || !e.userMessage || ['timeout', 'network'].includes(e.code);
}

export function useRound(store: StoreLike, hooks: RoundHooks): RoundFlow {
  const round = ref<RoundView | null>(null);
  const error = ref('');
  const notice = ref('');
  const pending = ref<string | null>(null);
  const elapsed = ref(0);
  const limits = ref<RoundLimits>({ ...DEFAULT_LIMITS });
  const available = ref(true);
  let app = '';
  let poll: ReturnType<typeof setInterval> | null = null;
  let tick: ReturnType<typeof setInterval> | null = null;
  // Round IDs whose published change was already handed to the page.
  const seenApplied = new Set<string>();
  // Set after the first read: a round already finished then is only shown.
  let started = false;
  // The slow-request notice is up; it goes once the work is over.
  let slowShown = false;

  const active = computed(() => !!round.value && ACTIVE.includes(round.value.state));

  function take(res: RoundResponse, appName: string) {
    if (appName !== app) {
      return;
    }
    const r = res.round || null;
    const wasActive = !!round.value && ACTIVE.includes(round.value.state);

    if (typeof res.roundsAvailable === 'boolean') {
      available.value = res.roundsAvailable;
    }
    if (r?.limits || res.limits) {
      limits.value = { ...DEFAULT_LIMITS, ...(res.limits || {}), ...(r?.limits || {}) };
    }
    // A running round that is gone (deleted from the cluster).
    if (wasActive && !r) {
      notice.value = hooks.lostMessage();
    } else if (r && r.id !== round.value?.id) {
      notice.value = '';
    }
    round.value = r;
    if (slowShown && (!r || !['converging', 'applying'].includes(r.state))) {
      slowShown = false;
      notice.value = '';
    }
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
    const before = round.value?.id || null;

    try {
      take(await call(), name);

      return true;
    } catch (e) {
      if (AFTER[kind] && noAnswer(e)) {
        await refresh();
        const r = round.value;
        // "open" made a round only if it is a new one.
        const happened = !!r && AFTER[kind].includes(r.state) && (kind !== 'open' || r.id !== before);

        if (happened) {
          // Still working: say so until it ends. Already over: nothing to
          // add, the round shows the result.
          slowShown = ['converging', 'applying'].includes(r!.state);
          if (slowShown) {
            notice.value = hooks.slowMessage();
          }

          return true;
        }
      }
      error.value = hooks.messageFor(e);
      refresh();

      return false;
    } finally {
      pending.value = null;
      stopElapsed();
    }
  }

  function open(items: OpinionItem[], deadlineMinutes: number, scope: string) {
    const name = app;

    return run('open', () => openRound(store, name, items, deadlineMinutes, scope, hooks.aiTimeoutMs()));
  }

  function add(items: OpinionItem[]) {
    const name = app;

    return run('opinions', () => addOpinions(store, name, items));
  }

  function support(opinionId: string) {
    const name = app;

    return run('support', () => supportOpinion(store, name, opinionId));
  }

  function reply(opinionId: string, text: string) {
    const name = app;

    return run('reply', () => replyToOpinion(store, name, opinionId, text));
  }

  function edit(opinionId: string, text: string) {
    const name = app;

    return run('edit', () => editOpinion(store, name, opinionId, text));
  }

  function withdraw(opinionId: string) {
    const name = app;

    return run('withdraw', () => withdrawOpinion(store, name, opinionId));
  }

  function deadline(minutes: number) {
    const name = app;

    // 0 converges now: like opening with "now", it may run the AI.
    return run(minutes === 0 ? 'converge' : 'deadline', () => setRoundDeadline(store, name, minutes, minutes === 0 ? hooks.aiTimeoutMs() : undefined));
  }

  function comment(step: number, text: string) {
    const name = app;

    return run('comment', () => commentOnPlan(store, name, step, text));
  }

  function supportComment(commentId: string) {
    const name = app;

    return run('support', () => supportPlanComment(store, name, commentId));
  }

  // Not through run(): saving drafts must not block the page or show a
  // spinner; a refused save is shown like other errors.
  async function saveDrafts(items: OpinionItem[], shared: boolean): Promise<boolean> {
    const name = app;

    if (!name || !round.value || round.value.state !== 'collecting') {
      return false;
    }
    try {
      take(await saveRoundDrafts(store, name, items, shared), name);

      return true;
    } catch (e) {
      error.value = hooks.messageFor(e);

      return false;
    }
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
    round, error, notice, pending, elapsed, active, limits, available, refresh, open, act, add, support, reply,
    edit, withdraw, deadline, comment, supportComment, saveDrafts, start, stop
  };
}
