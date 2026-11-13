const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");
const { mirrorSettings } = require("./helpers/settings.cjs");

function editor(fetch = async () => { throw new Error("unexpected request"); }, os = "macos") {
  const listeners = {}, controls = new Map();
  const control = (id) => {
    if (!controls.has(id)) controls.set(id, { options: [], value: "8", classList: { toggle() {} },
      append(node) { this.options.push(node); }, replaceChildren() {} });
    return controls.get(id);
  };
  const context = vm.createContext({
    AN: { shellOS: () => os, art: { url: (name) => `/art/${name}` } }, structuredClone, fetch, setTimeout: () => 0,
    document: {
      getElementById: control,
      createElement: (tag) => ({ tag, dataset: {}, setAttribute() {}, addEventListener() {}, appendChild() {} }),
      createTextNode: (text) => text,
    },
    window: { addEventListener: (name, fn) => { listeners[name] = fn; } },
  });
  const run = (code) => vm.runInContext(code, context);
  const load = (file) => {
    run(readFileSync(`${__dirname}/../${file}`, "utf8"));
    mirrorSettings(context);
  };
  for (const file of ["settings-state.js", "member-models.js"]) load(file);
  run(`
    updateControls = () => {};
    AN.settings.renderCards = () => {};
    AN.settings.renderErrorBanner = () => {};
    detected = { codex: { installed: true } };
    currentDoc = { version: 1, clis: {}, modes: {
      task: { codex: { models: [], overrides: {} } },
      discussion: { codex: { models: [], overrides: {} } }
    } };
    markSaved();
  `);
  return { run, load, listeners, doc: () => JSON.parse(run("JSON.stringify(currentDoc)")) };
}

test("Devin Fusion models stay out of choices even when saved previously", () => {
  const app = editor();
  app.run(`
    detected.devin = { models: [{ id: "swe-2-medium" }, { id: "fusion-gpt-sidekick-swe" }] };
    currentDoc.clis.devin = { models: { "fusion-gpt-sidekick-swe": {}, "swe-2-medium": {} },
      allowed_models: ["fusion-gpt-sidekick-swe", "swe-2-medium"] };
    currentDoc.modes.task.devin = { models: ["fusion-gpt-sidekick-swe"] };
  `);
  assert.deepEqual(Array.from(app.run('getAllModelIds("devin")')), ["swe-2-medium"]);
  assert.equal(app.doc().clis.devin.allowed_models[0], "fusion-gpt-sidekick-swe");
  app.run("AN.L = (zh) => zh");
  app.load("member-cards.js");
  assert.equal(app.run('poolSummary("devin", getAllModelIds("devin"), shownPool("devin"))'), "1 个：swe-2-medium");
  // A hidden model alone is no choice at all.
  app.run('currentDoc.clis.devin.allowed_models = ["fusion-gpt-sidekick-swe"]');
  assert.ok(app.run("configurationErrors().some((e) => /至少选一个模型/.test(e.message))"));
});

// The settings page's orchestration half, with a DOM just deep enough to read rendered rows.
function advancedEditor(fetch) {
  const app = editor(fetch);
  app.run(`
    AN.L = (zh) => zh;
    refreshDirty = () => {};
    const nodes = new Map();
    const node = (tag) => ({
      tagName: tag, attrs: {}, children: [], dataset: {}, listeners: {}, classList: { toggle() {} },
      setAttribute(key, value) { this.attrs[key] = value; }, removeAttribute(key) { delete this.attrs[key]; },
      addEventListener(type, fn) { this.listeners[type] = fn; }, appendChild(child) { this.children.push(child); },
      replaceChildren(...kids) { this.children = kids; }, querySelectorAll: () => [], querySelector: () => null,
    });
    document.createElement = node;
    document.createTextNode = (text) => text;
    document.getElementById = (id) => nodes.get(id) || nodes.set(id, node("div")).get(id);
    currentDoc.modes.task.codex = { models: ["chat-model"], overrides: {}, default: "chat-model" };
    detected.codex.models = [{ id: "chat-model" }];
  `);
  app.load("advanced.js");
  return app;
}

test("cooldown clear removes only the chosen CLI from the displayed list", async () => {
  const until = new Date(Date.now() + 60000).toISOString();
  let providers = { claude: { models: { a: { code: "auth", until: null }, b: { code: "quota", until } } } };
  const app = advancedEditor(async (url, options) => {
    if (url === "/app/provider-health/clear") { const { provider, model } = JSON.parse(options.body); delete providers[provider].models[model]; }
    return { ok: true, json: async () => ({ provider_health: providers }) };
  });
  await app.run("apiGetMembers()");
  assert.equal(app.run('$("provider-health").children.length'), 2);
  await app.run('$("provider-health").children[0].children[1].listeners.click()');
  assert.deepEqual(Object.keys(providers.claude.models), ["b"]);
  assert.equal(app.run('$("provider-health").children.length'), 1);
});

test("only a tier-governed option whose default loosens permissions is counted", () => {
  const app = editor();
  app.run(`currentDoc.clis = {
    claude: { models: { a: { options: { permission_mode: "default" } }, b: { options: { permission_mode: "plan" } } } },
    codex: { models: { a: { options: { sandbox: "read-only", approve_for_me: false, dangerously_bypass_approvals_and_sandbox: false } } } },
    antigravity: { models: { a: { options: { dangerously_skip_permissions: false, sandbox: true, mode: "ask" } } } },
    zcode: { models: { a: { options: { permission_mode: "guarded" } } } },
  }`);
  assert.equal(app.run("countPermissionOptions()"), 0);
  app.run(`currentDoc.clis.claude.models.a.options.permission_mode = "bypassPermissions";
    currentDoc.clis.codex.models.a.options.sandbox = "danger-full-access";
    currentDoc.clis.antigravity.models.a.options.dangerously_skip_permissions = true;
    currentDoc.clis.pi = { models: { a: { options: { permission_mode: "auto" } } } };
    currentDoc.modes.task.codex.overrides = { x: { options: { approve_for_me: true, unrelated_permission: true } } }`);
  assert.equal(app.run("countPermissionOptions()"), 4);
  assert.equal(app.run('grantsPermission("claude", "permission_mode", "auto")'), true);
});

test("the service's permission key table replaces the built-in one, and a refresh is a POST action", async () => {
  const calls = [];
  const app = editor(async (url, options = {}) => {
    calls.push([url, options.method || "GET", options.headers?.["X-AgentNave-Action"]]);
    return { ok: true, json: async () => ({ permission_keys: { pi: ["tier"], claude: [] } }) };
  });
  await app.run("apiGetMembers()");
  await app.run("apiGetMembers(true)");
  assert.deepEqual(calls, [["/settings/members", "GET", undefined], ["/settings/members", "POST", "1"]]);
  assert.equal(app.run('grantsPermission("pi", "tier", "auto")'), true);
  assert.equal(app.run('grantsPermission("claude", "permission_mode", "auto")'), false);
  assert.equal(app.run('grantsPermission("codex", "sandbox", "danger-full-access")'), true);
});

test("the optional image seat picks, clears, discards, saves and reloads as one flat seat", async () => {
  let saved = { version: 1, preset: "split" }, members;
  const app = advancedEditor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: saved, error: null, bosses: [], presets: { split: "rule", image_generation: "\nimage-rule" }, files: [] }) };
    }
    if (url === "/app/orchestration") saved = body;
    else members = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  });
  await app.run("loadOrchestration()");
  const row = () => app.run('$("orchestration-roles").children.at(-1)');
  const pick = (value) => row().children[1].listeners.change({ target: { value } });
  const seat = { provider: "codex", model: "chat-model" };

  assert.equal(row().children[0].children[0].children[0], "生图（可选）");
  assert.equal(row().children[0].children.length, 1);
  assert.equal(row().children[1].children[0].children[0], "未配置");
  assert.equal(row().children[2].attrs["aria-label"], "生图的对话模型");
  app.run('viewMode("off")');
  assert.equal(app.run('$("orchestration-roles").hidden'), false);
  assert.equal(app.run('$("orchestration-roles").children.length'), 1);
  assert.equal(row().children[0].children[0].children[0], "生图（可选）");
  app.run('viewMode("split")');

  assert.equal(app.run('orchestrationPresetText("split")'), "rule");
  pick("codex");
  assert.equal(app.run('orchestrationPresetText("split")'), "rule\nimage-rule");
  assert.equal(JSON.stringify(app.run("orchestrationWanted().image_generation")), JSON.stringify(seat));
  pick("");
  assert.equal(app.run('"image_generation" in orchestrationWanted()'), false);
  assert.equal(app.run('orchestrationPresetText("split")'), "rule");
  assert.equal(app.run("orchestrationChanged()"), false);
  pick("codex");
  app.run("discardAdvanced()");
  assert.equal(app.run("orchestrationChanged()"), false);

  pick("codex");
  await app.run("saveOrchestration()");
  assert.equal(JSON.stringify(saved.image_generation), JSON.stringify(seat));
  assert.ok(members.clis.codex.models["chat-model"]);
  await app.run("loadOrchestration()");
  assert.equal(row().children[2].children.find((option) => option.attrs.value === "chat-model").attrs.selected, "");

  pick("");
  await app.run("saveOrchestration()");
  assert.equal(Object.hasOwn(saved, "image_generation"), false);
  app.run('delete currentDoc.modes.task.codex; renderOrchestration()');
  assert.ok(row().children[1].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled),
    "orchestration can use a confirmed CLI even when solo tasks are off");
  app.run('delete currentDoc.modes.discussion.codex; renderOrchestration()');
  assert.equal(row().children[1].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled), false,
    "removing both scenes disables the CLI despite its historical model settings");
});

function rolesEditor(document, roles = JSON.parse(readFileSync(`${__dirname}/../../../../src/agentnave/orchestration_roles.json`, "utf8"))) {
  const state = { saved: document, members: null };
  const presets = {
    split: "whole", parallel: "whole+", image_generation: "\nimage", meetings: "H\nT",
    meetings_parts: { head: "H", tail: "\nT" },
    meeting_parts: {
      default_count: 3, separator: "、", seat_provider: "{name}<{provider}>",
      all: "\nA{count}:{seats}", some: "\nS{count}:{seats}", none: "\nN{count}",
    },
    role_base: { parallel: "base+" }, parallel_base: "base+",
    split_parts: { base: "base", meetings_asked: "\nasked", meetings_own: "\nown" },
    role_sections: Object.fromEntries((roles || []).map((role) => [role.group, `\n${role.group}`])),
  };
  const app = advancedEditor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: state.saved, error: null, bosses: [], presets, roles, files: [] }) };
    }
    if (url === "/app/orchestration") state.saved = body;
    else state.members = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: [] }) };
  });
  const sections = () => app.run('$("orchestration-roles").children').filter((box) => box.className === "orchestration-section");
  const groups = () => sections().flatMap((box) => box.children.slice(1));
  const group = (name) => groups().find((box) => box.children[0].children[0].children[1].children[0] === name);
  const toggle = (name, checked) => group(name).children[0].children[0].children[0].listeners.change({ target: { checked } });
  const split = (name, checked) => group(name).children[0].children[1].children[0].listeners.change({ target: { checked } });
  return { app, state, sections, groups, group, toggle, split };
}

test("custom editor tracks generated role and image lines without changing user text", async () => {
  const { app, state, toggle } = rolesEditor({ version: 1, preset: "custom", enabled_roles: [], custom: "worker_writing\n用户正文" });
  await app.run("loadOrchestration()");
  assert.equal(app.run('$("custom-text").value'), "用户正文");
  assert.equal(app.run("orchestrationChanged()"), false);
  toggle("写作", true);
  assert.equal(app.run('$("custom-text").value'), "用户正文\nworker_writing");
  toggle("写作", false);
  assert.equal(app.run('$("custom-text").value'), "用户正文");
  assert.equal(app.run("orchestrationChanged()"), false);
  app.run('setOrchestrationSeat("image_generation", null, {provider:"codex",model:"chat-model"})');
  assert.equal(app.run('$("custom-text").value'), "用户正文\nimage");
  await app.run("saveOrchestration()");
  assert.equal(state.saved.custom, "用户正文\nimage");
  await app.run("loadOrchestration()");
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("a failed reload keeps the loaded baseline, so an unsaved draft survives the next good reload", async () => {
  const { app, toggle } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  toggle("写作", true);
  assert.equal(app.run("orchestrationChanged()"), true);
  app.run("globalThis.goodFetch = fetch; fetch = async () => ({ ok: false, status: 500 })");
  await app.run("loadOrchestration()");
  assert.equal(app.run("orchestrationChanged()"), true);
  app.run("fetch = goodFetch");
  await app.run("loadOrchestration()");
  assert.equal(app.run("orchestrationChanged()"), true);
});

test("role switches default like old files, round-trip through save, and keep bindings when off", async () => {
  const binding = { provider: "codex", model: "chat-model" };
  const { app, state, groups, group, toggle } = rolesEditor({ version: 1, preset: "split", worker_writing: { easy: binding } });
  await app.run("loadOrchestration()");

  assert.equal(groups().length, 7);
  assert.equal(group("通用探索").children.length, 3);
  assert.equal(group("写作").children.length, 1, "a new role starts off with its seats out of sight");
  assert.equal(group("写作").children[0].children.length, 1, "and without its difficulty switch");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked\nexplorer\nworker\nworker_frontend");

  toggle("写作", true);
  assert.equal(group("写作").children.length, 3, "turning a role on shows its seats, split while only one is set");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked\nexplorer\nworker\nworker_frontend\nworker_writing");
  assert.equal(app.run("orchestrationChanged()"), true);
  toggle("写作", false);
  assert.equal(app.run("orchestrationChanged()"), false, "back to what is saved leaves nothing unsaved");
  assert.equal(app.run('"enabled_roles" in orchestrationWanted()'), false);

  for (const name of ["通用探索", "通用执行", "界面制作"]) toggle(name, false);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked");
  await app.run("saveOrchestration()");
  assert.deepEqual(state.saved.enabled_roles, []);
  assert.deepEqual(state.saved.worker_writing, { easy: binding }, "turning off keeps the binding");
  assert.equal(state.members, null, "a binding behind a switch stays out of the member file");

  await app.run("loadOrchestration()");
  assert.equal(group("通用探索").children.length, 1);
  toggle("写作", true);
  await app.run("saveOrchestration()");
  assert.ok(state.members.clis.codex.models["chat-model"], "turning a role on lists the binding it brings back");
  assert.deepEqual(state.saved.enabled_roles, ["worker_writing"]);
});

test("a global off shows only the image seat, and no catalog means no role is guessed", async () => {
  const off = rolesEditor({ version: 1, preset: "off" });
  await off.app.run("loadOrchestration()");
  assert.equal(off.app.run('$("orchestration-roles").children.length'), 1);
  off.app.run('viewMode("split")');
  assert.equal(off.groups().length, 7);

  const older = rolesEditor({ version: 1, preset: "split", enabled_roles: ["worker"] }, null);
  await older.app.run("loadOrchestration()");
  assert.equal(older.app.run('$("orchestration-roles").children.length'), 1);
  assert.equal(older.app.run("orchestrationChanged()"), false);
});

test("meetings only shows its preset text as is, no seat cards, and never counts as using seats", async () => {
  const { app } = rolesEditor({ version: 1, preset: "meetings", enabled_roles: ["worker"], image_generation: { provider: "codex", model: "gone" } });
  await app.run("loadOrchestration()");
  assert.equal(app.run('orchestrationPresetText("meetings")'), "H\nT");
  assert.equal(app.run('$("orchestration-text").textContent'), "H\nT");
  assert.equal(app.run('$("orchestration-roles").children.length'), 0);
  assert.equal(app.run('$("orchestration-split").hidden'), true);
});

test("meeting seats are picked one by one, shown in the three meeting presets, kept to the count, and clear to nothing", async () => {
  const { app } = rolesEditor({ version: 1, preset: "meetings", enabled_roles: [] });
  await app.run("loadOrchestration()");
  const rows = () => app.run('$("orchestration-meeting").children');
  const size = (n) => rows()[0].children[1].children[n - 2].listeners.click();
  const pick = (seat, value) => rows()[1 + seat].children[1].listeners.change({ target: { value } });
  const wanted = () => JSON.stringify(app.run("orchestrationWanted().meeting"));
  const preview = (mode) => app.run(`orchestrationPresetText("${mode}")`);
  const name = app.run("PROVIDER_NAMES.codex");
  const one = `${name}<codex>`;

  assert.equal(app.run('$("orchestration-meeting").hidden'), false);
  assert.equal(rows().length, 4);
  assert.equal(rows()[1].children[0].children[0], "席位 1");
  assert.equal(rows()[1].children[1].children[0].children[0], "由 boss 选");
  assert.equal(preview("meetings"), "H\nT");

  pick(0, "codex");
  assert.equal(wanted(), JSON.stringify({ count: 3, seats: [{ provider: "codex" }] }));
  assert.equal(rows()[1].children[2].attrs.disabled, "", "the model follows Employees and is read-only");
  assert.equal(rows()[1].children[3].attrs.disabled, "", "so does the effort");
  assert.equal(preview("meetings"), `H\nS3:${one}\nT`);
  app.run('viewMode("split")');
  assert.equal(app.run('$("orchestration-meeting").hidden'), false);
  assert.equal(preview("split"), `base\nasked\nS3:${one}`);
  app.run('viewMode("parallel")');
  assert.equal(preview("parallel"), `base+\nown\nS3:${one}`);
  app.run('viewMode("custom")');
  assert.equal(app.run('$("orchestration-meeting").hidden'), true);
  app.run('viewMode("meetings")');

  pick(1, "codex");
  size(2);
  assert.equal(rows().length, 3);
  assert.equal(preview("meetings"), `H\nA2:${one}、${one}\nT`);
  size(5);
  assert.equal(rows().length, 6);
  assert.equal(preview("meetings"), `H\nS5:${one}、${one}\nT`);
  pick(3, "codex");
  size(3);
  size(5);
  assert.equal(app.run("orchestrationWanted().meeting.seats.length"), 2, "seats past a smaller count are dropped and come back empty");

  pick(0, "");
  pick(1, "");
  size(4);
  assert.equal(wanted(), '{"count":4}');
  assert.equal(preview("meetings"), "H\nN4\nT");
  size(3);
  assert.equal(app.run("Object.hasOwn(orchestrationWanted(), 'meeting')"), false);
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("an older meeting seat with a model is read and previewed as its CLI alone", async () => {
  const { app } = rolesEditor({ version: 1, preset: "meetings", enabled_roles: [], meeting: { count: 2, seats: [{ provider: "codex", model: "gone", effort: "high" }, null] } });
  await app.run("loadOrchestration()");
  assert.equal(app.run('orchestrationPresetText("meetings")'), `H\nS2:${app.run("PROVIDER_NAMES.codex")}<codex>\nT`);
  assert.equal(app.run("seatConflicts().length"), 0);
});

test("general and specialist roles are grouped, and no role borrows another role's seat", async () => {
  const outside = { provider: "codex", model: "gone" };
  const worker = { provider: "codex", model: "chat-model" };
  const { app, sections, group, toggle } = rolesEditor({
    version: 1, preset: "split", enabled_roles: ["worker", "worker_frontend", "worker_writing"],
    worker: { easy: worker }, explorer_web: { hard: outside },
  });
  await app.run("loadOrchestration()");
  app.run('currentDoc.clis.codex = { allowed_models: ["chat-model"], models: {} }');

  const title = (box) => box.children[0].children[0];
  const name = (box) => box.children[0].children[0].children[1].children[0];
  assert.deepEqual(Array.from(sections(), title), ["通用", "专项"]);
  assert.deepEqual(Array.from(sections()[0].children.slice(1), name), ["通用探索", "通用执行"]);
  assert.equal(sections()[1].children.slice(1).map(name).includes("界面制作"), true);
  const unset = (row) => row.children[1].children[0].children[0];
  assert.equal(group("通用执行").children[0].children.length, 1, "general roles have no difficulty switch");
  assert.equal(group("界面制作").children.length, 2, "an unset specialist shows one shared row");
  assert.equal(unset(group("界面制作").children[1]), "未配置", "the interface role no longer borrows");

  assert.equal(app.run("seatConflicts().length"), 0, "a role turned off keeps a stale binding quietly");
  toggle("网页调研", true);
  assert.equal(app.run("seatConflicts().length"), 1);
  assert.match(app.run("seatConflictText(seatConflicts()[0])"), /网页调研 · 困难 用的 gone/);
  app.run("renderAdvancedBar()");
  assert.equal(app.run('$("advanced-save").disabled'), true, "and blocks saving until rebound, without repairing it");
  assert.deepEqual(JSON.parse(app.run("JSON.stringify(orchestrationWanted().explorer_web)")), { hard: outside });
});

test("a specialist picks one seat for both difficulties until it is split", async () => {
  const { app, group, split } = rolesEditor({ version: 1, preset: "split", enabled_roles: ["worker_writing"] });
  await app.run("loadOrchestration()");
  const seats = () => JSON.parse(app.run("JSON.stringify(orchestrationWanted().worker_writing ?? null)"));
  const pick = (row, value) => group("写作").children[row].children[1].listeners.change({ target: { value } });
  const shared = { provider: "codex", model: "chat-model" };

  pick(1, "codex");
  assert.deepEqual(seats(), { easy: shared, hard: shared });
  assert.equal(group("写作").children.length, 2, "the same seat on both stays one row");

  split("写作", true);
  assert.equal(group("写作").children.length, 3);
  assert.equal(app.run("orchestrationChanged()"), true);
  pick(1, "");
  assert.deepEqual(seats(), { hard: shared });
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "joining keeps the hard seat");
  assert.equal(group("写作").children.length, 2);

  split("写作", true);
  pick(2, "");
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "or the easy one when hard is unset");
  pick(1, "");
  assert.equal(seats(), null);
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("one review model fills only the review roles left unset, and follows until saved", async () => {
  const app = advancedEditor();
  app.run('reviewSeats = { "security-reviewer": { provider: "codex", model: "own" } }; renderReviewSeats()');
  const shown = () => JSON.parse(app.run("JSON.stringify(seatsShown())"));
  const pick = (value) => app.run('$("review-seats").children[0]').children[1].listeners.change({ target: { value } });
  const uniform = { provider: "codex", model: "chat-model" };

  assert.equal(app.run('$("review-seats").children[0].children[0].children[0].children[0]'), "统一评审模型");
  pick("codex");
  assert.deepEqual(shown()["security-reviewer"], { provider: "codex", model: "own" }, "a role set on its own is kept");
  assert.deepEqual(shown()["correctness-reviewer"], uniform);
  assert.equal(Object.hasOwn(shown(), "correctness-reviewer-easy"), false, "unset, it already follows correctness");
  app.run('setReviewSeat("architecture-reviewer", { provider: "codex", model: "picked" })');
  pick("");
  assert.deepEqual(Object.keys(shown()).sort(), ["architecture-reviewer", "security-reviewer"]);
  app.run("discardAdvanced()");
});

test("editing a full list keeps both scenes explicit and newly detected CLIs off", () => {
  const app = editor();
  app.run('setProviderEnabled("codex", false); setProviderEnabled("codex", true)');
  app.run('detected.claude = { installed: true }');
  assert.equal(app.run('slot("task", "claude").enabled'), false);
  assert.equal(app.run('slot("discussion", "claude").enabled'), false);
  app.run('setProviderEnabled("claude", true)');
  for (const scene of ["task", "discussion"]) {
    assert.deepEqual(Object.keys(app.doc().modes[scene]).sort(), ["claude", "codex"]);
  }
  assert.equal(Object.hasOwn(app.doc(), "defaults"), false);
});

test("saving handwritten open scenes moves defaults into the explicit CLI lists", async () => {
  let sent;
  const app = editor(async (_url, options) => {
    sent = JSON.parse(options.body);
    return { ok: true, json: async () => ({ saved: true }) };
  });
  app.run(`currentDoc = {
    version: 1, clis: { grok: { models: { saved: {} } } }, modes: {},
    defaults: { task: { grok: "saved" } }
  }`);
  assert.equal(app.run('slot("task", "grok").model'), "saved");
  await app.run("apiPostMembers(currentDoc)");
  assert.deepEqual(sent.modes.task, {
    codex: { models: [], overrides: {} },
    grok: { models: [], overrides: {}, default: "saved" },
  });
  assert.deepEqual(sent.modes.discussion, { codex: { models: [], overrides: {} } });
  assert.equal(Object.hasOwn(sent, "defaults"), false);
});

test("host events reload clean members and preserve edits made before or during the read", async () => {
  let answer, requests = 0;
  const app = editor(() => {
    requests++;
    return new Promise((resolve) => { answer = (document) => resolve({ ok: true, json: async () => ({ document }) }); });
  });
  const hosts = (at) => ({ detail: { hosts: [{ id: "codex", status: "connected", last_action: { at } }] } });
  const reloaded = app.listeners["agentnave:hosts"](hosts("1"));
  const document = app.doc();
  document.modes.task.claude = { models: [], overrides: {} };
  answer(document);
  await reloaded;
  assert.equal(app.run('slot("task", "claude").enabled'), true);

  const racing = app.listeners["agentnave:hosts"](hosts("2"));
  app.run('setProviderEnabled("codex", false)');
  answer(document);
  await racing;
  assert.equal(app.run('slot("task", "codex").enabled'), false);
  // With edits unsaved a later event reads again, yet only merges CLIs the file gained.
  const dirtyRead = app.listeners["agentnave:hosts"](hosts("3"));
  answer(document);
  await dirtyRead;
  assert.equal(requests, 3);
  assert.equal(app.run('slot("task", "codex").enabled'), false);
});

test("a connection made while employee edits are unsaved joins them instead of being lost", async () => {
  // The second start is a handwritten file whose scenes are still open.
  for (const open of [false, true]) {
    let file;
    const app = editor(async () => ({ ok: true, json: async () => ({ document: file }) }));
    if (open) app.run("currentDoc = { version: 1, clis: {}, modes: {} }; markSaved();");
    app.run('setProviderEnabled("codex", false)');
    file = { version: 1, clis: {}, modes: {} };
    for (const scene of ["task", "discussion"]) {
      file.modes[scene] = { codex: { models: [], overrides: {} }, grok: { models: [], overrides: {} } };
    }
    await app.listeners["agentnave:hosts"]({ detail: { hosts: [{ id: "grok", status: "connected", last_action: { at: "1" } }] } });
    assert.equal(app.run('slot("task", "grok").enabled'), true);
    assert.equal(app.run('slot("task", "codex").enabled'), false);
    assert.equal(app.run("dirty"), true);
  }
});

test("turning a CLI off and on again leaves nothing unsaved", () => {
  const app = editor();
  app.run(`
    detected.claude = { installed: true };
    for (const scene of SCENE_IDS) currentDoc.modes[scene] = { codex: { models: [], overrides: {} }, claude: { models: [], overrides: {} } };
    markSaved();
    setProviderEnabled("codex", false, true);
    setProviderEnabled("codex", true);
  `);
  assert.equal(app.run("dirty"), false);
});

test("a connection published while the members file is being read is read again later", async () => {
  const answers = [];
  const app = editor(() => new Promise((resolve) => answers.push((document) => resolve({ ok: true, json: async () => ({ document }) }))));
  const hosts = (...ids) => ({ detail: { hosts: ids.map((id) => ({ id, status: "connected", last_action: { at: "1" } })) } });
  const file = app.doc();
  const first = app.listeners["agentnave:hosts"](hosts("codex"));
  await app.listeners["agentnave:hosts"](hosts("codex", "grok"));
  answers.shift()(file);
  // No further publication: the first read itself reads again for the hosts it missed.
  await new Promise(setImmediate);
  for (const scene of ["task", "discussion"]) file.modes[scene].grok = { models: [], overrides: {} };
  answers.shift()(file);
  await first;
  await new Promise(setImmediate);
  assert.equal(app.run('slot("task", "grok").enabled'), true);
});

test("a saved employee without an installed CLI stays known until its removal is saved", () => {
  const app = editor();
  app.run(`
    currentDoc.modes.task.claude = { models: [], overrides: {} };
    markSaved();
    setProviderEnabled('claude', false, true);
    let lastSaved = confirmScenes(JSON.parse(savedSnapshot));
  `);
  assert.equal(app.run("known('claude', lastSaved)"), true);
  assert.equal(app.run("slot('task', 'claude').enabled"), false);
  app.run("markSaved(); lastSaved = confirmScenes(JSON.parse(savedSnapshot))");
  assert.equal(app.run("known('claude', lastSaved)"), false);
});

function runtimeEditor(fetch, os) {
  const app = editor(fetch, os);
  // Plain nodes, installed before the sections that take them from AN.settings are loaded.
  app.run(`
    AN.settings.el = (tag, attrs, ...children) => ({ tag, attrs, children });
    AN.settings.textNode = (s) => s;
    AN.settings.settingTip = () => AN.settings.el("span");
  `);
  app.load("member-cards.js");
  app.run("renderCards = () => {}");
  return app;
}

test("runtime switches show only where each provider supports them", async () => {
  const app = runtimeEditor(async () => ({ ok: true, json: async () => ({ settings: { version: 1, max_running: 8 } }) }));
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.disabled"), true);
  await app.run("loadLimit()");
  assert.equal(app.run("renderInteractive('codex')"), null);
  assert.equal(app.run("renderInteractive('antigravity').children[0].children[0].attrs.checked"), false);
  const row = app.run("renderInteractive('claude')");
  assert.equal(row.attrs.className, "member-row member-runtime");
  const label = row.children[0], input = label.children[0];
  assert.equal(label.tag, "label");
  assert.equal(label.children[1], "交互模式运行");
  assert.equal(input.attrs.checked, false);
  assert.equal(input.attrs.disabled, false);
  const windows = runtimeEditor(undefined, "windows");
  assert.equal(windows.run("renderInteractive('claude').attrs.className"), "member-row member-runtime");
  assert.equal(windows.run("renderInteractive('antigravity')"), null);
});

test("runtime saves immediately without changing other settings or the unsaved limit", async () => {
  const calls = [];
  const app = runtimeEditor(async (url, options) => {
    assert.equal(url, "/app/settings");
    if (options.method === "POST") {
      calls.push(JSON.parse(options.body));
      assert.equal(options.headers["X-AgentNave-Action"], "1");
      return { ok: true, json: async () => ({ saved: true }) };
    }
    return { ok: true, json: async () => ({ settings: { version: 1, max_running: 8, history_retention_days: 90, claude_interactive: true } }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.checked"), true);
  app.run("$('max-running').value = '3'; refreshDirty()");
  await app.run("renderInteractive('claude').children[0].children[0].attrs.onChange({ target: { checked: false } })");
  assert.deepEqual(calls, [{ version: 1, claude_interactive: false }]);
  await app.run("renderInteractive('antigravity').children[0].children[0].attrs.onChange({ target: { checked: true } })");
  assert.deepEqual(calls[1], { version: 1, antigravity_interactive: true });
  calls.pop();
  assert.equal(app.run("limitChanged()"), true);
  assert.equal(app.run("dirty"), true);
  assert.equal(app.run("limitDoc.history_retention_days"), 90);
  await app.run("saveLimit()");
  assert.deepEqual(calls[1], { version: 1, max_running: 3 });
  assert.equal(app.run("limitDoc.claude_interactive"), false);
});

test("failed writes restore the runtime switch and report preview refusals", async () => {
  let finish;
  const app = runtimeEditor(() => new Promise(resolve => { finish = resolve; }));
  app.run("limitDoc = { version: 1, max_running: 8, claude_interactive: false }");
  const saving = app.run("setInteractive('claude_interactive', true)");
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.disabled"), true);
  finish({ ok: false, json: async () => ({ error: "预览版不修改正式数据" }) });
  await saving;
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.checked"), false);
  assert.equal(app.run("busy"), false);
  assert.match(app.run("$('notice').textContent"), /预览版不修改正式数据/);
});

test("broken settings keep the switch disabled until the existing settings save repairs them", async () => {
  let posts = 0;
  const app = runtimeEditor(async (_url, options) => {
    if (options.method === "POST") {
      posts++;
      const payload = JSON.parse(options.body);
      assert.equal(payload.claude_interactive, true);
      return { ok: true, json: async () => ({ saved: true }) };
    }
    return { ok: true, json: async () => ({ settings: { version: 1, max_running: 8, claude_interactive: true }, error: "claude_interactive 必须为布尔值" }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.disabled"), true);
  await app.run("setInteractive('claude_interactive', true)");
  assert.equal(posts, 0);
  assert.match(app.run("$('notice').textContent"), /claude_interactive/);
  app.run("$('max-running').value = '3'");
  await app.run("saveLimit()");
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.disabled"), false);
});

test("a settings read started before a runtime save cannot restore its old value", async () => {
  let finish;
  const app = runtimeEditor((_url, options) => options.method === "POST"
    ? Promise.resolve({ ok: true, json: async () => ({ saved: true }) })
    : new Promise(resolve => { finish = resolve; }));
  app.run("limitDoc = { version: 1, max_running: 8, claude_interactive: false }");
  const loading = app.run("loadLimit()");
  await app.run("setInteractive('claude_interactive', true)");
  finish({ ok: true, json: async () => ({ settings: { version: 1, max_running: 8, claude_interactive: false } }) });
  await loading;
  assert.equal(app.run("limitDoc.claude_interactive"), true);
});


test("a failed settings read cannot make a limit save reset other runtime settings", async () => {
  let sent;
  const app = runtimeEditor(async (_url, options) => {
    if (options.method !== "POST") throw new Error("offline");
    sent = JSON.parse(options.body);
    return { ok: true, json: async () => ({ saved: true }) };
  });
  await app.run("loadLimit()");
  assert.equal(app.run("renderInteractive('claude').children[0].children[0].attrs.disabled"), true);
  app.run("$('max-running').value = '3'");
  await app.run("saveLimit()");
  assert.deepEqual(sent, { version: 1, max_running: 3 });
});

function poolEditor(fetch) {
  const app = runtimeEditor(fetch);
  app.run("AN.L = (zh) => zh");
  app.load("advanced.js");
  app.run(`
    function findKey(nodes, key) {
      for (const node of nodes || []) {
        if (!node || typeof node !== "object") continue;
        if (node.attrs?.dataset?.focusKey === key) return node;
        const found = findKey(node.children, key);
        if (found) return found;
      }
      return null;
    }
    const offered = (rows, key) => findKey(rows, key).children.flat()
      .filter((option) => !option.attrs.disabled).map((option) => option.attrs.value);
    const seatOptions = (seat) => seatSelects("seat", "工位", seat, () => {})[1].children.flat();
    const poolPick = (key, checked) => {
      pickerOpen = "codex/pool";
      const action = findKey(poolRows("codex"), key).attrs;
      if (action.onClick) action.onClick();
      else action.onChange({ target: { checked } });
    };
    detected.codex = { installed: true, native: { model: "a" }, models: [{ id: "a" }, { id: "b" }] };
    currentDoc.clis.codex = { models: { old: {} } };
    markSaved();
  `);
  return app;
}

test("one CLI pool bounds solo tasks, meetings and seats and lets no left-out model through", async () => {
  const posts = [];
  const app = poolEditor(async (url, options) => {
    posts.push(url);
    return { ok: true, json: async () => ({ saved: true, document: JSON.parse(options.body) }) };
  });
  app.run(`
    currentDoc.clis.codex.models.b = {};
    currentDoc.modes.discussion.codex.default = "b";
    reviewSeats = { "security-reviewer": { provider: "codex", model: "b" } };
    markSaved();
    pickerOpen = "codex/pool";
  `);
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/allow/old")'), null, "a saved model the CLI no longer lists is not offered");
  assert.equal(app.run('seatOptions({ provider: "codex", model: "old" }).find((o) => o.attrs.value === "old").attrs.disabled'), true, "a seat bound to it shows it as unavailable");
  app.run(`
    poolPick("codex/pool/all");
    poolPick("codex/pool/allow/b", false);
  `);
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a"]);
  assert.deepEqual(app.doc().modes.task.codex.models, []);
  assert.equal(app.doc().modes.task.codex.default, "a");

  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "task", true), "codex/task/model"))')).slice(1), ["a"]);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "discussion", true), "codex/discussion/model"))')).slice(1), ["a"]);

  // The default and the seat outside the pool stay as saved and are shown as conflicts.
  assert.equal(app.doc().modes.discussion.codex.default, "b");
  assert.equal(app.run('findKey(slotRows("codex", "discussion", true), "codex/discussion/model").children.flat().find((o) => o.attrs.value === "b").attrs.disabled'), true);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "b" }).find((o) => o.attrs.value === "b").attrs.disabled'), true);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(seatOptions({ provider: "codex", model: "a" }).map((o) => o.attrs.value))')), ["a"]);
  assert.equal(app.run('poolConflicts("codex").length'), 2);
  await app.run("saveMembers()");
  assert.deepEqual(posts, []);
  app.run('changeSlot("codex", "discussion", { model: "a" })');
  assert.match(app.run("seatConflictText(seatConflicts()[0])"), /安全评审 用的 b/);
  assert.equal(app.run("memberSaveErrors().length"), 0);
  await app.run("saveMembers()");
  assert.deepEqual(posts, ["/app/members"], "the pool can be saved before rebinding a conflicting seat");

  // A role cannot widen the pool: the saved list refuses the seat before anything is written.
  app.run('reviewSeats = {}; markSaved()');
  assert.equal(app.run("memberSaveErrors().length"), 0);
  await assert.rejects(app.run('listSeatModel({ provider: "codex", model: "b" })'), /不在 Codex CLI 的可用模型里/);
  assert.deepEqual(posts, ["/app/members"]);

  // Select all is an explicit list; future CLI models are not silently allowed.
  app.run('poolPick("codex/pool/all"); detected.codex.models.push({ id: "next" })');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a", "b"]);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "a" }).some((o) => o.attrs.value === "next")'), false);
  app.run(`
    currentDoc.clis.codex.allowed_models = ["a"];
    orchestration = { document: { version: 1, preset: "off", image_generation: { provider: "codex", model: "b" } } };
  `);
  assert.equal(app.run("seatConflicts().length"), 0, "inactive image bindings do not block model-pool edits");
  app.run('orchestration.document.preset = "split"');
  assert.equal(app.run("seatConflicts().length"), 1, "enabling orchestration restores the binding conflict");
  app.run('orchestration.document.preset = "meetings"');
  assert.equal(app.run("seatConflicts().length"), 0, "meetings only uses no seats");
});

test("a model the CLI no longer lists is marked but blocks no save, and nothing is marked without a listing", () => {
  const app = poolEditor();
  app.run(`
    currentDoc.modes.task.codex.default = "old";
    reviewSeats = { "security-reviewer": { provider: "codex", model: "old" } };
    detected.codex.native = { model: "gone" };
    markSaved();
  `);
  assert.equal(app.run("configurationErrors().length"), 0, "an unlisted default does not block saving");
  assert.equal(app.run('poolConflicts("codex").length'), 2, "both bindings are named on the card");
  assert.equal(app.run('seatConflicts().some((c) => c.blocking)'), false);
  assert.equal(app.run('nativeDefault("codex").model'), null, "a CLI default its listing lacks is not offered");
  assert.equal(app.run('findKey(slotRows("codex", "discussion", true), "codex/discussion/model").children.flat().some((o) => o.attrs.value === "")'), true, "the scene can still follow the CLI");

  app.run('detected.codex.models = []');
  assert.equal(app.run('poolConflicts("codex").length'), 0, "a failed or pending detection marks nothing");
  assert.deepEqual(Array.from(app.run('getAllModelIds("codex")')), ["old"]);
});

test("model choices start unchecked, clear all stays unsaved, and no list size is refused", () => {
  const app = poolEditor();
  app.run(`
    function poolText(nodes) {
      return (nodes || []).flat(Infinity).flatMap((node) => typeof node === "string" ? [node]
        : node && typeof node === "object" ? poolText(node.children) : []);
    }
    detected.codex = { installed: true, native: { model: "model-0" },
      models: Array.from({ length: 24 }, (_, i) => ({ id: "model-" + i })) };
    currentDoc.clis.codex.models = {};
    markSaved();
  `);
  assert.equal(app.run('poolSummary("codex", getAllModelIds("codex"), shownPool("codex"))'), "不限模型");
  app.run('pickerOpen = "codex/pool"');
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/allow/model-0").attrs.checked'), false);
  app.run('poolPick("codex/pool/all")');
  assert.equal(app.doc().clis.codex.allowed_models.length, 24, "every listed model is selectable");
  assert.equal(app.run("memberSaveErrors().length"), 0);
  const text = Array.from(app.run('poolText(poolRows("codex"))'));
  assert.ok(text.includes("已选 24"), "the picker counts what is chosen");
  assert.ok(text.includes("建议每个 CLI 不超过 10 个模型"), "a smaller list is only a suggestion");
  app.run('poolPick("codex/pool/none")');
  assert.deepEqual(app.doc().clis.codex.allowed_models, []);
  assert.match(app.run('configurationErrors()[0].message'), /至少选一个模型/);
  app.run('poolPick("codex/pool/allow/model-0", true)');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["model-0"]);
  app.run('poolPick("codex/pool/allow/model-0", false)');
  assert.deepEqual(app.doc().clis.codex.allowed_models, []);

  // A saved list still shrinks one model at a time.
  app.run('currentDoc.clis.codex.allowed_models = Array.from({ length: 24 }, (_, i) => `model-${i}`); markSaved()');
  app.run('poolPick("codex/pool/allow/model-20", false)');
  assert.equal(app.doc().clis.codex.allowed_models.length, 23);
});

test("older scene lists keep working until the pool is edited, and then stop narrowing it", () => {
  const app = poolEditor();
  app.run(`
    currentDoc.clis.codex.models = { a: {}, b: {} };
    currentDoc.modes.task.codex = { models: ["a"], overrides: {}, default: "a" };
    currentDoc.modes.discussion.codex = { models: ["a", "b"], overrides: {}, default: "b" };
    reviewSeats = { "security-reviewer": { provider: "codex", model: "b" } };
    markSaved();
  `);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(slot("task", "codex").allowed)')), ["a"]);
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(shownPool("codex"))')), ["a", "b"]);
  assert.equal(app.run('poolSummary("codex", ["a", "b"], shownPool("codex"))'), "单独干活 1 个、开会 2 个");
  assert.equal(app.run("memberSaveErrors().length"), 0);

  app.run('detected.codex.models.push({ id: "c" }); poolPick("codex/pool/allow/c", true)');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a", "b", "c"]);
  assert.deepEqual(app.doc().modes.task.codex, { models: [], overrides: {}, default: "a" });
  assert.deepEqual(JSON.parse(app.run('JSON.stringify(offered(slotRows("codex", "task", true), "codex/task/model"))')).slice(1), ["a", "b", "c"]);
  assert.ok(app.doc().clis.codex.models.c);
});

test("each boss is named by its own saved orchestration mode", () => {
  const app = poolEditor();
  app.run(`orchestration = { error: null, files: [], document: { version: 1, preset: "off", apps: { codex: "split" } }, bosses: [
    { keys: ["codex"], labels: ["Codex CLI"] }, { keys: ["grok"], labels: ["Grok CLI"] },
  ] }; renderTaskScope();`);
  assert.equal(app.run('$("task-scope").hidden'), false);
  assert.match(app.run('$("task-scope").textContent'), /^Codex CLI 已启用编排.*单独干活只提供给未启用编排的 boss/);
  app.run('orchestration.document.apps = {}; renderTaskScope()');
  assert.equal(app.run('$("task-scope").hidden'), true);
});

test("split work lets the boss call meetings only when its switch is on", async () => {
  const { app } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nasked");
  const meetings = () => app.run('$("orchestration-split").children[0].children[0]');
  meetings().listeners.change({ target: { checked: true } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().split_options)"), '{"meetings":true}');
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nown");
  meetings().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationWanted().split_options"), undefined);
  app.run('viewMode("parallel")');
  assert.equal(app.run('$("orchestration-split").children.length'), 2);
  assert.equal(app.run('orchestrationPresetText("parallel")'), "base+\nown");
  meetings().listeners.change({ target: { checked: false } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().parallel_options)"), '{"meetings":false}');
  assert.equal(app.run('orchestrationPresetText("parallel")'), "base+\nasked");
  meetings().listeners.change({ target: { checked: true } });
  assert.equal(app.run("orchestrationWanted().parallel_options"), undefined);
});

test("the orchestration-only switch rewrites its line in split and parallel only", async () => {
  const { app } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  app.run(`orchestration.presets.split_parts.base = "base\\nlead-asked";
    orchestration.presets.parallel_base = "par\\nlead-asked";
    orchestration.presets.lead = { asked: "lead-asked", always: "lead-always" }; renderOrchestration()`);
  const lead = () => app.run('$("orchestration-split").children[1].children[0]');
  lead().listeners.change({ target: { checked: true } });
  assert.equal(app.run("orchestrationWanted().lead_only"), true);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nlead-always\nasked");
  assert.equal(app.run('orchestrationPresetText("parallel")'), "par\nlead-always\nown");
  app.run('viewMode("custom")');
  assert.equal(app.run('$("orchestration-split").hidden'), true);
  app.run('viewMode("split")');
  lead().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationWanted().lead_only"), undefined);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nlead-asked\nasked");
});

test("a switch put back to what is saved keeps the saved form of its key", async () => {
  const saved = { version: 1, preset: "split", enabled_roles: [], lead_only: false,
    split_options: { meetings: false }, parallel_options: {} };
  const { app } = rolesEditor(saved);
  await app.run("loadOrchestration()");
  const [meetings, lead] = [0, 1].map((i) => () => app.run(`$("orchestration-split").children[${i}].children[0]`));
  for (const [switchOf, key] of [[meetings, "split_options"], [lead, "lead_only"]]) {
    switchOf().listeners.change({ target: { checked: true } });
    assert.equal(app.run("orchestrationChanged()"), true);
    switchOf().listeners.change({ target: { checked: false } });
    assert.deepEqual(JSON.parse(app.run(`JSON.stringify(orchestrationWanted()["${key}"])`)), saved[key]);
    assert.equal(app.run("orchestrationChanged()"), false);
  }
  app.run('viewMode("parallel")');
  meetings().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationChanged()"), true);
  meetings().listeners.change({ target: { checked: true } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().parallel_options)"), "{}");
  assert.equal(app.run("orchestrationChanged()"), false);
});

// The decision mode: its page, options, the first catalog and the key, on the same fake DOM.
function decisionEditor(preset, loaded, catalogError = null) {
  const calls = [];
  const app = advancedEditor(async (url, options = {}) => {
    calls.push({ url, method: options.method || "GET", body: options.body ? JSON.parse(options.body) : null });
    if (url === "/app/decision-seats" && options.method === "POST" && catalogError) return { ok: false, status: 500, json: async () => ({ error: catalogError }) };
    if (url === "/app/decision-seats" && !options.method) return { ok: true, json: async () => loaded };
    if (url === "/app/decision-key") return { ok: true, json: async () => ({ key_set: JSON.parse(options.body).key !== "" }) };
    return { ok: true, json: async () => ({ saved: true, files: [], bosses: JSON.parse(app.run("JSON.stringify(orchestration.bosses)")) }) };
  });
  const seat = (provider, model, effort) => ({ provider, model, ...(effort ? { effort } : {}) });
  app.run(`
    orchestration = { error: null, files: [], bosses: [{ keys: ["codex"], ids: ["codex"], labels: ["Codex"], mark: "codex" }],
      roles: [
        { group: "explorer", name_zh: "通用探索", name_en: "General exploration", scope_zh: "读代码", access: "read", fallback_group: null, legacy_enabled: true },
        { group: "worker", name_zh: "通用执行", name_en: "General execution", scope_zh: "改代码", access: "write", fallback_group: null, legacy_enabled: true },
        { group: "worker_frontend", name_zh: "界面制作", name_en: "Interface work", scope_zh: "做界面", access: "write", fallback_group: "worker", legacy_enabled: true },
        { group: "worker_writing", name_zh: "写作", name_en: "Writing", scope_zh: "写文章", access: "write", fallback_group: "worker", legacy_enabled: false },
      ],
      document: { version: 1, preset: "${preset}",
        enabled_roles: ["explorer", "worker", "worker_frontend", "worker_writing"],
        explorer: { easy: ${JSON.stringify(seat("codex", "chat-model"))}, hard: ${JSON.stringify(seat("codex", "chat-model", "high"))} },
        worker: { easy: ${JSON.stringify(seat("codex", "chat-model"))}, hard: ${JSON.stringify(seat("codex", "chat-model"))} },
        worker_frontend: { easy: ${JSON.stringify(seat("codex", "chat-model"))}, hard: ${JSON.stringify(seat("codex", "chat-model"))} },
        worker_writing: { easy: ${JSON.stringify(seat("codex", "chat-model"))} },
        image_generation: ${JSON.stringify(seat("codex", "chat-model"))} } };
  `);
  const decision = (code) => app.run(`(${code})(window.AgentNaveDecisionSeats)`);
  app.load("decision-seats.js");
  app.run(`orchestration.presets = {
    decision: "decision", image_generation: "|image", lead: { asked: "LEAD-ASKED", always: "LEAD-ALWAYS" },
    decision_parts: { base: "BASE|LEAD-ASKED|", parallel_writes_asked: "PW-ASKED|", parallel_writes_own: "PW-OWN|",
      meetings_asked: "M-ASKED|", meetings_own: "M-OWN|" } };`);
  app.run(`viewMode("${preset}")`);
  return { app, calls, decision };
}
const roleChildren = (app) => app.run('$("orchestration-roles")').children;
const NO_FILE = { document: null, error: null, key_set: false, key_error: null };

const boss = (app) => app.run("AN.settings.orchestration.bosses[0]");

const seatsOf = (app) => JSON.parse(app.run("JSON.stringify(window.AgentNaveDecisionSeats.document().seats)"));
const cardsOf = (app) => roleChildren(app)[0].children[3].children;

test("a boss put into the decision mode makes the first catalog from the fixed seats once", async () => {
  const { app, decision } = decisionEditor("split", NO_FILE);
  await decision((d) => d.load());
  app.run('viewMode("decision")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  const seats = seatsOf(app);
  // One card per bound group with easy and hard filled in and medium empty, then the image seat.
  assert.deepEqual(seats.map(({ kind, role, description, access, difficulty }) => [kind, role, description, access, difficulty]), [
    ["explorer", "通用探索", "读代码", "read", "easy"],
    ["explorer", "通用探索", "读代码", "read", "hard"],
    ["worker", "通用执行", "改代码", "write", "easy"],
    ["worker", "通用执行", "改代码", "write", "hard"],
    ["worker_frontend", "界面制作", "做界面", "write", "easy"],
    ["worker_frontend", "界面制作", "做界面", "write", "hard"],
    ["worker_writing", "写作", "写文章", "write", "easy"],
    ["image", "通用生图", "通用生图", "write", undefined],
  ]);
  assert.equal(seats[1].effort, "high");
  assert.ok(!("difficulty" in seats[7]));
  assert.equal(new Set(seats.map((seat) => seat.id)).size, 8);
  assert.ok(seats.every((seat) => /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(seat.id) && !("tags" in seat)));
  assert.equal(cardsOf(app).length, 5);
  assert.equal(decision((d) => d.changed()), true);
  assert.equal(decision((d) => d.problem()), null);
  // Leaving the mode keeps the catalog, and coming back does not make another one.
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "off")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  assert.equal(seatsOf(app).length, 8);
});

test("a saved file is never prefilled again, and a boss already on the mode does not prefill", async () => {
  const saved = { document: { version: 1, seats: [] }, error: null, key_set: false, key_error: null };
  const { app, decision } = decisionEditor("split", saved);
  await decision((d) => d.load());
  app.run('viewMode("decision")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  assert.equal(seatsOf(app).length, 0);
  assert.equal(decision((d) => d.changed()), false);

  // Opening the page with the boss on the mode makes nothing either.
  const open = decisionEditor("decision", NO_FILE);
  await open.decision((d) => d.load());
  assert.equal(seatsOf(open.app).length, 0);
  assert.equal(open.decision((d) => d.changed()), false);
});

test("only the decision mode's page shows the catalog, its options and the key, in place of the fixed seats and the fixed image seat", async () => {
  const { app, decision } = decisionEditor("split", NO_FILE);
  await decision((d) => d.load());
  const kinds = () => Array.from(roleChildren(app)).map((child) => child.className);
  const options = () => app.run('$("orchestration-split")').children;
  const fixed = ["orchestration-section", "orchestration-section", "review-row"];
  app.run('$("custom-text").value = "我的规则"');
  // Another mode's page is what it was: fixed seats, its own two switches, nothing of the catalog.
  assert.deepEqual(kinds(), fixed);
  assert.equal(options().length, 2);
  assert.equal(decision((d) => d.panel() === null), false);
  for (const mode of ["off", "meetings", "parallel", "custom"]) {
    app.run(`viewMode("${mode}")`);
    assert.ok(!kinds().some((kind) => /decision/.test(kind)), mode);
  }
  app.run('viewMode("decision")');
  // The catalog sits where the fixed seats were, and the fixed image seat is gone; three switches.
  assert.deepEqual(kinds(), ["orchestration-section decision-seats"]);
  assert.equal(options().length, 3);
  assert.equal(roleChildren(app)[0].children[0].tagName, "figure");
  // Under the picture, four who-does-what captions and one line saying what the decision model never does.
  const figure = roleChildren(app)[0].children[0];
  const [image, captions, footnote] = figure.children;
  assert.equal(captions.children.length, 4);
  assert.ok(captions.children.every((cell) => cell.children[0].tagName === "strong" && cell.children[1].tagName === "span"));
  const text = JSON.stringify(captions);
  for (const part of ["boss", "写好任务，交给服务", "筛出现在能派的工位", "决策模型", "只回答选哪个工位", "启动选中的工位，把选了谁告诉 boss"]) assert.ok(text.includes(part), part);
  assert.match(JSON.stringify(footnote), /决策模型不接触 CLI，也不改任务内容；任务跑完后，boss 照常自己去收结果。/);
  assert.equal(image.tagName, "img");
  assert.match(JSON.stringify(roleChildren(app)[0].children[1]), /发给 Google/);
  assert.match(app.run('$("orchestration-effect")').textContent, /决策模型/);
  app.run('viewMode("split")');
  assert.deepEqual(kinds(), fixed);
});

test("cards: a group once, custom as often as wanted, one image card with a row for each specialty", async () => {
  const { app, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const add = (what) => app.run(`window.AgentNaveDecisionSeats.addCard(${JSON.stringify(what)})`);
  const pick = (row, provider) => row.children[1].listeners.change({ target: { value: provider } });
  const fill = (input, text) => input.listeners.input({ target: { value: text } });
  add("worker_writing");
  add("worker_writing");
  add("custom");
  add("image");
  add("image");
  add("gardening");
  assert.equal(cardsOf(app).length, 3);
  // The menu lists general seats, specialist seats, custom seats and image seats; a group or the image
  // card already on the page is disabled, custom stays open.
  const menu = roleChildren(app)[0].children[4];
  const groups = menu.children.filter((child) => child.tagName === "optgroup");
  assert.deepEqual(Array.from(groups.map((one) => one.attrs.label)), ["通用工位", "专业工位", "自定义工位", "生图工位"]);
  const option = (value) => groups.flatMap((one) => one.children).find((one) => one.attrs.value === value);
  assert.deepEqual(Array.from(groups.map((one) => Array.from(one.children.map((child) => child.attrs.value)))), [["explorer", "worker"], ["worker_frontend", "worker_writing"], ["custom"], ["image"]]);
  assert.equal(option("worker_writing").disabled, true);
  assert.equal(option("worker_frontend").disabled, false);
  assert.equal(option("custom").disabled, false);
  assert.equal(option("image").disabled, true);
  const [group, custom, image] = Array.from(cardsOf(app));
  const head = (card) => card.children[0].children;
  // A group's card is named and fixed: its title and access are text; a custom card takes a specialty and
  // an access; the image card is titled and writable. The head always has name, access, tiers and delete.
  assert.deepEqual([group, custom, image].map((card) => head(card).length), [4, 4, 4]);
  assert.deepEqual([group, custom, image].map((card) => head(card)[0].tagName), ["strong", "input", "strong"]);
  assert.deepEqual([group, custom, image].map((card) => head(card)[1].tagName), ["span", "select", "span"]);
  assert.match(JSON.stringify(head(image)[0]), /生图工位/);
  assert.match(JSON.stringify(head(image)[1]), /可写/);
  // A group says what it is for (the text the decision model gets); custom and image cards have none.
  assert.match(JSON.stringify(group.children[1]), /写文章/);
  assert.match(group.children[1].className, /fine/);
  // Three level rows on a group or custom card, one row and an add button on the image card.
  assert.deepEqual([group, custom, image].map((card) => card.children.length), [5, 4, 3]);
  assert.match(JSON.stringify(image.children[2]), /添加方向/);
  // Tiers set, beside the access: none is flagged, then the count follows the levels bound.
  assert.match(JSON.stringify(head(group)[2]), /未设置/);
  assert.match(head(group)[2].className, /unset/);
  assert.match(decision((d) => d.problem()), /第 1 张卡片.*至少要绑定一档/);
  pick(group.children[3], "codex");
  assert.match(JSON.stringify(head(Array.from(cardsOf(app))[0])[2]), /已设 1 档/);
  assert.doesNotMatch(Array.from(cardsOf(app))[0].children[0].children[2].className, /unset/);
  assert.equal(decision((d) => d.problem()), "第 2 张卡片缺少专业");
  fill(custom.children[0].children[0], "数据库迁移");
  assert.match(decision((d) => d.problem()), /第 2 张卡片（数据库迁移）至少要绑定一档/);
  pick(custom.children[3], "codex");
  // The image card: its rows are checked one by one.
  const imageRow = (n) => Array.from(cardsOf(app))[2].children[n];
  assert.equal(imageRow(1).children.length, 5);
  assert.equal(decision((d) => d.problem()), "生图工位第 1 行缺少专业");
  fill(imageRow(1).children[0], "漫画风格");
  assert.match(decision((d) => d.problem()), /生图工位第 1 行（漫画风格）还没选 CLI 和模型/);
  pick(imageRow(1), "codex");
  assert.equal(decision((d) => d.problem()), null);
  const shape = ({ kind, role, description, access, difficulty }) => [kind, role, description, access, difficulty];
  assert.deepEqual(seatsOf(app).map(shape), [
    ["worker_writing", "写作", "写文章", "write", "medium"],
    ["custom", "数据库迁移", "数据库迁移", "write", "hard"],
    ["image", "漫画风格", "漫画风格", "write", undefined],
  ]);
  assert.ok(seatsOf(app).every((seat) => seat.provider === "codex" && seat.model === "chat-model"));
  // A second specialty is a second row of the same card, saved as one more image seat; the same
  // specialty twice is refused.
  Array.from(cardsOf(app))[2].children[2].listeners.click();
  assert.equal(cardsOf(app).length, 3);
  assert.equal(Array.from(cardsOf(app))[2].children.length, 4);
  assert.equal(decision((d) => d.problem()), "生图工位第 2 行缺少专业");
  fill(imageRow(2).children[0], "漫画风格");
  assert.match(decision((d) => d.problem()), /生图工位第 2 行（漫画风格）还没选 CLI 和模型/);
  pick(imageRow(2), "codex");
  assert.equal(decision((d) => d.problem()), "生图工位第 2 行的专业“漫画风格”与前面重复");
  fill(imageRow(2).children[0], "写实");
  assert.equal(decision((d) => d.problem()), null);
  const images = seatsOf(app).filter((seat) => seat.kind === "image");
  assert.deepEqual(images.map((seat) => seat.role), ["漫画风格", "写实"]);
  assert.equal(new Set(images.map((seat) => seat.id)).size, 2);
  assert.ok(images.every((seat) => !("difficulty" in seat)));
  // A row goes with its own delete button; with none left the card cannot be saved.
  imageRow(1).children[4].listeners.click();
  assert.deepEqual(seatsOf(app).filter((seat) => seat.kind === "image").map((seat) => seat.role), ["写实"]);
  imageRow(1).children[4].listeners.click();
  assert.equal(decision((d) => d.problem()), "生图工位至少要有一行");
  Array.from(cardsOf(app))[2].children[1].listeners.click();
  fill(imageRow(1).children[0], "写实");
  pick(imageRow(1), "codex");
  assert.equal(decision((d) => d.problem()), null);
  // Two custom cards may not share a specialty.
  add("custom");
  fill(Array.from(cardsOf(app))[3].children[0].children[0], "数据库迁移");
  pick(Array.from(cardsOf(app))[3].children[1], "codex");
  assert.match(decision((d) => d.problem()), /两张自定义卡片的专业都是“数据库迁移”/);
  // A custom card's access is its own to set.
  Array.from(cardsOf(app))[3].children[0].children[1].listeners.change({ target: { value: "read" } });
  assert.equal(seatsOf(app).at(-1).access, "read");
  // Taking a level off a card leaves its other levels, and a card goes with its delete button.
  pick(Array.from(cardsOf(app))[1].children[3], "");
  assert.equal(seatsOf(app).filter((seat) => seat.kind === "custom" && seat.role === "数据库迁移").length, 1);
  Array.from(cardsOf(app))[3].children[0].children[3].listeners.click();
  assert.equal(cardsOf(app).length, 3);
  // The image card may be added again once it is gone, with its one empty row.
  Array.from(cardsOf(app))[2].children[0].children[3].listeners.click();
  add("image");
  assert.equal(Array.from(cardsOf(app)).at(-1).children.length, 3);
});

test("the saved image seats come back as one card with a row for each, in the file's order", async () => {
  const seat = (id, kind, role, extra) => ({ id, kind, role, description: role, provider: "codex", model: "chat-model", access: "write", ...extra });
  const saved = { document: { version: 1, seats: [
    seat("img-a", "image", "漫画风格"), seat("w-1", "worker", "通用执行", { difficulty: "easy", description: "改代码" }), seat("img-b", "image", "写实", { effort: "high" }),
  ] }, error: null, key_set: false, key_error: null };
  const { app, decision } = decisionEditor("decision", saved);
  await decision((d) => d.load());
  const cards = Array.from(cardsOf(app));
  assert.equal(cards.length, 2);
  const rows = cards[0].children.slice(1, -1);
  assert.equal(rows.length, 2);
  assert.deepEqual(Array.from(rows.map((row) => row.children[0].attrs.value)), ["漫画风格", "写实"]);
  assert.equal(decision((d) => d.changed()), false);
  assert.deepEqual(Array.from(seatsOf(app).map((one) => one.id)), ["img-a", "img-b", "w-1"]);
  assert.equal(seatsOf(app)[1].effort, "high");
});

test("the decision options are saved with the orchestration and shape its rules text", async () => {
  const { app, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const toggle = (index, on) => app.run('$("orchestration-split")').children[index].children[0].listeners.change({ target: { checked: on } });
  const text = () => app.run('orchestrationPresetText("decision")');
  assert.equal(text(), "BASE|LEAD-ASKED|PW-ASKED|M-ASKED|");
  toggle(0, true);
  assert.equal(text(), "BASE|LEAD-ASKED|PW-OWN|M-ASKED|");
  assert.deepEqual(JSON.parse(app.run("JSON.stringify(orchestrationWanted().decision_options)")), { parallel_writes: true });
  toggle(1, true);
  assert.equal(text(), "BASE|LEAD-ASKED|PW-OWN|M-OWN|");
  toggle(2, true);
  assert.equal(text(), "BASE|LEAD-ALWAYS|PW-OWN|M-OWN|");
  // Back to the defaults leaves no key, so nothing is unsaved.
  toggle(0, false);
  toggle(1, false);
  toggle(2, false);
  assert.equal(app.run("Object.hasOwn(orchestrationWanted(), 'decision_options')"), false);
  assert.equal(app.run("orchestrationChanged()"), false);
  toggle(1, true);
  assert.equal(app.run("orchestrationChanged()"), true);
  // The mode's text is the other modes' text only in the sections they share; theirs are unchanged.
  assert.equal(app.run('orchestrationPresetText("off")'), undefined);
});

test("a card read from the file goes back unchanged, and a changed specialty takes its own description", async () => {
  const seat = (id, kind, role, description, extra) => ({ id, kind, role, description, provider: "codex", model: "chat-model", access: "write", ...extra });
  const file = [
    seat("c-1", "custom", "数据库迁移", "只做 PostgreSQL 的迁移脚本", { difficulty: "easy" }),
    seat("c-2", "custom", "数据库迁移", "只做 PostgreSQL 的迁移脚本和回滚", { difficulty: "hard" }),
    seat("i-1", "image", "漫画风格", "日式漫画的线稿和上色"),
  ];
  const { app, decision } = decisionEditor("decision", { document: { version: 1, seats: file }, error: null, key_set: false, key_error: null });
  await decision((d) => d.load());
  const descriptions = () => Array.from(seatsOf(app).map((one) => one.description));
  // Nothing is rewritten by reading and saving the cards, whatever the description says.
  assert.deepEqual(descriptions(), ["只做 PostgreSQL 的迁移脚本", "只做 PostgreSQL 的迁移脚本和回滚", "日式漫画的线稿和上色"]);
  assert.equal(decision((d) => d.changed()), false);
  // A new specialty names its own seats; putting the old one back restores what the file said.
  const card = () => Array.from(cardsOf(app))[0];
  card().children[0].children[0].listeners.input({ target: { value: "数据迁移" } });
  assert.deepEqual(Array.from(seatsOf(app).filter((one) => one.kind === "custom").map((one) => one.description)), ["数据迁移", "数据迁移"]);
  card().children[0].children[0].listeners.input({ target: { value: "数据库迁移" } });
  assert.equal(decision((d) => d.changed()), false);
  // A card made now has its role for a description.
  app.run('window.AgentNaveDecisionSeats.addCard("custom")');
  const fresh = Array.from(cardsOf(app)).at(-1);
  fresh.children[0].children[0].listeners.input({ target: { value: "写作" } });
  fresh.children[1].children[1].listeners.change({ target: { value: "codex" } });
  assert.equal(seatsOf(app).find((one) => one.role === "写作").description, "写作");
});

test("a specialty written with spaces around it still keeps its description", async () => {
  const seat = (id, kind, role, description, extra) => ({ id, kind, role, description, provider: "codex", model: "chat-model", access: "write", ...extra });
  const file = [seat("c-1", "custom", " 美术 ", "专长水彩", { difficulty: "easy" }), seat("i-1", "image", " 漫画 ", "日式线稿")];
  const { app, decision } = decisionEditor("decision", { document: { version: 1, seats: file }, error: null, key_set: false, key_error: null });
  await decision((d) => d.load());
  assert.deepEqual(Array.from(seatsOf(app).map((one) => [one.role, one.description])), [["美术", "专长水彩"], ["漫画", "日式线稿"]]);
});

test("saving sends the flat catalog on its own, without any boss switch", async () => {
  const { app, calls, decision } = decisionEditor("split", NO_FILE);
  await decision((d) => d.load());
  app.run('viewMode("decision")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  assert.equal(decision((d) => d.problem()), null);
  await decision((d) => d.save());
  const save = calls.find((call) => call.url === "/app/decision-seats" && call.method === "POST");
  assert.deepEqual(Object.keys(save.body).sort(), ["decision_model", "seats", "version"]);
  assert.equal(save.body.seats.length, 8);
  assert.deepEqual(Object.keys(save.body.seats[0]), ["id", "kind", "role", "description", "provider", "model", "access", "difficulty"]);
  assert.equal(decision((d) => d.changed()), false);
  // The saved file comes back as the same cards.
  assert.equal(cardsOf(app).length, 5);
  app.run('window.AgentNaveDecisionSeats.addCard("custom")');
  assert.match(decision((d) => d.problem()), /缺少专业/);
});

test("the seat catalog is saved before the modes, and the modes wait when it fails", async () => {
  const run = async (catalogError) => {
    const { app, calls } = decisionEditor("split", NO_FILE, catalogError);
    app.run("window.AgentNaveSkills = { save: async () => [], discard() {}, changed: () => false, problem: () => null }");
    await app.run("window.AgentNaveDecisionSeats.load()");
    app.run('viewMode("decision")');
    app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
    app.run("bindAdvanced()");
    await app.run('$("advanced-save").listeners.click()');
    return { app, posts: calls.filter((call) => call.method === "POST" && /^\/app\/(decision-seats|orchestration)$/.test(call.url)).map((call) => call.url), feedback: app.run('$("advanced-feedback").textContent') };
  };
  const ok = await run(null);
  assert.deepEqual(Array.from(ok.posts), ["/app/decision-seats", "/app/orchestration"]);
  assert.equal(ok.feedback, "设置已保存");
  // A catalog that did not save leaves the modes unsaved, with the reason and the consequence said.
  const failed = await run("磁盘已满");
  assert.deepEqual(Array.from(failed.posts), ["/app/decision-seats"]);
  assert.match(failed.feedback, /决策模型推荐工位：磁盘已满/);
  assert.match(failed.feedback, /编排：档位未保存/);
  assert.equal(failed.app.run("orchestrationChanged()"), true);
});

test("the key goes out once and the page afterwards only knows that one is set", async () => {
  const { app, calls, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const rows = () => roleChildren(app)[0].children;
  const keyRow = () => rows()[2];
  const [input, save, clear] = keyRow().children[1].children;
  assert.match(JSON.stringify(rows()[1]), /发给 Google/);
  assert.equal(clear.disabled, true);
  input.listeners.input({ target: { value: "AIza-secret" } });
  await save.listeners.click();
  const sent = calls.filter((call) => call.url === "/app/decision-key");
  assert.deepEqual(sent.map((call) => call.body), [{ key: "AIza-secret" }]);
  assert.ok(!JSON.stringify(rows()).includes("AIza-secret"));
  assert.match(JSON.stringify(keyRow()), /已保存，粘贴新密钥可替换/);
  assert.equal(keyRow().children[1].children[2].disabled, false);
  // Clearing sends an empty key.
  await keyRow().children[1].children[2].listeners.click();
  assert.deepEqual(calls.filter((call) => call.url === "/app/decision-key").at(-1).body, { key: "" });
});

test("a seat gains up to three ordered fallbacks that never repeat it, and keeps them when its own pick changes", () => {
  const app = advancedEditor();
  app.run(`
    detected.codex.models = [{ id: "chat-model" }, { id: "m2" }, { id: "m3" }, { id: "m4" }];
    reviewSeats = { "security-reviewer": { provider: "codex", model: "chat-model" } };
    renderReviewSeats();
    function findKey(nodes, key) {
      for (const node of [nodes].flat(Infinity)) {
        if (!node || typeof node !== "object") continue;
        if (node.dataset?.focusKey === key) return node;
        const found = findKey(node.children, key);
        if (found) return found;
      }
      return null;
    }
    const review = () => $("review-seats").children;
    const seat = () => seatsShown()["security-reviewer"];
  `);
  const seat = () => JSON.parse(app.run("JSON.stringify(seat())"));
  const find = (key) => app.run(`findKey(review(), ${JSON.stringify(key)})`);
  const offered = (key) => Array.from(find(key).children.flat().filter((option) => !option.disabled).map((option) => option.attrs.value));

  find("security-reviewer/fb-add").listeners.click();
  assert.deepEqual(seat().fallbacks, [{ provider: "codex", model: "m2" }], "the first free pick, at its default level");
  assert.deepEqual(offered("security-reviewer/fb0/model"), ["m2", "m3", "m4"], "never the seat's own model");
  assert.deepEqual(offered("security-reviewer/model"), ["chat-model", "m3", "m4"], "nor a model a fallback took");
  find("security-reviewer/fb0/model").listeners.change({ target: { value: "m4" } });
  find("security-reviewer/fb-add").listeners.click();
  find("security-reviewer/fb-add").listeners.click();
  assert.deepEqual(seat().fallbacks.map((pick) => pick.model), ["m4", "m2", "m3"]);
  assert.equal(find("security-reviewer/fb-add"), null, "no fourth fallback to add");

  find("security-reviewer/model").listeners.change({ target: { value: "chat-model" } });
  find("security-reviewer/cli").listeners.change({ target: { value: "codex" } });
  assert.equal(seat().fallbacks.length, 3, "changing the seat's CLI or model keeps its fallbacks");
  find("security-reviewer/fb1/remove").listeners.click();
  assert.deepEqual(seat().fallbacks.map((pick) => pick.model), ["m4", "m3"]);
  find("security-reviewer/fb0/remove").listeners.click();
  find("security-reviewer/fb0/remove").listeners.click();
  assert.deepEqual(seat(), { provider: "codex", model: "chat-model" }, "no fallback left, no field");
  app.run("discardAdvanced()");
});

test("a catalog seat's fallbacks come back as saved, and the add button sits in the row's last column", async () => {
  const fallbacks = [{ provider: "codex", model: "m2", effort: "low" }];
  const seat = { id: "w-1", kind: "worker", role: "通用执行", description: "改代码", provider: "codex", model: "chat-model", access: "write", difficulty: "easy", fallbacks };
  const saved = { document: { version: 1, seats: [seat] }, error: null, key_set: false, key_error: null };
  const { app, decision } = decisionEditor("decision", saved);
  await decision((d) => d.load());
  assert.equal(decision((d) => d.changed()), false);
  assert.deepEqual(JSON.parse(app.run("JSON.stringify(window.AgentNaveDecisionSeats.document().seats)")), [seat]);
  const card = cardsOf(app)[0];
  const rows = card.children.filter((child) => child.className === "seat-block");
  assert.equal(rows.length, 1, "only the easy level has a fallback line");
  assert.equal(rows[0].children[1].className, "fallback-row");
  assert.equal(rows[0].children[0].children.at(-1).tagName, "button");
});
