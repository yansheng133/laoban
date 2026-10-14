// Per-app edit lock (phase-1 "同一 Project 編輯鎖"; M0 locks one app).
//
// The detail page takes the lock when it opens, renews it while open and
// gives it back when it closes. The backend lets a lock lapse after
// LOCK_TTL (3 min) without renewal, so a closed laptop or crashed tab
// frees the app on its own. Renewal stops when the user has been idle for
// IDLE_MS, so an open but unused page does not keep others out.
//
// The holder is the signed-in user as verified by the Kubernetes API server
// (the backend is an aggregated API, docs/m0-design.md), so every change to
// the app (plan with baseApp, apply, restore, saving code or layout) is
// checked against the caller; there is no token to carry.
import {
  ref, computed, onBeforeUnmount, Ref, ComputedRef
} from 'vue';
import {
  acquireLock, renewLock, releaseLock, getApp, RequestError, StoreLike
} from '../utils/backend';

export const RENEW_MS = 60 * 1000;
export const IDLE_MS = 3 * 60 * 1000;

export type LockState = 'none' | 'acquiring' | 'held' | 'other' | 'lost' | 'error';

export interface EditLock {
  state: Ref<LockState>;
  // The backend's message naming who holds it, when someone else does.
  otherMessage: Ref<string>;
  // Who holds it when someone else does (display name when known).
  otherHolder: Ref<string>;
  holder: ComputedRef<string>;
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

/**
 * A Rancher user ID (as the API server reports it, e.g. "u-abc12") shown as
 * the user's display name when the store has that user, else the ID.
 */
export function userLabel(store: StoreLike, id: string): string {
  try {
    const byId = store?.getters?.['management/byId'] as ((type: string, id: string) => Record<string, unknown> | undefined) | undefined;
    const u = byId?.('management.cattle.io.user', id);

    for (const v of [u?.displayName, u?.username]) {
      if (typeof v === 'string' && v.trim()) {
        return v.trim().slice(0, 100);
      }
    }
  } catch {
    // Not loaded or not allowed to see users: show the ID.
  }

  return id;
}

export function useEditLock(store: StoreLike): EditLock {
  const state = ref<LockState>('none');
  const otherMessage = ref('');
  const otherHolder = ref('');
  const holder = computed(() => rancherUserName(store));
  let app = '';
  let held = false;
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
    held = false;
    state.value = next;
  }

  // Who holds it, read from the app (the error message has the raw ID).
  async function readHolder(name: string) {
    try {
      const lock = (await getApp(store, name)).lock;

      otherHolder.value = lock?.holder ? userLabel(store, lock.holder) : '';
    } catch {
      otherHolder.value = '';
    }
  }

  function noteError(e: unknown) {
    if (!(e instanceof RequestError)) {
      return;
    }
    if (e.code === 'locked') {
      otherMessage.value = e.userMessage || '';
      lose('other');
      if (app) {
        readHolder(app);
      }
    } else if (e.code === 'lock_required') {
      lose('lost');
    }
  }

  async function renew() {
    if (!held || !app) {
      return;
    }
    // Idle: let the lock lapse so others can edit.
    if (Date.now() - lastActivity > IDLE_MS) {
      lose('lost');

      return;
    }
    try {
      await renewLock(store, app);
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
    otherHolder.value = '';
    try {
      await acquireLock(store, name);

      if (app !== name) {
        releaseLock(store, name).catch(() => {});

        return;
      }
      held = true;
      state.value = 'held';
      stopTimer();
      timer = setInterval(renew, RENEW_MS);
    } catch (e) {
      held = false;
      if (e instanceof RequestError && e.code === 'locked') {
        state.value = 'other';
        otherMessage.value = e.userMessage || '';
        readHolder(name);
      } else {
        state.value = 'error';
      }
    }
  }

  function release() {
    stopTimer();
    if (held && app) {
      releaseLock(store, app).catch(() => {});
    }
    held = false;
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
    otherHolder,
    holder,
    canEdit,
    acquire,
    release,
    noteError,
  };
}
