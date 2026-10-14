// Per-app edit lock (phase-1 "同一 Project 編輯鎖"; M0 locks one app).
//
// The detail page takes the lock when it opens, renews it while open and
// gives it back when it closes. The backend lets a lock lapse after
// LOCK_TTL (3 min) without renewal, so a closed laptop or crashed tab
// frees the app on its own. Renewal stops when the user has been idle for
// IDLE_MS, so an open but unused page does not keep others out.
//
// Every change to the app (plan with baseApp, apply, restore, saving code
// or layout) sends the token; without it the backend answers
// "locked"/"lock_required".
//
// The holder name is the Rancher user shown to others. M0's backend cannot
// verify it (see docs/m0-design.md); phase 1 takes it from the session.
import {
  ref, computed, onBeforeUnmount, Ref, ComputedRef
} from 'vue';
import {
  acquireLock, renewLock, releaseLock, RequestError, StoreLike
} from '../utils/backend';

export const RENEW_MS = 60 * 1000;
export const IDLE_MS = 3 * 60 * 1000;

export type LockState = 'none' | 'acquiring' | 'held' | 'other' | 'lost' | 'error';

export interface EditLock {
  state: Ref<LockState>;
  // The backend's message naming who holds it, when someone else does.
  otherMessage: Ref<string>;
  holder: ComputedRef<string>;
  token: () => string | null;
  canEdit: ComputedRef<boolean>;
  acquire: (app: string) => Promise<void>;
  release: () => void;
  // Called when the backend refused a change for lock reasons.
  noteError: (e: unknown) => void;
}

const ACTIVITY_EVENTS = ['pointerdown', 'keydown', 'wheel'] as const;

/** The logged-in Rancher user's name, for display to other editors. */
export function rancherUserName(store: StoreLike): string {
  const g = store?.getters || {};
  const user = (g['auth/user'] || {}) as Record<string, unknown>;

  for (const v of [user.username, user.displayName, user.name, g['auth/principalId']]) {
    if (typeof v === 'string' && v.trim()) {
      return v.trim().slice(0, 100);
    }
  }

  return 'rancher-user';
}

export function useEditLock(store: StoreLike): EditLock {
  const state = ref<LockState>('none');
  const otherMessage = ref('');
  const holder = computed(() => rancherUserName(store));
  let app = '';
  let token: string | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;
  let lastActivity = Date.now();

  const canEdit = computed(() => state.value === 'held');

  function touch() {
    lastActivity = Date.now();
  }

  function stopTimer() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  function lose(next: LockState) {
    stopTimer();
    token = null;
    state.value = next;
  }

  function noteError(e: unknown) {
    if (!(e instanceof RequestError)) {
      return;
    }
    if (e.code === 'locked') {
      otherMessage.value = e.userMessage || '';
      lose('other');
    } else if (e.code === 'lock_required') {
      lose('lost');
    }
  }

  async function renew() {
    if (!token || !app) {
      return;
    }
    // Idle: let the lock lapse so others can edit.
    if (Date.now() - lastActivity > IDLE_MS) {
      lose('lost');

      return;
    }
    try {
      await renewLock(store, app, token);
    } catch (e) {
      if (e instanceof RequestError && (e.code === 'locked' || e.code === 'lock_required')) {
        lose('lost');
      }
      // Network blips: keep trying on the next tick.
    }
  }

  async function acquire(name: string) {
    if (app && app !== name) {
      release();
    }
    app = name;
    touch();
    state.value = 'acquiring';
    otherMessage.value = '';
    try {
      const grant = await acquireLock(store, name, holder.value);

      if (app !== name) {
        releaseLock(store, name, grant.token).catch(() => {});

        return;
      }
      token = grant.token;
      state.value = 'held';
      stopTimer();
      timer = setInterval(renew, RENEW_MS);
    } catch (e) {
      token = null;
      if (e instanceof RequestError && e.code === 'locked') {
        state.value = 'other';
        otherMessage.value = e.userMessage || '';
      } else {
        state.value = 'error';
      }
    }
  }

  function release() {
    stopTimer();
    if (token && app) {
      releaseLock(store, app, token).catch(() => {});
    }
    token = null;
    state.value = 'none';
  }

  function onUnload() {
    release();
  }

  ACTIVITY_EVENTS.forEach((ev) => window.addEventListener(ev, touch, { passive: true }));
  window.addEventListener('pagehide', onUnload);

  onBeforeUnmount(() => {
    ACTIVITY_EVENTS.forEach((ev) => window.removeEventListener(ev, touch));
    window.removeEventListener('pagehide', onUnload);
    release();
  });

  return {
    state,
    otherMessage,
    holder,
    token:  () => token,
    canEdit,
    acquire,
    release,
    noteError,
  };
}
