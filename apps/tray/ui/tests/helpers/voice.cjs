const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { common } = require("./common.cjs");

function dom() {
  const nodes = [];
  const matches = (item, selector) => selector === "nav a" ? item.tag.startsWith("a.") : selector === "a[href]" ? item.tag.startsWith("a.") && item.attributes.href != null : selector.startsWith(".") ? item.tag.endsWith(selector) : selector === "a" ? item.tag.startsWith("a.") : item.tag === selector;
  function h(tag, attributes = {}, ...children) {
    const node = { tag, style: {}, classList: { add() {}, toggle() {} }, attributes: { ...attributes }, dataset: {}, children, textContent: children.filter(c => typeof c === "string").join(""), listeners: {},
      setAttribute(key, value) { this.attributes[key] = value; }, removeAttribute(key) { delete this.attributes[key]; }, get href() { return this.attributes.href; }, getAttribute(key) { return this.attributes[key]; }, before() {},
      prepend(...items) { this.children.unshift(...items); }, contains(item) { return this === item || this.children.some(child => child?.contains?.(item)); },
      querySelector(selector) { return nodes.find(item => matches(item, selector)) || h(`i${selector}`); }, querySelectorAll(selector) { return nodes.filter(item => matches(item, selector)); },
      append(...items) { this.children.push(...items); },
      replaceChildren(...items) { this.children = items; }, remove() { this.removed = true; },
      addEventListener(name, fn) { this.listeners[name] = fn; } };
    for (const child of children) if (child && typeof child === "object") child.parentNode = node;
    if (tag.startsWith("iframe")) node.contentWindow = { messages: [], postMessage(data) { this.messages.push(data); } };
    nodes.push(node);
    return node;
  }
  const elements = new Map();
  const document = { hidden: false, body: h("body"), querySelector: selector => nodes.find(item => matches(item, selector)) || h("div"), querySelectorAll: selector => nodes.filter(item => matches(item, selector)), createElementNS: (_ns, tag) => h(tag), documentElement: { style: { setProperty() {} } }, getElementById: id => {
    if (!elements.has(id)) elements.set(id, Object.assign(h("div"), { id, hidden: true, getBoundingClientRect: () => ({ height: 32 }) }));
    return elements.get(id);
  }, listeners: {}, addEventListener(name, fn) { this.listeners[name] = fn; } };
  return { h, nodes, document, elements };
}

const flush = () => new Promise(resolve => setImmediate(resolve));
const message = (id, time, extra = {}) => ({
  id, time, text: id, kind: "reply", status: "succeeded", desk: id,
  speaker: { role: "member", provider: "codex" }, thread: { kind: "desk" }, visibility: "private", ...extra,
});

function startButton(AN, ctx, messages) {
  const readable = m => m.text && ["member", "manager"].includes(m.speaker?.role) && ["prompt", "instruction", "forward", "reply"].includes(m.kind)
    && !["running", "pending", "not_started", "review"].includes(m.status);
  // Clicking the first message of a conversation with chaining on reads it to the end.
  return ({ hidden: !messages.some(readable), listeners: { async click() {
    const first = messages.filter(readable).sort((x, y) => Date.parse(x.time) - Date.parse(y.time))[0];
    if (!first) return;
    const rec = { bub: { prepend() {} } };
    AN.voice.decorate(rec, first, ctx);
    rec.read.listeners.click();
    await flush();
  } } });
}

async function office(listening = true, synthesize = async () => ({}), loadRoom = async data => ({ ok: true, json: async () => structuredClone(data) }), read_scope = "all", transport = async () => null) {
  let config = { enabled: true, listen_minutes: 60, continue_read: true, read_scope, engine: "elevenlabs", keys: { elevenlabs: true }, summary: { enabled: true } };
  const requests = [], sounds = [], playbacks = [], navigations = [], published = [], located = [];
  const surface = dom(), { nodes } = surface;
  const windowListeners = {}, timers = new Map();
  let now = 0, timerId = 0;
  const rooms = new Map();
  const { h, document } = surface;
  const navLinks = [h("a.office-nav", { href: "http://agentnave.localhost/chat/" }), h("a.settings-nav", { href: "http://agentnave.localhost/settings/" })];
  class Audio {
    constructor(src) { this.text = decodeURIComponent(src.slice(5)); this.currentTime = 0; this.paused = true; this.plays = 0; sounds.push(this); }
    play() { this.paused = false; this.plays++; playbacks.push({ text: this.text, time: this.currentTime }); return Promise.resolve(); }
    pause() { this.paused = true; }
    removeAttribute() {}
    load() {}
  }
  const AN = { ...common(), h, L: (zh, en) => String(document.documentElement.lang || "zh").startsWith("en") ? en : zh, setLanguage: lang => { document.documentElement.lang = lang; }, href: path => path, updateFlow: () => navigations.push("check-update"),
    art: { character: (provider, model) => model || provider || "default", head: name => h("span.head", { name }) },
    reconcile: (root, items) => { root.children = items; },
  };
  const context = vm.createContext({
    AN, document, Audio, AbortController, Map, Event, CustomEvent: class extends Event { constructor(type, options) { super(type); this.detail = options?.detail; } }, queueMicrotask,
    location: Object.assign(new URL("http://agentnave.localhost/chat/workspace"), { assign: path => navigations.push(path) }),
    go: path => navigations.push(path),
    ResizeObserver: class { observe() {} },
    setTimeout: (fn, delay) => { const id = ++timerId; timers.set(id, { fn, at: now + delay }); return id; }, clearTimeout: id => timers.delete(id),
    Avatars: { family: (provider, model) => model?.includes("claude") ? "claude" : provider === "codex" ? "gpt" : provider || null },
    Date: class extends Date { static now() { return now; } },
    window: { dispatchEvent(event) { if (event.type === "agentnave:voice-locate") located.push(event.detail); }, addEventListener(name, fn) { windowListeners[name] = fn; } },
    URL: class extends URL { static createObjectURL(blob) { return `blob:${encodeURIComponent(blob.text)}`; } static revokeObjectURL() {} },
    async fetch(path, options) {
      const response = await transport(path, options);
      if (response) return response;
      if (rooms.has(path)) return loadRoom(rooms.get(path));
      if (path === "/app/voice/speak") {
        requests.push(JSON.parse(options.body));
        return { ok: true, status: 200, headers: { get: () => null }, blob: async () => { await synthesize(JSON.parse(options.body), options.signal); return { text: JSON.parse(options.body).text }; } };
      }
      if (path === "/app/voice/player") { published.push(JSON.parse(options.body)); return { ok: true, json: async () => ({ ok: true }) }; }
      if (options?.method === "POST") config = JSON.parse(options.body);
      return { ok: true, json: async () => structuredClone(config) };
    },
  });
  for (const file of ["office-settings.js", "voice-menu.js", "voice.js"]) vm.runInContext(readFileSync(`${__dirname}/../../${file}`, "utf8"), context);
  await flush();
  const ctx = { c: { group: "workspace", host: "codex", messages: [], threads: [] }, desks: new Map(), rooms: new Map() };
  const starter = messages => startButton(AN, ctx, messages);
  const tick = async (ms = 2000) => {
    now += ms;
    const due = [...timers].filter(([, timer]) => timer.at <= now);
    for (const [id, timer] of due) if (timers.delete(id)) timer.fn();
    await flush(); await flush();
  };
  const command = async (kind, value) => { windowListeners["agentnave:voice-command"]({ detail: { kind, value } }); await flush(); };
  const feed = async messages => {
    ctx.c.messages = messages;
    rooms.set("/app/chats/workspace", { ...ctx.c, messages, desks: [...ctx.desks.values()], rooms: [...ctx.rooms.values()] });
    // Each feed is a poll after the window came back, which reads the settings again.
    windowListeners["agentnave:focus"]?.();
    AN.voice.ingest({ group: "workspace", messages }); await flush();
    await tick();
  };
  await feed([]);
  AN.voice.controls(true, null, ctx);
  if (listening) await command("listen", { group: "workspace" });
  return {
    AN, document, avatars: context.Avatars, voice: AN.voice, requests, sounds, playbacks, ctx, feed, nodes, navigations, published, located, rooms, navLinks,
    clickLink(link) { document.listeners.click({ button: 0, target: { closest: () => link }, preventDefault() {} }); },
    tick, advance(ms) { now += ms; },
    // A click in the menu bar panel's copy of the top bar control.
    command,
    navigate: path => AN.navigate(path),
    setConfig: patch => Object.assign(config, patch),
    settingsMessage(data, source = nodes.find(n => n.tag === "iframe.office-settings")?.contentWindow) { windowListeners.message({ source, data: { type: "agentnave:settings", ...data } }); },
    async settings(patch) { Object.assign(config, patch); await feed(ctx.c.messages); },
    async scope(value) { config.read_scope = value; await feed(ctx.c.messages); await flush(); },
    stop() { if (nodes.find(n => n.tag === "div.voice-menu").hidden) nodes.find(n => n.tag === "button.bar-voice").listeners.click(); nodes.find(n => n.tag === "button.voice-primary").listeners.click(); },
    // A desk view, showing what chat.js's desk stream would: here, the desk's own messages.
    private(desk, messages = [], shown = chat => chat.messages.filter(m => m.desk === desk.key)) {
      ctx.c.messages = messages;
      ctx.desks = new Map([[desk.key, desk]]);
      rooms.set("/app/chats/workspace/history", { ...ctx.c, messages, desks: [desk] });
      AN.voice.controls(true, null, ctx, desk.key, shown);
      return starter(messages);
    },
    room(messages, id = "room", desks = null) {
      const members = messages.filter(m => m.speaker?.role === "member" && m.conversation === id && m.kind === "reply");
      ctx.desks = new Map(desks ? desks.map(d => [d.key, d]) : members.map(m => [m.desk, { key: m.desk, provider: m.speaker.provider, model: m.speaker.model || m.desk, identity: m.speaker.identity }]));
      ctx.c.messages = messages;
      ctx.rooms.set(id, { id, seats: [...ctx.desks.values()].map(d => ({ desk: d.key, identity: d.identity })) });
      rooms.set(`/app/chats/workspace/focus/${id}`, { group: "workspace", host: "codex", desks: [...ctx.desks.values()], rooms: [...ctx.rooms.values()], messages });
      AN.voice.controls(true, { group: "workspace", id }, ctx);
      return starter(messages.filter(m => m.conversation === id));
    },
    async end() { const clip = sounds.find(sound => !sound.paused); if (clip) { clip.paused = true; clip.onended(); } await flush(); },
    async leave() { windowListeners.pagehide(); await flush(); },
    async hide() { document.hidden = true; document.listeners.visibilitychange(); await flush(); },
  };
}


module.exports = { office, dom, flush, message };

async function panel(managers = [], commandReply = async () => ({ ok: true, json: async () => ({ ok: true, opening: true }) })) {
  const { h, nodes, document, elements } = dom(), events = {}, commands = [], timers = new Map();
  let timer = 0, now = 0;
  const data = { managers, counts: { running: 0 }, empty: null };
  const config = { enabled: true, engine: "elevenlabs", keys: { elevenlabs: true } };
  const AN = { ...common(), h, $: id => document.getElementById(id), L: zh => zh, href: path => path,
    art: { character: (_provider, model) => model || "default", head: name => h("span.head", { name }) },
    themeSwitch: () => h("span.theme"), load: async path => path === "/app/voice" ? config : path === "/app/voice/player" ? null : path === "/app/stage" ? data : {},
    setText: (el, text) => { el.textContent = text; }, setAttr: (el, key, value) => el.setAttribute(key, value), setClass: (el, value) => { el.className = value; },
    reconcile: (el, children) => { el.children = children; }, stickyOrder: (_old, items) => items.sort((a, b) => a.rank - b.rank).map(item => item.id),
    activities: d => d.activities || [], dozing: d => d.sleep === true, ongoing: value => value, urgent: value => ["running", "review"].includes(value),
    pick: (_table, value) => [value, value === "running" ? "run" : "idle"], colorClass: () => "c0", relTime: () => "刚刚", fullTime: value => value,
    MEMBER: {}, GROUP: {}, hostTile: () => h("span.host-tile"), cliName: value => value, chatName: m => m.group,
    shownName: m => m.group, chatPlace: () => "project", clip: value => value, at: Date.parse, poll: fn => { queueMicrotask(fn); return fn; } };
  const context = vm.createContext({ AN, document, URLSearchParams, location: { search: "" }, Date,
    addEventListener: (name, fn) => { events[name] = fn; },
    setTimeout: (fn, delay) => { const id = ++timer; timers.set(id, { fn, at: now + delay }); return id; }, clearTimeout: id => timers.delete(id),
    fetch: async (_path, options) => { const command = JSON.parse(options.body); commands.push(command); return commandReply(command); } });
  for (const file of ["voice-menu.js", "panel.js"]) vm.runInContext(readFileSync(`${__dirname}/../../${file}`, "utf8"), context);
  await flush(); await flush();
  return { nodes, elements, commands, config,
    snapshot: raw => AN.voiceState({ enabled: true, ready: true, state: "idle", jobs: [], ...raw }),
    show: raw => { events["agentnave:voice-player"]({ detail: raw }); },
    render: managers => { data.managers = managers; context.render(data); },
    async feed(managers) { data.managers = managers; await context.refresh(); },
    async tick(ms) { now += ms; for (const [id, item] of timers) if (item.at <= now) { timers.delete(id); item.fn(); } await flush(); },
    async settings() { document.listeners.visibilitychange(); await flush(); },
  };
}
module.exports.panel = panel;

function settingsFrame(embedded = true) {
  const { document } = dom(), listeners = {}, messages = [], emitted = [];
  document.documentElement.dataset = { lang: "zh" }; document.documentElement.lang = "zh-CN";
  const parent = { postMessage: data => messages.push(data) };
  const emit = (name, event = {}) => { for (const fn of listeners[name] || []) fn(event); };
  const context = { document, URL, URLSearchParams, Event,
    CustomEvent: class extends Event { constructor(type, options) { super(type); this.detail = options?.detail; } },
    location: new URL(`http://agentnave.localhost/settings/${embedded ? "?embedded=1" : ""}#voice-settings`),
    addEventListener(name, fn) { (listeners[name] ||= []).push(fn); },
    dispatchEvent(event) { emitted.push(event.type); emit(event.type, event); },
  };
  context.window = context; context.parent = embedded ? parent : context;
  vm.runInNewContext(readFileSync(`${__dirname}/../../settings-frame.js`, "utf8"), context);
  return { context, document, messages, emitted, emit,
    api: context.AgentNaveSettingsFrame,
    message(data, source = parent) { emit("message", { source, data: { type: "agentnave:settings", ...data } }); },
  };
}
module.exports.settingsFrame = settingsFrame;
