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

function flat(o: unknown, prefix = ''): [string, string][] {
  if (!o || typeof o !== 'object') {
    return [[prefix, String(o)]];
  }

  return Object.entries(o as Record<string, unknown>).flatMap(([k, v]) => flat(v, prefix ? `${ prefix }.${ k }` : k));
}

const texts = Object.fromEntries(LOCALES.map((l) => [l, flat(yaml.load(readFileSync(join(root, 'l10n', `${ l }.yaml`), 'utf8')))]));

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

  it('has a text for every error code the page words itself', () => {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const { TRANSLATED_ERRORS } = require('../errors');

    for (const l of LOCALES) {
      expect((TRANSLATED_ERRORS as string[]).filter((c) => !tables[l].has(`laoban.errors.${ c }`))).toEqual([]);
    }
  });

  it('has the keys the subagent cards build at run time', () => {
    for (const k of ['part.web', 'part.api', 'state.running', 'state.done', 'state.failed', 'state.skipped', 'gap.apiMissing', 'gap.unknownCall']) {
      expect(tables['en-us'].has(`laoban.subagents.${ k }`)).toBe(true);
    }
  });

  // i18n review 2026-11: one word per idea in each language.
  it('uses the agreed words', () => {
    const bad = (l: string, re: RegExp) => texts[l].filter(([, v]) => re.test(v)).map(([k]) => k);

    // English: the README says consolidate, so the page does too.
    expect(bad('en-us', /converg/i)).toEqual([]);
    // Japanese, as in README.ja.md: 取りまとめ (not 収束 or 集約), バージョン
    // (not 版, except 試食版), アーキテクチャ図 (not 構成図).
    expect(bad('ja', /収束|集約|構成図|プレビュー版/)).toEqual([]);
    expect(bad('ja', /(?<!試食)版/)).toEqual([]);
    // Taiwan wording: 開啟 (not 打開), 有異議 for objecting.
    expect(bad('zh-hant', /打開|(?<!所)有意見/)).toEqual([]);
  });

  it('has no punctuation of one language fixed in the templates', () => {
    for (const src of [...sources('pages'), ...sources('components')]) {
      const template = (src.match(/<template>([\s\S]*)<\/template>/) || ['', ''])[1];

      // Full-width brackets, colons and the Chinese list comma come from
      // laoban.common.* instead, so English reads "Steps (3)".
      expect(template.match(/[（）：]|'、'/g)).toBeNull();
    }
  });
});
