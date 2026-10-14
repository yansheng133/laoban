// Every laoban.* key used in the code exists in all three languages, and
// the languages have the same keys.
import { describe, it, expect } from '@jest/globals';
import { readFileSync, readdirSync } from 'fs';
import { join } from 'path';

// js-yaml comes with @rancher/shell, without type declarations.
// eslint-disable-next-line @typescript-eslint/no-var-requires
const yaml: { load(text: string): unknown } = require('js-yaml');

const root = join(__dirname, '../..');
const LOCALES = ['zh-hant', 'en-us', 'ja'];

function keys(o: unknown, prefix = ''): string[] {
  if (!o || typeof o !== 'object') {
    return [prefix];
  }

  return Object.entries(o as Record<string, unknown>).flatMap(([k, v]) => keys(v, prefix ? `${ prefix }.${ k }` : k));
}

const tables = Object.fromEntries(LOCALES.map((l) => [l, new Set(keys(yaml.load(readFileSync(join(root, 'l10n', `${ l }.yaml`), 'utf8'))))]));

function sources(dir: string): string[] {
  return readdirSync(join(root, dir)).filter((f) => /\.(vue|ts)$/.test(f)).map((f) => readFileSync(join(root, dir, f), 'utf8'));
}

describe('l10n', () => {
  it('has the same keys in every language', () => {
    for (const l of LOCALES) {
      expect([...tables[l]].filter((k) => !tables['en-us'].has(k))).toEqual([]);
      expect([...tables['en-us']].filter((k) => !tables[l].has(k))).toEqual([]);
    }
  });

  it('has every fixed key the pages and components use', () => {
    const used = new Set<string>();

    for (const src of [...sources('pages'), ...sources('components'), ...sources('composables')]) {
      for (const m of src.matchAll(/t\('(laoban\.[a-zA-Z0-9_.]+)'/g)) {
        used.add(m[1]);
      }
    }
    const missing = [...used].filter((k) => !tables['en-us'].has(k) && ![...tables['en-us']].some((x) => x.startsWith(`${ k }.`)));

    expect(missing).toEqual([]);
    expect(used.size).toBeGreaterThan(50);
  });
});
