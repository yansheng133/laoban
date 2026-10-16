/**
 * @jest-environment jsdom
 */
// Mount every page's setup() in a real Vue instance. `yarn build-pkg` does
// not run setup(), so an error there (v0.1.12: a watch reading `round`
// before its declaration, so the detail page stayed blank) only showed in
// the browser. Here the page's <script> is compiled from the .vue file as
// the build does, its own composables and utils are the real ones, and
// only Rancher, the router, the backend's network and child components
// are stubbed.
import {
  describe, it, expect, beforeAll, afterAll, jest
} from '@jest/globals';
import { readFileSync } from 'fs';
import { dirname, join, resolve } from 'path';
import { createApp, h, reactive } from 'vue';

// eslint-disable-next-line @typescript-eslint/no-var-requires
const { parse, compileScript } = require('@vue/compiler-sfc');
// eslint-disable-next-line @typescript-eslint/no-var-requires
const babel = require('@babel/core');

const PAGES = join(__dirname, '..');

const store = {
  getters: new Proxy({} as Record<string, unknown>, {
    get: (_t, k) => {
      if (k === 'i18n/t') {
        return (key: string) => key;
      }
      if (k === 'prefs/get') {
        return () => undefined;
      }

      return undefined;
    },
  }),
  dispatch: () => Promise.resolve(),
  commit:   () => undefined,
  state:    {},
};

// The same stubs for modules the pages' composables import.
jest.mock('@shell/composables/useI18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }), { virtual: true });

const STUBS: Record<string, unknown> = {
  vuex:                          { useStore: () => store },
  'vue-router':                  { useRoute: () => ({ params: { name: 'todo' }, query: {} }), useRouter: () => ({ push: () => undefined }) },
  '@shell/composables/useI18n':  { useI18n: () => ({ t: (key: string) => key }) },
};

// Compile a .vue file's <script> to CommonJS, the way the build sees it.
function loadPage(file: string): { setup: (props: unknown, ctx: unknown) => unknown } {
  const src = readFileSync(file, 'utf8');
  const { descriptor } = parse(src, { filename: file });
  const script = compileScript(descriptor, { id: 'test', isProd: false });
  const out = babel.transformSync(script.content, {
    filename:   `${ file }.ts`,
    configFile: false,
    babelrc:    false,
    presets:    [['@babel/preset-env', { targets: { node: 'current' } }], '@babel/preset-typescript'],
  });
  const mod = { exports: {} as Record<string, unknown> };
  const req = (id: string) => {
    if (id in STUBS) {
      return STUBS[id];
    }
    if (id.endsWith('.vue')) {
      return { default: { name: 'Stub', render: () => null } };
    }
    if (id.startsWith('.')) {
      return require(resolve(dirname(file), id));
    }

    return require(id);
  };

  // eslint-disable-next-line no-new-func
  new Function('require', 'module', 'exports', out.code)(req, mod, mod.exports);

  return (mod.exports.default || mod.exports) as { setup: (props: unknown, ctx: unknown) => unknown };
}

describe('pages mount without errors in setup()', () => {
  const realFetch = global.fetch;

  beforeAll(() => {
    // The backend is not there: every request fails, as when it is down.
    global.fetch = jest.fn(() => Promise.reject(new Error('offline'))) as unknown as typeof fetch;
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
  });
  afterAll(() => {
    global.fetch = realFetch;
  });

  for (const page of ['AppDetail.vue', 'Apps.vue', 'Dashboard.vue']) {
    it(page, async() => {
      const def = loadPage(join(PAGES, page));
      const errors: unknown[] = [];
      let bindings: Record<string, unknown> | null = null;
      const app = createApp({
        setup(props: unknown, ctx: unknown) {
          bindings = def.setup(props, ctx) as Record<string, unknown>;

          return () => h('div');
        },
      });

      app.config.errorHandler = (e) => {
        errors.push(e);
      };
      const el = document.createElement('div');

      app.mount(el);
      await new Promise((r) => setTimeout(r, 20));
      app.unmount();

      expect(errors).toEqual([]);
      expect(bindings).not.toBeNull();
      expect(typeof bindings).toBe('object');
    });
  }
});

describe('the round panel checks the limits before sending', () => {
  beforeAll(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
  });

  function panel(over: Record<string, unknown>) {
    const def = loadPage(join(PAGES, '../components/RoundPanel.vue'));
    const props = reactive({
      round:          null,
      draft:          '',
      pending:        null,
      elapsed:        0,
      error:          '',
      disabled:       false,
      drafts:         [],
      removedDraft:   null,
      selectedTarget: null,
      notice:         '',
      limits:         {
        opinionChars: 1000, replyChars: 300, roundChars: 8000, opinions: 30, replies: 20, batch: 10
      },
      available: true,
      ...over,
    });
    let b: Record<string, { value: unknown }> = {};
    const app = createApp({
      setup() {
        b = def.setup(props, { emit: () => undefined }) as Record<string, { value: unknown }>;

        return () => h('div');
      },
    });

    app.mount(document.createElement('div'));

    return { b, props, app };
  }

  it('a pasted long opinion is kept as typed and cannot be sent', () => {
    const pasted = '廢'.repeat(1001);
    const { b, props, app } = panel({ draft: pasted });

    expect(props.draft).toBe(pasted);
    expect((b.draftState.value as { over: boolean }).over).toBe(true);
    expect(b.canOpen.value).toBe(false);
    props.draft = '廢'.repeat(1000);
    expect(b.canOpen.value).toBe(true);
    app.unmount();
  });

  it('a full round takes no more opinions; without the CRD nothing opens', () => {
    const r = {
      id: 'r1', app: 'todo', initiator: 'u-a', scope: ['web'], state: 'collecting', deadline: 0, opinions: [], confirmers: [], plan: null, message: '', result: null, log: [], you: {
        user: 'u-a', isInitiator: true, mustConfirm: false, canAddOpinion: true, canDiscuss: true
      },
      used: { chars: 7990, opinions: 9 },
    };
    const { b, props, app } = panel({ round: r, draft: 'x'.repeat(10) });

    expect(b.canAdd.value).toBe(true);
    props.draft = 'x'.repeat(11);
    expect(b.canAdd.value).toBe(false);
    expect((b.space.value as { over: boolean; shown: boolean }).over).toBe(true);
    app.unmount();

    const closed = panel({ draft: 'x', available: false });

    expect(closed.b.canOpen.value).toBe(false);
    closed.app.unmount();
  });
});
