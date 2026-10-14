// Structure changes made in the 3D view, turned into change-request
// sentences for "要怎麼修改" (components/StructureEdit.vue). Pure functions
// so they can be unit tested.

export type StructureAction =
  | { type: 'rename'; from: string; to: string }
  | { type: 'delete'; name: string }
  | { type: 'connect'; from: string; to: string }
  | { type: 'add'; part: 'web' | 'api'; name: string; purpose: string };

type Translate = (key: string, args?: Record<string, unknown>) => string;

const NAME = /^[\p{L}\p{N}][\p{L}\p{N}_-]{0,39}$/u;

/** A module name as typed, or '' if it is not one (letters, digits, - and _). */
export function moduleName(input: string): string {
  const s = (input || '').trim();

  return NAME.test(s) ? s : '';
}

/** One sentence for the change request; '' for an invalid action. */
export function structureSentence(a: StructureAction, t: Translate): string {
  switch (a.type) {
  case 'rename':
    return moduleName(a.to) && a.from ? t('laoban.structure.lines.rename', { from: a.from, to: a.to }) : '';
  case 'delete':
    return a.name ? t('laoban.structure.lines.delete', { name: a.name }) : '';
  case 'connect':
    return a.from && a.to && a.from !== a.to ? t('laoban.structure.lines.connect', { from: a.from, to: a.to }) : '';
  case 'add': {
    if (!moduleName(a.name)) {
      return '';
    }
    const part = t(`laoban.structure.parts.${ a.part }`);

    return a.purpose ? t('laoban.structure.lines.addWithPurpose', {
      part, name: a.name, purpose: a.purpose
    }) : t('laoban.structure.lines.add', { part, name: a.name });
  }
  default:
    return '';
  }
}

/** Add a sentence to the prompt on its own line, without duplicates. */
export function appendLine(prompt: string, line: string, max = 4000): string {
  const current = prompt.replace(/\s+$/, '');
  const lines = current ? current.split('\n') : [];

  if (!line || lines.includes(line)) {
    return prompt;
  }
  const next = current ? `${ current }\n${ line }` : line;

  return next.length > max ? prompt : next;
}
