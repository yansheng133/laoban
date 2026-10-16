// The signed-in Rancher user's ID ("user-abc12"), as laoban's backend
// records people. Rancher 2.15's `auth/v3User` getter can be empty
// (report 22, B19), so the user is read once from /v3/users?me=true.
import type { StoreLike } from './backend';

let mine: Promise<string> | null = null;

export function currentUserId(store: StoreLike): Promise<string> {
  try {
    const u = (store?.getters?.['auth/v3User'] || {}) as { id?: unknown };

    if (typeof u.id === 'string' && u.id) {
      return Promise.resolve(u.id);
    }
  } catch {}
  if (!mine) {
    mine = (async() => {
      try {
        const res = await fetch('/v3/users?me=true', { headers: { Accept: 'application/json' }, credentials: 'same-origin' });
        const body = res.ok ? await res.json() as { data?: { id?: unknown }[] } : null;
        const id = body?.data?.[0]?.id;

        if (typeof id === 'string' && id) {
          return id;
        }
        // Not known this time: ask again next time.
        mine = null;

        return '';
      } catch {
        mine = null;

        return '';
      }
    })();
  }

  return mine;
}
