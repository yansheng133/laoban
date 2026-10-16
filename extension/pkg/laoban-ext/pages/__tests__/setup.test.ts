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

describe('the round panel in the page\'s language', () => {
  beforeAll(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
  });

  it('shows a known failure by its code, an unknown one as sent', () => {
    const def = loadPage(join(PAGES, '../components/RoundPanel.vue'));
    const base = {
      id: 'r1', app: 'todo', initiator: 'u-a', scope: ['web'], state: 'failed', deadline: 0, opinions: [], confirmers: [], plan: null, result: null, you: {
        user: 'u-a', isInitiator: true, mustConfirm: false, canAddOpinion: false
      },
    };
    const props = reactive({
      round: {
        ...base,
        message: '後端在發布途中重新啟動…',
        messageCode: 'restartUnpublished',
        log: [{
          at: '', by: 'laoban', event: '失敗：…', code: 'failed', args: { message: '後端在發布途中重新啟動…', code: 'restartUnpublished' }
        }],
      } as Record<string, unknown>,
      draft: '', pending: null, elapsed: 0, error: '', disabled: false, drafts: [], removedDraft: null, selectedTarget: null, notice: '',
      limits: {
        opinionChars: 1000, replyChars: 300, roundChars: 8000, opinions: 30, replies: 20, batch: 10
      },
      available: true,
    });
    let b: Record<string, unknown> = {};
    const app = createApp({
      setup() {
        b = def.setup(props, { emit: () => undefined }) as Record<string, unknown>;

        return () => h('div');
      },
    });

    app.mount(document.createElement('div'));
    // The test store's t() returns the key.
    expect((b.failureText as { value: string }).value).toBe('laoban.round.failures.restartUnpublished');
    const logText = b.logText as (l: unknown) => string;

    expect(logText((props.round as { log: unknown[] }).log[0])).toBe('laoban.round.events.failed');
    props.round = { ...base, message: '別的原因', log: [] };
    expect((b.failureText as { value: string }).value).toBe('別的原因');
    app.unmount();
  });
});

describe('v0.1.16 pieces mount and act', () => {
  beforeAll(() => {
    jest.spyOn(console, 'warn').mockImplementation(() => undefined);
  });

  function mount(file: string, props: Record<string, unknown>, emitted: unknown[][] = []) {
    const def = loadPage(join(PAGES, file));
    const p = reactive(props);
    let b: Record<string, unknown> = {};
    const app = createApp({
      setup() {
        b = def.setup(p, { emit: (...a: unknown[]) => emitted.push(a), expose: () => undefined }) as Record<string, unknown>;

        return () => h('div');
      },
    });

    app.mount(document.createElement('div'));

    return { b, p, app };
  }

  const LIMITS = {
    opinionChars: 1000, replyChars: 300, roundChars: 8000, opinions: 30, replies: 20, batch: 10
  };

  it('an author edits within the limits and withdraws with a second press', () => {
    const emitted: unknown[][] = [];
    const o = {
      id: 'o2', by: 'u-b', at: '', text: '舊的', supporters: [], replies: []
    };
    const { b, app } = mount('../components/OpinionList.vue', {
      opinions: [o], me: 'u-b', canDiscuss: true, canEdit: true, busy: false, selected: null, limits: LIMITS, roundLeft: 10
    }, emitted);
    const startEdit = b.startEdit as (x: unknown) => void;
    const editText = b.editText as { value: string };
    const editProblem = b.editProblem as (x: unknown) => boolean;

    startEdit(o);
    expect(editText.value).toBe('舊的');
    expect(editProblem(o)).toBe(true);
    editText.value = '新的內容';
    expect(editProblem(o)).toBe(false);
    // More than the round has room for.
    editText.value = 'x'.repeat(13);
    expect(editProblem(o)).toBe(true);
    editText.value = '新的內容';
    (b.saveEdit as (x: unknown) => void)(o);
    expect(emitted).toContainEqual(['edit', 'o2', '新的內容']);
    const withdraw = b.withdraw as (x: unknown) => void;

    withdraw(o);
    expect(emitted.some((e) => e[0] === 'withdraw')).toBe(false);
    withdraw(o);
    expect(emitted).toContainEqual(['withdraw', 'o2']);
    app.unmount();
  });

  it('the round panel moves the deadline and sends step comments', () => {
    const emitted: unknown[][] = [];
    const round = {
      id: 'r1', app: 'todo', initiator: 'u-a', scope: ['web'], state: 'confirming', deadline: 0, opinions: [], confirmers: [], message: '', result: null, log: [],
      plan:         { planId: 'p', summary: 's', steps: ['做排序'], architecture: {} },
      planComments: [{
        id: 'c1', step: 1, stepText: '做排序', by: 'u-b', at: '', text: '先只做這個', supporters: []
      }],
      you: {
        user: 'u-a', isInitiator: true, mustConfirm: true, canAddOpinion: false, canComment: true
      },
    };
    const { b, app } = mount('../components/RoundPanel.vue', {
      round, draft: '', pending: null, elapsed: 0, error: '', disabled: false, drafts: [], removedDraft: null, selectedTarget: null, notice: '', limits: LIMITS, available: true, draftsShared: true
    }, emitted);

    expect((b.commentsOf as (n: number) => unknown[])(1)).toHaveLength(1);
    (b.stepText as { value: Record<number, string> }).value = { 1: '再小一點' };
    (b.sendComment as (n: number) => void)(1);
    expect(emitted).toContainEqual(['comment', 1, '再小一點']);
    (b.stepText as { value: Record<number, string> }).value = { 1: 'x'.repeat(301) };
    expect((b.stepTooLong as (n: number) => boolean)(1)).toBe(true);
    (b.convergeNow as () => void)();
    expect(emitted).toContainEqual(['deadline', 0]);
    app.unmount();
  });

  it('past rounds load when opened', async() => {
    const realFetch = global.fetch;

    global.fetch = jest.fn(() => Promise.reject(new Error('offline'))) as unknown as typeof fetch;
    const { b, app } = mount('../components/PastRounds.vue', { app: 'todo', messageFor: () => 'offline' });

    (b.onToggle as (e: unknown) => void)({ target: { open: true } });
    await new Promise((r) => setTimeout(r, 20));
    expect((b.error as { value: string }).value).toBeTruthy();
    expect((b.loaded as { value: boolean }).value).toBe(false);
    // A known log line is shown in the page's language.
    expect((b.logText as (l: unknown) => string)({ event: '撤回意見 o2', code: 'withdrawn', args: { opinion: 'o2' } })).toBe('laoban.round.events.withdrawn');
    expect((b.logText as (l: unknown) => string)({ event: '截止', code: 'deadline', args: { minutes: '0' } })).toBe('laoban.round.events.deadlineNow');
    app.unmount();
    global.fetch = realFetch;
  });
});
