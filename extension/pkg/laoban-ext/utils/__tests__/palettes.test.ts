// The 3D palettes (styles/palettes.scss): read the real stylesheet and check
// that each palette's select colour stays apart from the version comparison
// marks and from its own layer colours, in light and dark.
import { describe, it, expect } from '@jest/globals';
import { readFileSync } from 'fs';
import { join } from 'path';
import { PALETTES } from '../../config/appearance';

const scss = readFileSync(join(__dirname, '../../styles/palettes.scss'), 'utf8');

// The body of the first rule whose selector list starts with `selector`.
function block(selector: string): string {
  const i = scss.indexOf(`${ selector }`);

  expect(i).toBeGreaterThanOrEqual(0);
  const open = scss.indexOf('{', i);

  return scss.slice(open + 1, scss.indexOf('}', open));
}

function layers(selector: string) {
  const m = /@include layers\(([^)]*)\)/.exec(block(selector));

  expect(m).not.toBeNull();
  const [network, code, data, edge, select] = m![1].split(',').map((x) => x.trim());

  return {
    network, code, data, edge, select
  };
}

function diff(selector: string): string[] {
  const b = block(selector);

  return ['added', 'removed', 'changed'].map((k) => new RegExp(`--laoban-diff-${ k }: (#[0-9a-f]{6})`).exec(b)![1]);
}

function hsl(hex: string): { h: number; s: number } {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  const l = (max + min) / 2;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  let h = 0;

  if (d) {
    h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  }

  return { h: (h * 60 + 360) % 360, s };
}

function hueGap(a: string, b: string): number {
  const d = Math.abs(hsl(a).h - hsl(b).h);

  return Math.min(d, 360 - d);
}

const selectorFor = (p: string, dark: boolean) => `${ dark ? '.theme-dark ' : '' }${ p === 'rancher' ? '.laoban-arch,' : `.laoban-arch[data-laoban-palette="${ p }"],` }`;

describe('3D palettes', () => {
  for (const dark of [false, true]) {
    const marks = diff(dark ? '.theme-dark .laoban-m0 {' : '.laoban-m0 {');

    for (const p of PALETTES) {
      it(`${ p } (${ dark ? 'dark' : 'light' }): the select colour is not a comparison mark or a layer colour`, () => {
        const c = layers(selectorFor(p, dark));

        for (const m of marks) {
          expect(hueGap(c.select, m)).toBeGreaterThanOrEqual(40);
        }
        // Greys (code layer, edges) have no hue to clash with.
        for (const l of [c.network, c.code, c.data].filter((x) => hsl(x).s >= 0.3)) {
          expect(hueGap(c.select, l)).toBeGreaterThanOrEqual(30);
        }
        expect(hsl(c.select).s).toBeGreaterThanOrEqual(0.5);
      });
    }
  }

  it('the palettes no longer change the page (Rancher primary colours)', () => {
    expect(scss).not.toMatch(/--primary\s*:/);
  });
});
