// Who someone is, for display. The backend reports users as the API server
// verified them: for Rancher users the Rancher user ID ("u-abc12").
import type { StoreLike } from './backend';

/**
 * A user ID shown as the user's display name when the Rancher store has
 * that user, else the ID itself. An unknown user is asked for once in the
 * background; the label updates when the store has it (report 26, N41).
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
  loadUsers(store, [id]);

  return id;
}

const USER_TYPE = 'management.cattle.io.user';
// IDs already asked for (found or refused): asked once per page load.
const asked = new Set<string>();

/**
 * Ask the Rancher store for these users, so userLabel can show their names
 * (report 26, N41: versions showed "u-pc66f"). Users the viewer may not
 * read stay IDs; failures are ignored.
 */
export async function loadUsers(store: StoreLike, ids: Iterable<string>): Promise<void> {
  const dispatch = (store as { dispatch?: (a: string, p: unknown) => Promise<unknown> })?.dispatch;

  if (typeof dispatch !== 'function') {
    return;
  }
  const todo = [...new Set(ids)].filter((id) => /^[a-z0-9][a-z0-9-]{1,62}$/.test(id) && !asked.has(id));

  await Promise.all(todo.map(async(id) => {
    asked.add(id);
    try {
      await dispatch('management/find', { type: USER_TYPE, id });
    } catch {
      // Not allowed or gone: the ID is shown.
    }
  }));
}

/**
 * Replace "(id)" / "（id）" in a version's request text ("(u-a) add a
 * filter; (u-b) …") with the user's name where the store knows it.
 */
export function withUserNames(store: StoreLike, text: string): string {
  return text.replace(/([(（])([a-z0-9][a-z0-9-]{1,62})([)）])/g, (all, open: string, id: string, close: string) => {
    const name = userLabel(store, id);

    return name === id ? all : `${ open }${ name }${ close }`;
  });
}
