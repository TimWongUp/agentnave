const { readFileSync } = require("node:fs");
const vm = require("node:vm");

// Run the shared production helpers instead of maintaining test-side naming rules.
function common(english = false) {
  const storage = new Map(), listeners = {};
  let selection = "";
  const context = vm.createContext({
    document: {
      documentElement: { lang: english ? "en" : "zh", dataset: {}, classList: { toggle() {} } },
      querySelectorAll: () => [], querySelector: () => null, addEventListener: (name, fn) => { listeners[name] = fn; },
    },
    getSelection: () => selection,
    window: { ANAppearance: { applyTheme() {}, reducedMotion: { matches: true } }, addEventListener() {}, dispatchEvent() {} },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
    localStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) },
    fetch: async () => ({ ok: false, json: async () => ({}) }),
    setInterval() {},
  });
  vm.runInContext(readFileSync(`${__dirname}/../../common.js`, "utf8"), context);
  const api = Object.fromEntries(["deskName", "deskParts", "handleOf", "fullHandle", "cliOf", "modelName", "setFullModelNames", "dayLabel", "clock", "fullTime"]
    .map(key => [key, context.window.AN[key]]));
  // Whether the shared contextmenu listener blocks WebKit's menu for this event and selection.
  api.menuBlocked = (event, current = "") => {
    selection = current;
    let blocked = false;
    listeners.contextmenu({ clientX: 0, clientY: 0, ...event, preventDefault() { blocked = true; } });
    return blocked;
  };
  return api;
}

// Concrete service projections, deliberately not a second naming implementation.
const identities = {
  review: {
    key: "desk", provider: "codex", cli: "Codex", model: "gpt-x", model_short: "gpt-x", model_source: "native",
    role: "评审", ordinal: 1, numbered: false, handle: "@gpt-x·Codex·评审", handle_short: "@gpt-x·Codex·评审", spoken_name: "GPT",
  },
  explorer: {
    key: "a", provider: "claude", cli: "Claude Code", model: "claude-x", model_short: "claude-x", model_source: "requested",
    role: "探索派", ordinal: 1, numbered: false, handle: "@claude-x·Claude Code·探索派", handle_short: "@claude-x·Claude Code·探索派", spoken_name: "Claude",
  },
  pragmatist: {
    key: "b", provider: "claude", cli: "Claude Code", model: "claude-y", model_short: "claude-y", model_source: "requested",
    role: "实用派", ordinal: 2, numbered: false, handle: "@claude-y·Claude Code·实用派", handle_short: "@claude-y·Claude Code·实用派", spoken_name: "Claude",
  },
  coder: {
    key: "c", provider: "codex", cli: "Codex", model: "gpt-z", model_short: "gpt-z", model_source: "requested",
    role: "", ordinal: 3, numbered: false, handle: "@gpt-z·Codex", handle_short: "@gpt-z·Codex", spoken_name: "GPT",
  },
};

module.exports = { common, identities };
