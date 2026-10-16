// How many rounds wait for the caller to confirm, for the count on the
// "Apps" entry of the side nav (v0.1.16). A module-level ref: the nav's
// count getter reads it, the pages that ask the backend update it, and a
// slow background read keeps it fresh while Rancher is open.
import { ref } from 'vue';
import { getWaitingRounds, StoreLike, WaitingRound } from './backend';

export const waitingToConfirm = ref(0);

export const WAITING_POLL_MS = 60 * 1000;

let timer: ReturnType<typeof setInterval> | null = null;

/** Count what is the caller's to confirm. */
export function confirmCount(rounds: WaitingRound[]): number {
  return rounds.filter((w) => w.reason === 'confirm').length;
}

// The last answer, kept when a read fails (the badges are a hint).
let last: WaitingRound[] = [];

export async function refreshWaiting(store: StoreLike): Promise<WaitingRound[]> {
  try {
    const res = await getWaitingRounds(store);
    const rounds = res.rounds || [];

    waitingToConfirm.value = confirmCount(rounds);
    last = rounds;

    return rounds;
  } catch {
    // The count is a hint; keep the last one.
    return last;
  }
}

/** Start the background read once (called by the nav's count getter). */
export function ensureWaitingPoll(store: StoreLike) {
  if (timer || typeof window === 'undefined') {
    return;
  }
  refreshWaiting(store);
  timer = setInterval(() => {
    if (document.visibilityState !== 'hidden') {
      refreshWaiting(store);
    }
  }, WAITING_POLL_MS);
}
