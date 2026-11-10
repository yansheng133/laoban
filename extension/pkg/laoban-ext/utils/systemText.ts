// Fixed texts that laoban itself writes into history or the architecture
// graph, in whatever language was in use then (the backend's install
// language, or the page language of whoever saved). Shown in the page's
// language instead; anything a person wrote is shown as it is.
// (i18n review 2026-11: a Japanese page showed "Rearranged the 3D layout".)

type Translate = (key: string, args?: Record<string, unknown>) => string;

const KNOWN: Record<string, string> = {
  // Backend, layout saves (service.rs, tr()).
  'Rearranged the 3D layout':   'laoban.versions.notes.layout',
  '調整 3D 版面':                     'laoban.versions.notes.layout',
  // The code editor's default note (laoban.editor.defaultNote) in each language.
  'Manual code edit':           'laoban.editor.defaultNote',
  '手動修改程式':                     'laoban.editor.defaultNote',
  'コードの手動修正':                   'laoban.editor.defaultNote',
  // Edge label of the generated architecture (plan.rs).
  memory:                       'laoban.view3d.edges.memory',
};

export function systemText(t: Translate, text: string): string {
  const key = KNOWN[text.trim()];

  return key ? t(key) : text;
}

export const SYSTEM_TEXTS = KNOWN;
