// Who someone is, for display. The backend reports users as the API server
// verified them: for Rancher users the Rancher user ID ("u-abc12").
import type { StoreLike } from './backend';

/**
 * A user ID shown as the user's display name when the Rancher store has
 * that user, else the ID itself.
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
