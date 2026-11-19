const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const flush = () => new Promise(resolve => setImmediate(resolve));
const decoder = new TextDecoder();

function makeHeaders(map = {}) {
  const normalized = new Map(Object.entries(map).map(([k, v]) => [k.toLowerCase(), String(v)]));
  return { get: name => normalized.get(String(name).toLowerCase()) ?? null };
}

function makeResponse({ status = 200, body = "", headers = {} } = {}) {
  const bytes = typeof body === "string" ? new TextEncoder().encode(body) : new Uint8Array(body);
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: makeHeaders(headers),
    arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
  };
}

function createFakeTerminalClass(written) {
  return class FakeTerminal {
    constructor(options) {
      let themeValue = options.theme;
      this.themeSets = 0;
      this.options = { ...options };
      Object.defineProperty(this.options, "theme", {
        enumerable: true,
        get: () => themeValue,
        set: v => { themeValue = v; this.themeSets++; },
      });
      this._csi = [];
      this._dataListeners = [];
      this._binaryListeners = [];
      this._resizeListeners = [];
      this.focused = 0;
      this.disposed = false;
      this.parser = {
        registerCsiHandler: (id, cb) => { this._csi.push({ id, cb }); },
      };
    }
    loadAddon() {}
    open() {}
    focus() { this.focused++; }
    hasSelection() { return false; }
    onData(cb) { this._dataListeners.push(cb); }
    onBinary(cb) { this._binaryListeners.push(cb); }
    onResize(cb) { this._resizeListeners.push(cb); }
    emitData(data) { for (const cb of this._dataListeners) cb(data); }
    emitBinary(data) { for (const cb of this._binaryListeners) cb(data); }
    write(data, cb) {
      const text = typeof data === "string" ? data : decoder.decode(data);
      written.push(text);
      queueMicrotask(() => {
        if (text.includes("\x1b[c")) this.emitData("\x1b[?1;2c");
        if (text.includes("\x1b]10;?")) this.emitData("\x1b]10;rgb:eeee/eeee/eeee\x1b\\");
        if (text.includes("\x1b]11;?")) this.emitData("\x1b]11;rgb:1414/1414/1414\x1b\\");
        if (text.includes("\x1b[6n")) this.emitData("\x1b[1;1R");
        for (const match of text.matchAll(/\x1b\[\?([0-9;]+)([hlmn])/g)) {
          const params = match[1].split(";").map(Number);
          const final = match[2];
          for (const handler of this._csi) {
            if (handler.id.prefix === "?" && handler.id.final === final) handler.cb(params);
          }
        }
        cb?.();
      });
    }
    dispose() { this.disposed = true; }
  };
}

function createHarness({ theme = "dark", lang = "zh", useRealXterm = false, transport } = {}) {
  const docListeners = {};
  const winListeners = {};
  const observers = [];
  const written = [];
  const posted = [];
  const inputs = [];
  const resizes = [];
  const timeouts = [];
  const root = { lang, dataset: { theme, motion: "full" } };
  const termContainer = { children: [], append(...nodes) { this.children.push(...nodes); } };

  const parent = { postMessage(message, targetOrigin) { posted.push({ message, targetOrigin }); } };
  const windowObj = {
    parent,
    addEventListener(name, fn) { (winListeners[name] ||= []).push(fn); },
    dispatchEvent(event) { for (const fn of winListeners[event.type] || []) fn(event); },
  };

  let TerminalClass;
  if (useRealXterm) {
    const xtermContext = vm.createContext({
      globalThis: null,
      self: null,
      window: null,
      document: { queryCommandSupported: () => false },
      setTimeout,
      clearTimeout,
      setInterval,
      clearInterval,
      queueMicrotask,
      requestAnimationFrame: fn => setTimeout(fn, 0),
      cancelAnimationFrame: clearTimeout,
      TextDecoder,
      TextEncoder,
      Uint8Array,
      ArrayBuffer,
      Map,
      Set,
      WeakMap,
      WeakSet,
      Promise,
      matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
      navigator: { userAgent: "Mac", platform: "MacIntel", language: "en-US" },
      performance: { now: () => Date.now() },
    });
    xtermContext.globalThis = xtermContext;
    xtermContext.self = xtermContext;
    xtermContext.window = xtermContext;
    vm.runInContext(readFileSync(`${__dirname}/../vendor/xterm/xterm.js`, "utf8"), xtermContext);
    const RealTerminal = xtermContext.Terminal;
    TerminalClass = class HeadlessTerminal extends RealTerminal {
      constructor(options) {
        super(options);
        this.focused = 0;
      }
      open() {}
      focus() { this.focused++; }
      write(data, cb) {
        written.push(typeof data === "string" ? data : decoder.decode(data));
        return super.write(data, cb);
      }
    };
  } else {
    TerminalClass = createFakeTerminalClass(written);
  }

  let latestTerm;
  const BaseTerminal = TerminalClass;
  TerminalClass = class TrackedTerminal extends BaseTerminal {
    constructor(options) { super(options); latestTerm = this; }
  };

  const context = vm.createContext({
    URLSearchParams: class {
      get(key) { return key === "session" ? "42" : null; }
    },
    location: { search: "?session=42", origin: "http://agentnave.localhost" },
    document: {
      documentElement: root,
      createElement: () => ({ className: "", offsetWidth: 800, offsetHeight: 600 }),
      addEventListener(name, fn) { (docListeners[name] ||= []).push(fn); },
    },
    window: windowObj,
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    AN: {
      $: id => (id === "term" ? termContainer : null),
      L: (zh, en) => (root.lang.startsWith("en") ? en : zh),
    },
    getComputedStyle: () => ({
      getPropertyValue: name => {
        if (name === "--term-fg") return root.dataset.theme === "light" ? "#1f1f1f" : "#e4e4e4";
        if (name === "--term-bg") return root.dataset.theme === "light" ? "#eeeeee" : "#141414";
        return "#888888";
      },
    }),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    ResizeObserver: class { observe() {} disconnect() {} },
    MutationObserver: class {
      constructor(cb) { this.cb = cb; observers.push(this); }
      observe(_target, options) { this.options = options; }
      disconnect() {}
    },
    FitAddon: { FitAddon: class { activate() {} dispose() {} fit() {} } },
    WebglAddon: { WebglAddon: class { constructor() { throw new Error("no webgl"); } } },
    Terminal: TerminalClass,
    Uint8Array,
    Promise,
    Number,
    setTimeout: (fn, ms) => {
      timeouts.push(ms);
      return setTimeout(fn, 0);
    },
    clearTimeout,
    queueMicrotask,
    fetch: async (path, options) => {
      if (path.startsWith("/app/terminal/input")) {
        const body = typeof options.body === "string"
          ? options.body
          : decoder.decode(options.body);
        inputs.push(body);
        return makeResponse({ status: 200, body: '{"ok":true}' });
      }
      if (path.startsWith("/app/terminal/resize")) {
        resizes.push(JSON.parse(options.body));
        return makeResponse({ status: 200, body: '{"ok":true}' });
      }
      return transport(path, options);
    },
  });
  context.window.TerminalView = undefined;
  vm.runInContext(readFileSync(`${__dirname}/../terminal-view.js`, "utf8"), context);
  context.TerminalView = context.window.TerminalView;
  vm.runInContext(readFileSync(`${__dirname}/../terminal.js`, "utf8"), context);

  return {
    root,
    written,
    posted,
    inputs,
    resizes,
    timeouts,
    term: () => latestTerm,
    start: async () => {
      for (const fn of docListeners.DOMContentLoaded || []) fn();
      await flush();
    },
    triggerThemeMutation: () => {
      for (const obs of observers) {
        if (obs.options?.attributeFilter?.includes("data-theme")) obs.cb();
      }
    },
    sendParentMessage: data => {
      for (const fn of winListeners.message || []) fn({ source: parent, data });
    },
  };
}

test("new session first packet answers queries once without duplicate 997 using vendored xterm.js", async () => {
  let step = 0;
  let hold;
  const app = createHarness({
    useRealXterm: true,
    transport: async () => {
      step++;
      if (step === 1) {
        return makeResponse({
          body: "\x1b[?2031h\x1b[?996n\x1b[c\x1b[6n",
          headers: { "X-Terminal-Start": 0, "X-Terminal-End": 25, "X-Terminal-Appearance": 1 },
        });
      }
      return new Promise(resolve => { hold = resolve; });
    },
  });

  await app.start();
  await new Promise(resolve => setTimeout(resolve, 30));

  assert.deepEqual(app.inputs, ["\x1b[?997;1n", "\x1b[?1;2c", "\x1b[1;1R"]);
  hold(makeResponse({ body: "", headers: { "X-Terminal-Start": 25, "X-Terminal-End": 25, "X-Terminal-Exit": "0" } }));
  await new Promise(resolve => setTimeout(resolve, 20));
});

test("replay suppresses all PTY answers including DA1, OSC 10/11, DSR, and ?996n, then sends appearance once and answers subsequent queries", async () => {
  let step = 0;
  let hold;
  const app = createHarness({
    transport: async path => {
      step++;
      if (step === 1) {
        assert.match(path, /from=0$/);
        // Truncated replay: ?2031h is gone from bytes, but X-Terminal-Appearance is 1.
        return makeResponse({
          body: "\x1b[c\x1b]10;?\x07\x1b]11;?\x07\x1b[6n\x1b[?996n",
          headers: {
            "X-Terminal-Start": 100,
            "X-Terminal-End": 200,
            "X-Terminal-Appearance": 1,
            "X-Terminal-Replay": 1,
          },
        });
      }
      if (step === 2) {
        assert.match(path, /from=200$/);
        return makeResponse({
          body: "\x1b[c\x1b]10;?\x07\x1b]11;?\x07\x1b[6n\x1b[?996n",
          headers: {
            "X-Terminal-Start": 200,
            "X-Terminal-End": 240,
            "X-Terminal-Appearance": 1,
          },
        });
      }
      return new Promise(resolve => { hold = resolve; });
    },
  });

  await app.start();
  await new Promise(resolve => setTimeout(resolve, 30));

  // Replay produced 0 query answers, then sent 1 appearance notification after replay finished;
  // live segment after replay answered each query (DA1, OSC 10, OSC 11, DSR, ?996n) once.
  assert.deepEqual(app.inputs, [
    "\x1b[?997;1n",
    "\x1b[?1;2c",
    "\x1b]10;rgb:eeee/eeee/eeee\x1b\\",
    "\x1b]11;rgb:1414/1414/1414\x1b\\",
    "\x1b[1;1R",
    "\x1b[?997;1n",
  ]);
  hold(makeResponse({ body: "", headers: { "X-Terminal-Start": 240, "X-Terminal-End": 240, "X-Terminal-Exit": "0" } }));
  await flush();
});

test("replaying defers focus until write callback and drops binary/data during replay only", async () => {
  let releaseReplayWrite;
  let releaseSecondFetch;
  const inputs = [];
  const written = [];
  const FakeTerm = createFakeTerminalClass(written);
  class ControlledTerminal extends FakeTerm {
    write(data, cb) {
      written.push(decoder.decode(data));
      queueMicrotask(() => {
        this.emitData("\x1b[?1;2c");
        this.emitBinary("\x00\x01");
        for (const h of this._csi) if (h.id.final === "n") h.cb([996]);
        releaseReplayWrite = cb;
      });
    }
  }
  const root = { lang: "zh", dataset: { theme: "dark", motion: "full" } };

  const context = vm.createContext({
    document: { documentElement: root, createElement: () => ({ className: "", offsetWidth: 800, offsetHeight: 600 }) },
    window: { addEventListener() {} },
    AN: { L: zh => zh },
    getComputedStyle: () => ({ getPropertyValue: () => "#141414" }),
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
    ResizeObserver: class { observe() {} disconnect() {} },
    MutationObserver: class { observe() {} disconnect() {} },
    FitAddon: { FitAddon: class { activate() {} dispose() {} fit() {} } },
    WebglAddon: { WebglAddon: class { constructor() { throw new Error("no webgl"); } } },
    Terminal: ControlledTerminal,
    Uint8Array, Promise, Number, setTimeout, clearTimeout, queueMicrotask,
    fetch: async path => {
      if (path.startsWith("/app/terminal/input")) {
        inputs.push(typeof arguments?.[1]?.body === "string" ? arguments[1].body : "");
        return makeResponse({ status: 200 });
      }
      return makeResponse({
        body: "\x1b[c\x1b[?996n",
        headers: { "X-Terminal-Start": 0, "X-Terminal-End": 10, "X-Terminal-Appearance": 0, "X-Terminal-Replay": 1 },
      });
    },
  });
  let fetchCount = 0;
  context.fetch = async (path, options) => {
    if (path.startsWith("/app/terminal/input")) {
      inputs.push(typeof options.body === "string" ? options.body : decoder.decode(options.body));
      return makeResponse({ status: 200 });
    }
    fetchCount++;
    if (fetchCount === 1) {
      return makeResponse({
        body: "\x1b[c\x1b[?996n",
        headers: { "X-Terminal-Start": 0, "X-Terminal-End": 10, "X-Terminal-Appearance": 0, "X-Terminal-Replay": 1 },
      });
    }
    return new Promise(resolve => { releaseSecondFetch = resolve; });
  };
  vm.runInContext(readFileSync(`${__dirname}/../terminal-view.js`, "utf8"), context);
  const view = new context.window.TerminalView(7);
  view.open();
  await flush();
  await flush();

  // While replay write is in flight: focus is deferred and keystrokes/responses are dropped.
  view.focus();
  assert.equal(view.term.focused, 0);
  view.term.emitData("ignored-during-replay");
  await flush();
  assert.deepEqual(inputs, []);

  // Once replay write completes: focus runs and subsequent typing works normally.
  releaseReplayWrite();
  await flush();
  assert.equal(view.term.focused, 1);
  assert.deepEqual(inputs, []);
  view.term.emitData("hello");
  await flush();
  assert.deepEqual(inputs, ["hello"]);

  releaseSecondFetch(makeResponse({ body: "", headers: { "X-Terminal-Start": 10, "X-Terminal-End": 10, "X-Terminal-Exit": "0" } }));
  await flush();
});

test("restyle deduplicates identical theme writes and notifies only on real light/dark changes", async () => {
  let step = 0;
  let hold;
  const app = createHarness({
    theme: "dark",
    transport: async () => {
      step++;
      if (step === 1) {
        return makeResponse({
          body: "ready",
          headers: { "X-Terminal-Start": 0, "X-Terminal-End": 5, "X-Terminal-Appearance": 1 },
        });
      }
      return new Promise(resolve => { hold = resolve; });
    },
  });

  await app.start();
  await flush();
  assert.deepEqual(app.inputs, []);

  // Rewriting the same data-theme (e.g. window focus/visibilitychange) does not reset theme or notify CLI.
  app.root.dataset.theme = "dark";
  app.triggerThemeMutation();
  await flush();
  assert.equal(app.term().themeSets, 0);
  assert.deepEqual(app.inputs, []);

  // Switching dark -> light resets theme and notifies once; rewriting light -> light does neither; light -> dark resets and notifies once.
  app.root.dataset.theme = "light";
  app.triggerThemeMutation();
  await flush();
  assert.equal(app.term().themeSets, 1);
  app.root.dataset.theme = "light";
  app.triggerThemeMutation();
  await flush();
  assert.equal(app.term().themeSets, 1);
  app.root.dataset.theme = "dark";
  app.triggerThemeMutation();
  await flush();
  assert.equal(app.term().themeSets, 2);
  assert.deepEqual(app.inputs, ["\x1b[?997;2n", "\x1b[?997;1n"]);

  hold(makeResponse({ body: "", headers: { "X-Terminal-Start": 5, "X-Terminal-End": 5, "X-Terminal-Exit": "0" } }));
  await flush();
});

test("exit handling: 5xx retries, 0 omits note, non-zero writes localized note, 404 exits, postMessage fires and stops input", async () => {
  // 1. 500 retries, then exit 0: no terminal exit line, postMessage code 0, no further inputs.
  let step0 = 0;
  const clean = createHarness({
    transport: async () => {
      step0++;
      if (step0 === 1) return makeResponse({ status: 500 });
      return makeResponse({
        body: "bye",
        headers: { "X-Terminal-Start": 0, "X-Terminal-End": 3, "X-Terminal-Appearance": 1, "X-Terminal-Exit": "0" },
      });
    },
  });
  await clean.start();
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(clean.timeouts, [1000]);
  assert.deepEqual(clean.written, ["bye"]);
  assert.deepEqual(JSON.parse(JSON.stringify(clean.posted)), [{
    message: { type: "agentnave:terminal", action: "exited", session: 42, code: 0 },
    targetOrigin: "http://agentnave.localhost",
  }]);
  clean.root.dataset.theme = "light";
  clean.triggerThemeMutation();
  await flush();
  assert.deepEqual(clean.inputs, []);

  // 2. Non-zero exit in English writes Exited (code 2) and posts code 2.
  const failed = createHarness({
    lang: "en",
    transport: async () => makeResponse({
      body: "",
      headers: { "X-Terminal-Start": 0, "X-Terminal-End": 0, "X-Terminal-Appearance": 0, "X-Terminal-Exit": "2" },
    }),
  });
  await failed.start();
  await flush();
  assert.deepEqual(failed.written, ["\r\n\x1b[2mExited (code 2)\x1b[0m\r\n"]);
  assert.deepEqual(JSON.parse(JSON.stringify(failed.posted[0].message)), { type: "agentnave:terminal", action: "exited", session: 42, code: 2 });

  // 3. 404 ends session with null code and posts null.
  const missing = createHarness({
    transport: async () => makeResponse({ status: 404 }),
  });
  await missing.start();
  await flush();
  assert.deepEqual(missing.written, []);
  assert.deepEqual(JSON.parse(JSON.stringify(missing.posted[0].message)), { type: "agentnave:terminal", action: "exited", session: 42, code: null });
});

test("each TerminalView sends a distinct view id on output requests", async () => {
  const seen = [];
  const createWithCapture = async () => {
    const app = createHarness({
      transport: async path => {
        seen.push(path);
        return makeResponse({
          body: "",
          headers: { "X-Terminal-Start": 0, "X-Terminal-End": 0, "X-Terminal-Appearance": 0, "X-Terminal-Exit": "0" },
        });
      },
    });
    await app.start();
    await flush();
  };
  await createWithCapture();
  await createWithCapture();
  assert.equal(seen.length, 2);
  const views = seen.map(path => path.match(/[?&]view=([^&]+)/)?.[1]);
  assert.ok(views[0] && views[1]);
  assert.notEqual(views[0], views[1]);
});
