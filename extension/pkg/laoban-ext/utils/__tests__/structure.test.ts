import { describe, it, expect } from '@jest/globals';
import { moduleName, structureSentence, appendLine } from '../structure';

const t = (key: string, args: Record<string, unknown> = {}) => `${ key }(${ Object.entries(args).map(([k, v]) => `${ k }=${ v }`).join(',') })`;

describe('moduleName', () => {
  it('accepts letters, digits, - and _ including CJK', () => {
    expect(moduleName(' drinks ')).toBe('drinks');
    expect(moduleName('menu-view_2')).toBe('menu-view_2');
    expect(moduleName('飲料')).toBe('飲料');
  });

  it('refuses empty, spaces, markup and long names', () => {
    expect(moduleName('')).toBe('');
    expect(moduleName('a b')).toBe('');
    expect(moduleName('<script>')).toBe('');
    expect(moduleName('-x')).toBe('');
    expect(moduleName('a'.repeat(41))).toBe('');
  });
});

describe('structureSentence', () => {
  it('builds one sentence per action', () => {
    expect(structureSentence({ type: 'rename', from: 'a', to: 'b' }, t)).toBe('laoban.structure.lines.rename(from=a,to=b)');
    expect(structureSentence({ type: 'delete', name: 'a' }, t)).toBe('laoban.structure.lines.delete(name=a)');
    expect(structureSentence({ type: 'connect', from: 'a', to: 'b' }, t)).toBe('laoban.structure.lines.connect(from=a,to=b)');
    expect(structureSentence({
      type: 'add', part: 'api', name: 'drinks', purpose: ''
    }, t)).toBe('laoban.structure.lines.add(part=laoban.structure.parts.api(),name=drinks)');
    expect(structureSentence({
      type: 'add', part: 'web', name: 'drinks', purpose: '飲料'
    }, t)).toContain('purpose=飲料');
  });

  it('refuses invalid actions', () => {
    expect(structureSentence({ type: 'rename', from: 'a', to: 'b c' }, t)).toBe('');
    expect(structureSentence({ type: 'connect', from: 'a', to: 'a' }, t)).toBe('');
    expect(structureSentence({
      type: 'add', part: 'web', name: '', purpose: ''
    }, t)).toBe('');
  });
});

describe('appendLine', () => {
  it('adds on its own line without duplicates', () => {
    expect(appendLine('', 'A')).toBe('A');
    expect(appendLine('x\n', 'A')).toBe('x\nA');
    expect(appendLine('x\nA', 'A')).toBe('x\nA');
  });

  it('keeps the prompt when the limit would be exceeded', () => {
    expect(appendLine('abc', 'defg', 6)).toBe('abc');
  });
});
