import { describe, it, expect } from '@jest/globals';
import { userLabel, withUserNames } from '../users';

function storeWith(users: Record<string, { displayName?: string; username?: string }>) {
  const asked: string[] = [];
  const store = {
    getters:  { 'management/byId': (_type: string, id: string) => users[id] },
    dispatch: (_a: string, p: { id: string }) => {
      asked.push(p.id);

      return Promise.reject(new Error('forbidden'));
    },
  };

  return { store, asked };
}

describe('userLabel', () => {
  it('shows the display name, else the ID, and asks for unknown users once', () => {
    const { store, asked } = storeWith({ 'u-a': { displayName: 'Alice' } });

    expect(userLabel(store as never, 'u-a')).toBe('Alice');
    expect(userLabel(store as never, 'u-zz1')).toBe('u-zz1');
    expect(userLabel(store as never, 'u-zz1')).toBe('u-zz1');
    expect(asked).toEqual(['u-zz1']);
  });
});

describe('withUserNames', () => {
  it('replaces known IDs in brackets only', () => {
    const { store } = storeWith({ 'u-a': { displayName: 'Alice' }, 'm-vvk9f': { username: 'admin' } });

    expect(withUserNames(store as never, '(u-a) add a filter; (u-b) sort; (m-vvk9f) x'))
      .toBe('(Alice) add a filter; (u-b) sort; (admin) x');
    expect(withUserNames(store as never, '（u-a）加篩選；［u-a］不換')).toBe('（Alice）加篩選；［u-a］不換');
  });
});
