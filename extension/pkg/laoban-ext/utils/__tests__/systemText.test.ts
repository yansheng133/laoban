// Fixed texts laoban writes are shown in the page's language.
import { describe, it, expect } from '@jest/globals';
import { readFileSync } from 'fs';
import { join } from 'path';
import { systemText, SYSTEM_TEXTS } from '../systemText';

// eslint-disable-next-line @typescript-eslint/no-var-requires
const yaml: { load(text: string): any } = require('js-yaml');

const t = (key: string) => `<${ key }>`;

describe('systemText', () => {
  it('words known fixed texts in the page language and leaves the rest', () => {
    expect(systemText(t, 'Rearranged the 3D layout')).toBe('<laoban.versions.notes.layout>');
    expect(systemText(t, '調整 3D 版面')).toBe('<laoban.versions.notes.layout>');
    expect(systemText(t, 'memory')).toBe('<laoban.view3d.edges.memory>');
    expect(systemText(t, 'Make the title bigger')).toBe('Make the title bigger');
  });

  it("knows the editor's default note in every language", () => {
    for (const l of ['en-us', 'zh-hant', 'ja']) {
      const note = yaml.load(readFileSync(join(__dirname, '../../l10n', `${ l }.yaml`), 'utf8')).laoban.editor.defaultNote;

      expect(SYSTEM_TEXTS[note]).toBe('laoban.editor.defaultNote');
    }
  });
});
