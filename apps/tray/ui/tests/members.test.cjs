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
    AN: { shellOS: () => os, art: { url: (name) => `/art/${name}` } }, structuredClone, fetch, URL, setTimeout: () => 0,
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
    if (context.AN.settings) mirrorSettings(context);
  };
  for (const file of ["provider-health.js", "settings-state.js", "member-models.js"]) load(file);
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
  providers.claude.groups = { "Gemini Models": { code: "quota", until, reset: until } };
  await app.run("apiGetMembers()");
  assert.equal(app.run('$("provider-health").children.length'), 2);
  assert.match(app.run('$("provider-health").children[1].children[0].children[0]'), /Claude Code · Gemini Models · 额度用完/);
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
  const row = () => app.run('$("orchestration-roles").children.at(-1)').children[0].children[0];
  const pick = (value) => row().children[1].children[0].listeners.change({ target: { value } });
  const seat = { provider: "codex", model: "chat-model" };

  assert.equal(row().children[0].children[0], "生图（可选）");
  assert.equal(row().children[1].children[0].children[0].children[0], "未配置");
  assert.equal(row().children[1].children[1].attrs["aria-label"], "生图的对话模型");
  app.run('viewMode("off")');
  assert.equal(app.run('$("orchestration-roles").hidden'), false);
  assert.equal(app.run('$("orchestration-roles").children.length'), 1);
  assert.equal(row().children[0].children[0], "生图（可选）");
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
  assert.equal(row().children[1].children[1].children.find((option) => option.attrs.value === "chat-model").attrs.selected, "");

  pick("");
  await app.run("saveOrchestration()");
  assert.equal(Object.hasOwn(saved, "image_generation"), false);
  app.run('delete currentDoc.modes.task.codex; renderOrchestration()');
  assert.ok(row().children[1].children[0].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled),
    "orchestration can use a confirmed CLI even when solo tasks are off");
  app.run('delete currentDoc.modes.discussion.codex; renderOrchestration()');
  assert.equal(row().children[1].children.some(option => option.attrs.value === "codex" && !option.attrs.disabled), false,
    "removing both scenes disables the CLI despite its historical model settings");
});

test("a boss's own-CLI choice saved from the boss page is kept by this page's later save", async () => {
  let saved = { version: 1, preset: "split" };
  const bosses = [{ keys: ["codex"], labels: ["Codex"], ids: [], mark: "codex" }];
  const app = advancedEditor(async (url, options) => {
    const body = options?.body && JSON.parse(options.body);
    if (url === "/app/orchestration" && !body) {
      return { ok: true, json: async () => ({ document: saved, error: null, bosses, presets: { split: "rule" }, files: [] }) };
    }
    if (url === "/app/orchestration") saved = body;
    return { ok: true, json: async () => ({ saved: true, files: [], bosses }) };
  });
  await app.run("loadOrchestration()");
  app.run('setBossMode(orchestration.bosses[0], "meetings")');
  app.listeners["agentnave:host-cli"]({ detail: { codex: "launch" } });
  assert.equal(app.run("orchestrationChanged()"), true);
  await app.run("saveOrchestration()");
  assert.equal(JSON.stringify(saved.host_cli), JSON.stringify({ codex: "launch" }));
  app.listeners["agentnave:host-cli"]({ detail: {} });
  assert.equal(app.run('"host_cli" in orchestration.document'), false);
});

function rolesEditor(document, roles = JSON.parse(readFileSync(`${__dirname}/../../../../src/agentnave/orchestration_roles.json`, "utf8"))) {
  const state = { saved: document, members: null };
  const presets = {
    split: "whole", decision: "whole-d", image_generation: "\nimage", meetings: "H\nT",
    meetings_parts: { head: "H", tail: "\nT" },
    meeting_parts: {
      default_count: 3, separator: "、", seat_provider: "{name}<{provider}>",
      all: "\nA{count}:{seats}", some: "\nS{count}:{seats}", none: "\nN{count}",
    },
    role_base: { split: "base" },
    split_parts: {
      base: "base", parallel_writes_asked: "\npw-asked", parallel_writes_own: "\npw-own",
      meetings_asked: "\nasked", meetings_own: "\nown", tail: "\ntail-s",
    },
    decision_parts: {
      base: "base", parallel_writes_asked: "\npw-asked", parallel_writes_own: "\npw-own",
      meetings_asked: "\nasked", meetings_own: "\nown", tail: "\ntail-d",
    },
    role_sections: Object.fromEntries((roles || []).map((role) => [role.group, `\n${role.group}`])),
    role_sections_medium: Object.fromEntries((roles || []).map((role) => [role.group, `\n${role.group}+medium`])),
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
  const sections = () => app.run('$("orchestration-roles").children').filter((box) => box.className === "s-group" && box.children[0].className === "s-title");
  const groups = () => sections().flatMap((box) => box.children[1].children);
  const group = (name) => groups().find((box) => box.children[0].children[0].children[0] === name);
  const toggle = (name, checked) => group(name).children[0].children[1].listeners.change({ target: { checked } });
  const split = (name, checked) => group(name).children[1].children[1].listeners.change({ target: { checked } });
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
  assert.equal(group("通用探索").children.length, 4);
  assert.equal(group("写作").children.length, 1, "a new role starts off with its seats out of sight");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s\nexplorer\nworker\nworker_frontend");

  toggle("写作", true);
  assert.equal(group("写作").children.length, 5, "turning a role on shows its difficulty switch and its seats, split while only one is set");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s\nexplorer\nworker\nworker_frontend\nworker_writing");
  assert.equal(app.run("orchestrationChanged()"), true);
  toggle("写作", false);
  assert.equal(app.run("orchestrationChanged()"), false, "back to what is saved leaves nothing unsaved");
  assert.equal(app.run('"enabled_roles" in orchestrationWanted()'), false);

  for (const name of ["通用探索", "通用执行", "界面制作"]) toggle(name, false);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s");
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
  assert.equal(app.run('$("orchestration-split").hidden'), false);
});

test("meeting seats are picked one by one, shown in the three meeting presets, kept to the count, and clear to nothing", async () => {
  const { app } = rolesEditor({ version: 1, preset: "meetings", enabled_roles: [] });
  await app.run("loadOrchestration()");
  const rows = () => app.run('$("orchestration-meeting").children');
  const size = (n) => rows()[0].children[1].children[n - 2].listeners.click();
  const pick = (seat, value) => rows()[1 + seat].children[1].children[0].listeners.change({ target: { value } });
  const wanted = () => JSON.stringify(app.run("orchestrationWanted().meeting"));
  const preview = (mode) => app.run(`orchestrationPresetText("${mode}")`);
  const name = app.run("PROVIDER_NAMES.codex");
  const one = `${name}<codex>`;

  assert.equal(app.run('$("orchestration-meeting").hidden'), false);
  assert.equal(rows().length, 4);
  assert.equal(rows()[1].children[0].children[0], "席位 1");
  assert.equal(rows()[1].children[1].children[0].children[0].children[0], "由 boss 选");
  assert.equal(preview("meetings"), "H\nT");

  pick(0, "codex");
  assert.equal(wanted(), JSON.stringify({ count: 3, seats: [{ provider: "codex" }] }));
  assert.equal(rows()[1].children[1].children[1].attrs.disabled, "", "the model follows Employees and is read-only");
  assert.equal(rows()[1].children[1].children[2].attrs.disabled, "", "so does the effort");
  assert.equal(preview("meetings"), `H\nS3:${one}\nT`);
  app.run('viewMode("split")');
  assert.equal(app.run('$("orchestration-meeting").hidden'), false);
  assert.equal(preview("split"), `base\npw-asked\nasked\nS3:${one}\ntail-s`);
  app.run('viewMode("decision")');
  assert.equal(preview("decision"), `base\npw-asked\nasked\nS3:${one}\ntail-d`);
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
  const name = (box) => box.children[0].children[0].children[0];
  assert.deepEqual(Array.from(sections(), title), ["通用", "专项"]);
  assert.deepEqual(Array.from(sections()[0].children[1].children, name), ["通用探索", "通用执行"]);
  assert.equal(sections()[1].children[1].children.map(name).includes("界面制作"), true);
  const unset = (row) => row.children[1].children[0].children[0].children[0];
  assert.equal(group("通用执行").children.length, 4, "general roles have no difficulty switch");
  assert.equal(group("界面制作").children.length, 3, "an unset specialist shows its difficulty switch and one shared row");
  assert.equal(unset(group("界面制作").children[2]), "未配置", "the interface role no longer borrows");

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
  const pick = (row, value) => group("写作").children[row + 1].children[1].children[0].listeners.change({ target: { value } });
  const shared = { provider: "codex", model: "chat-model" };

  pick(1, "codex");
  assert.deepEqual(seats(), { easy: shared, hard: shared });
  assert.equal(group("写作").children.length, 3, "the same seat on both stays one row");

  split("写作", true);
  assert.equal(group("写作").children.length, 5);
  assert.equal(app.run("orchestrationChanged()"), true);
  pick(1, "");
  assert.deepEqual(seats(), { hard: shared });
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "joining keeps the hard seat");
  assert.equal(group("写作").children.length, 3);

  split("写作", true);
  pick(3, "");
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "or the easy one when hard is unset");

  split("写作", true);
  pick(2, "codex");
  assert.deepEqual(seats(), { easy: shared, medium: shared, hard: shared });
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s\nworker_writing+medium");
  split("写作", false);
  assert.deepEqual(seats(), { easy: shared, hard: shared }, "joining drops the medium seat");
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s\nworker_writing");
  pick(1, "");
  assert.equal(seats(), null);
  assert.equal(app.run("orchestrationChanged()"), false);
});

test("one review model fills only the review roles left unset, and follows until saved", async () => {
  const app = advancedEditor();
  app.run('reviewSeats = { "security-reviewer": { provider: "codex", model: "own" } }; renderReviewSeats()');
  const shown = () => JSON.parse(app.run("JSON.stringify(seatsShown())"));
  const pick = (value) => app.run('$("review-seats").children[0]').children[1].children[0].listeners.change({ target: { value } });
  const uniform = { provider: "codex", model: "chat-model" };

  assert.equal(app.run('$("review-seats").children[0].children[0].children[0]'), "统一评审模型");
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
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.disabled"), true);
  await app.run("loadLimit()");
  assert.equal(app.run("renderInteractive('codex')"), null);
  assert.equal(app.run("renderInteractive('antigravity').children[1].attrs.checked"), false);
  const row = app.run("renderInteractive('claude')");
  assert.equal(row.attrs.className, "s-row member-runtime");
  const label = row.children[0], input = row.children[1];
  assert.equal(label.tag, "label");
  assert.equal(label.children[0], "交互模式运行");
  assert.equal(input.attrs.checked, false);
  assert.equal(input.attrs.disabled, false);
  const windows = runtimeEditor(undefined, "windows");
  assert.equal(windows.run("renderInteractive('claude').attrs.className"), "s-row member-runtime");
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
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.checked"), true);
  app.run("$('max-running').value = '3'; refreshDirty()");
  await app.run("renderInteractive('claude').children[1].attrs.onChange({ target: { checked: false } })");
  assert.deepEqual(calls, [{ version: 1, claude_interactive: false }]);
  await app.run("renderInteractive('antigravity').children[1].attrs.onChange({ target: { checked: true } })");
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
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.disabled"), true);
  finish({ ok: false, json: async () => ({ error: "预览版不修改正式数据" }) });
  await saving;
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.checked"), false);
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
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.disabled"), true);
  await app.run("setInteractive('claude_interactive', true)");
  assert.equal(posts, 0);
  assert.match(app.run("$('notice').textContent"), /claude_interactive/);
  app.run("$('max-running').value = '3'");
  await app.run("saveLimit()");
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.disabled"), false);
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
  assert.equal(app.run("renderInteractive('claude').children[1].attrs.disabled"), true);
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

test("task seats offer only CLIs Employees enabled, not a connected boss's other CLI", () => {
  const app = poolEditor();
  app.run(`
    detected.grok = { installed: true, models: [{ id: "g" }] };
    orchestration = { error: null, files: [], bosses: [{ keys: ["grok"], ids: ["grok"], labels: ["Grok"], mark: "grok" }],
      document: { version: 1, preset: "split" } };
  `);
  const clis = (seat) => JSON.parse(app.run(`JSON.stringify(seatSelects("seat", "工位", ${JSON.stringify(seat)}, () => {})[0].children.flat()
    .filter((option) => option.attrs.value && !option.attrs.disabled).map((option) => option.attrs.value))`));
  assert.deepEqual(clis(null), ["codex"]);
  assert.match(app.run(`seatSelects("seat", "工位", { provider: "grok", model: "g" }, () => {})[0].children.flat()
    .find((option) => option.attrs.value === "grok").children.join("")`), /员工里已停用/);
});

test("DeepSeek route selection is explicit, model scoped and preserves other settings", () => {
  const app = poolEditor();
  app.run(`
    detected.deepseek = { installed: true, models: [{ id: "deepseek-flash" }, { id: "deepseek-v4-pro" }] };
    currentDoc.clis.deepseek = { models: {
      "deepseek-flash": { effort: "high", options: { extra: "keep" } },
      "deepseek-v4-pro": { options: { provider: "gateway", stored_credentials: false } },
    } };
    currentDoc.modes.task.deepseek = {
      models: [],
      overrides: {
        "deepseek-flash": { options: { permission_mode: "read-only" } },
      },
    };
    currentDoc.modes.discussion.deepseek = {
      models: [],
      overrides: {
        "deepseek-flash": { options: { permission_mode: "read-only", provider: "gateway", stored_credentials: false } },
      },
    };
    markSaved();
  `);
  assert.equal(app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-flash").children[0].attrs.selected'), true);
  assert.equal(app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-v4-pro").children[3].attrs.selected'), true);
  app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-flash").attrs.onChange({target: {value: "account"}})');
  assert.deepEqual(app.doc().clis.deepseek.models["deepseek-flash"], {
    effort: "high", options: { extra: "keep", provider: "deepseek-account", stored_credentials: true },
  });
  assert.deepEqual(app.doc().modes.task.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only", provider: "deepseek-account", stored_credentials: true },
  });
  assert.deepEqual(app.doc().modes.discussion.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only", provider: "gateway", stored_credentials: false },
  });
  assert.deepEqual(app.doc().clis.deepseek.models["deepseek-v4-pro"].options, { provider: "gateway", stored_credentials: false });
  app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-flash").attrs.onChange({target: {value: "official"}})');
  assert.deepEqual(app.doc().clis.deepseek.models["deepseek-flash"].options, { extra: "keep", provider: "deepseek-official" });
  assert.deepEqual(app.doc().modes.task.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only", provider: "deepseek-official" },
  });
  assert.deepEqual(app.doc().modes.discussion.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only", provider: "gateway", stored_credentials: false },
  });
  app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-flash").attrs.onChange({target: {value: "default"}})');
  assert.deepEqual(app.doc().clis.deepseek.models["deepseek-flash"].options, { extra: "keep" });
  assert.deepEqual(app.doc().modes.task.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only" },
  });
  assert.deepEqual(app.doc().modes.discussion.deepseek.overrides["deepseek-flash"], {
    options: { permission_mode: "read-only", provider: "gateway", stored_credentials: false },
  });
  // a scene that only turned stored credentials off keeps that choice
  app.run('currentDoc.modes.discussion.deepseek.overrides["deepseek-flash"] = { options: { stored_credentials: false } }');
  app.run('findKey(deepSeekRouteRows(), "deepseek/route/deepseek-flash").attrs.onChange({target: {value: "account"}})');
  assert.deepEqual(app.doc().modes.discussion.deepseek.overrides["deepseek-flash"], { options: { stored_credentials: false } });
});

test("DeepSeek route rows show a visible bilingual note without leaking internal fields", () => {
  const forbidden = /\b(provider|stored_credentials|deepseek-account|deepseek-official|options)\b/;
  const zhApp = poolEditor();
  zhApp.run('detected.deepseek = { installed: true, models: [{ id: "custom-model" }] }');
  assert.equal(zhApp.run("deepSeekRouteRows().length"), 0);
  zhApp.run('detected.deepseek = { installed: true, models: [{ id: "deepseek-flash" }] }');
  const zhRows = zhApp.run("deepSeekRouteRows()");
  const zhNote = zhRows.find((node) => node.tag === "p");
  assert.ok(zhNote);
  assert.equal(zhNote.attrs.className, "member-muted member-note");
  assert.equal(zhNote.attrs.hidden, undefined);
  const zhText = zhNote.children.join("");
  assert.match(zhText, /登录账号.*保存/);
  assert.match(zhText, /官方 API Key.*官方 API/);
  assert.match(zhText, /沿用 CLI 设置.*不改变路由/);
  assert.match(zhText, /选中.*模型时生效/);
  assert.doesNotMatch(zhText, forbidden);

  const enApp = editor();
  enApp.run(`
    AN.settings.el = (tag, attrs, ...children) => ({ tag, attrs, children });
    AN.settings.textNode = (s) => s;
    AN.settings.settingTip = () => AN.settings.el("span");
    AN.L = (_zh, en) => en;
    AN.settings.L = AN.L;
  `);
  enApp.load("member-cards.js");
  enApp.run('detected.deepseek = { installed: true, models: [{ id: "deepseek-flash" }] }');
  const enRows = enApp.run("deepSeekRouteRows()");
  const enNote = enRows.find((node) => node.tag === "p");
  assert.ok(enNote);
  assert.equal(enNote.attrs.className, "member-muted member-note");
  assert.equal(enNote.attrs.hidden, undefined);
  const enText = enNote.children.join("");
  assert.match(enText, /Signed-in account.*save settings/i);
  assert.match(enText, /Official API key.*official API/i);
  assert.match(enText, /Follow CLI settings.*leaves routing unchanged/i);
  assert.match(enText, /when that model is selected/i);
  assert.doesNotMatch(enText, forbidden);
});

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
    poolPick("codex/pool/ticked");
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

  // Only the ticked is an explicit list, so a model the CLI lists later is not let in.
  app.run('poolPick("codex/pool/ticked"); detected.codex.models.push({ id: "next" })');
  assert.deepEqual(app.doc().clis.codex.allowed_models, ["a"]);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "a" }).some((o) => o.attrs.value === "next")'), false);
  // All models drops the list, so later models count and the default and conflicts stay right.
  app.run('poolPick("codex/pool/all")');
  assert.equal(Object.hasOwn(app.doc().clis.codex, "allowed_models"), false);
  assert.equal(app.run('poolConflicts("codex").length'), 0);
  assert.equal(app.run('seatOptions({ provider: "codex", model: "a" }).some((o) => o.attrs.value === "next")'), true);
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

test("every model lifts a scene list an older file kept", () => {
  const app = poolEditor();
  app.run(`
    currentDoc.modes.task.codex.models = ["a"];
    currentDoc.modes.task.codex.default = "a";
    markSaved();
    pickerOpen = "codex/pool";
  `);
  assert.equal(app.run('shownPool("codex")'), null, "one unlimited scene shows every model");
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/all").attrs["aria-checked"]'), "false", "a scene still limited is not shown as every model");
  app.run('poolPick("codex/pool/all")');
  assert.deepEqual(app.doc().modes.task.codex.models, [], "the limited scene takes every model too");
  assert.equal(Object.hasOwn(app.doc().clis.codex, "allowed_models"), false);
});

test("every model is ticked until one is unticked, and no list size is refused", () => {
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
  assert.equal(app.run('poolSummary("codex", getAllModelIds("codex"), shownPool("codex"))'), "全部（含以后新出的）");
  app.run('pickerOpen = "codex/pool"');
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/allow/model-0").attrs.checked'), true);
  assert.equal(app.run('findKey(poolRows("codex"), "codex/pool/none")'), null, "clearing belongs to the ticked tier");
  app.run('poolPick("codex/pool/allow/model-3", false)');
  assert.equal(app.doc().clis.codex.allowed_models.length, 23, "unticking one leaves the rest as a list");
  assert.equal(app.doc().clis.codex.allowed_models.includes("model-3"), false);
  app.run('poolPick("codex/pool/all")');
  assert.equal(Object.hasOwn(app.doc().clis.codex, "allowed_models"), false, "all models saves no list");
  app.run('poolPick("codex/pool/ticked")');
  assert.equal(app.doc().clis.codex.allowed_models.length, 24, "the ticked tier starts from every listed model");
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
  ] }; `);
  assert.match(app.run("taskScopeNote()"), /^Codex CLI 已启用编排.*单独干活只提供给未启用编排的 boss/);
  app.run('orchestration.document.apps = {}');
  assert.equal(app.run("taskScopeNote()"), "");
});

test("meetings mode saves its opt-in switch and previews both branches", async () => {
  const { app, state } = rolesEditor({ version: 1, preset: "meetings" });
  await app.run("loadOrchestration()");
  app.run('orchestration.presets.meetings_parts.meetings_asked = "\\nasked"; orchestration.presets.meetings_parts.meetings_own = "\\nown"');
  const toggle = () => app.run('$("orchestration-split").children[0].children[1]');
  assert.equal(app.run('orchestrationPresetText("meetings")'), "H\nasked\nT");
  toggle().listeners.change({ target: { checked: true } });
  assert.equal(app.run('orchestrationPresetText("meetings")'), "H\nown\nT");
  await app.run("saveOrchestration()");
  assert.equal(JSON.stringify(state.saved.meetings_options), '{"meetings":true}');
});

test("split work shares the three switches with decision mode and saves its own split_options", async () => {
  const { app } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  assert.equal(app.run('$("orchestration-split").children.length'), 3);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s");
  const sw = (i) => () => app.run(`$("orchestration-split").children[${i}].children[1]`);
  const [parallelWrites, meetings] = [sw(0), sw(1)];
  parallelWrites().listeners.change({ target: { checked: true } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().split_options)"), '{"parallel_writes":true}');
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-own\nasked\ntail-s");
  meetings().listeners.change({ target: { checked: true } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().split_options)"), '{"parallel_writes":true,"meetings":true}');
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-own\nown\ntail-s");
  parallelWrites().listeners.change({ target: { checked: false } });
  assert.equal(app.run("JSON.stringify(orchestrationWanted().split_options)"), '{"meetings":true}');
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nown\ntail-s");
  meetings().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationWanted().split_options"), undefined);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\npw-asked\nasked\ntail-s");
});

test("the orchestration-only switch rewrites its line in split and decision only", async () => {
  const { app } = rolesEditor({ version: 1, preset: "split", enabled_roles: [] });
  await app.run("loadOrchestration()");
  app.run(`orchestration.presets.split_parts.base = "base\\nlead-asked";
    orchestration.presets.decision_parts.base = "base\\nlead-asked";
    orchestration.presets.lead = { asked: "lead-asked", always: "lead-always" }; renderOrchestration()`);
  const lead = () => app.run('$("orchestration-split").children[2].children[1]');
  lead().listeners.change({ target: { checked: true } });
  assert.equal(app.run("orchestrationWanted().lead_only"), true);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nlead-always\npw-asked\nasked\ntail-s");
  assert.equal(app.run('orchestrationPresetText("decision")'), "base\nlead-always\npw-asked\nasked\ntail-d");
  app.run('viewMode("custom")');
  assert.equal(app.run('$("orchestration-split").hidden'), true);
  app.run('viewMode("split")');
  lead().listeners.change({ target: { checked: false } });
  assert.equal(app.run("orchestrationWanted().lead_only"), undefined);
  assert.equal(app.run('orchestrationPresetText("split")'), "base\nlead-asked\npw-asked\nasked\ntail-s");
});

test("a switch put back to what is saved keeps the saved form of its key", async () => {
  const saved = { version: 1, preset: "split", enabled_roles: [], lead_only: false,
    split_options: { meetings: false } };
  const { app } = rolesEditor(saved);
  await app.run("loadOrchestration()");
  const [meetings, lead] = [1, 2].map((i) => () => app.run(`$("orchestration-split").children[${i}].children[1]`));
  for (const [switchOf, key] of [[meetings, "split_options"], [lead, "lead_only"]]) {
    switchOf().listeners.change({ target: { checked: true } });
    assert.equal(app.run("orchestrationChanged()"), true);
    switchOf().listeners.change({ target: { checked: false } });
    assert.deepEqual(JSON.parse(app.run(`JSON.stringify(orchestrationWanted()["${key}"])`)), saved[key]);
    assert.equal(app.run("orchestrationChanged()"), false);
  }
});

test("JS preset preview matches orchestration_rules.json assembly verbatim for split and decision", async () => {
  const rules = JSON.parse(readFileSync(`${__dirname}/../../../../src/agentnave/orchestration_rules.json`, "utf8"));
  const roles = JSON.parse(readFileSync(`${__dirname}/../../../../src/agentnave/orchestration_roles.json`, "utf8"));
  const roleLine = (role, medium) => rules.enabled_roles[medium ? "line_medium" : "line"]
    .replace("{name_zh}", role.name_zh)
    .replace("{access_zh}", rules.enabled_roles.access_zh[role.access])
    .replace("{scope_zh}", role.scope_zh)
    .replace("{seat_prefix}", role.seat_prefix);
  const dispatchParts = (tail) => ({
    base: rules.sections.shared,
    parallel_writes_asked: rules.sections.parallel_writes_when_asked,
    parallel_writes_own: rules.sections.parallel_writes_by_boss,
    checks: rules.sections.shared_checks,
    meetings_asked: rules.sections.meetings_when_asked,
    meetings_own: rules.sections.meetings_on_tradeoffs,
    tail: rules.sections[tail],
  });
  const doc = {
    version: 1, preset: "split", lead_only: true,
    split_options: { parallel_writes: true, meetings: true },
    decision_options: { parallel_writes: true, meetings: false },
    meeting: { count: 4, seats: [{ provider: "codex" }, null, { provider: "claude" }] },
    enabled_roles: ["explorer", "worker_writing"],
    worker_writing: { medium: { provider: "codex", model: "chat-model" } },
    image_generation: { provider: "codex", model: "chat-model" },
  };
  const { app } = rolesEditor(doc, roles);
  await app.run("loadOrchestration()");
  app.run(`orchestration.presets = ${JSON.stringify({
    split: "x", decision: "x",
    image_generation: rules.sections.image_generation,
    meeting_parts: {
      default_count: rules.meeting_defaults.default_count,
      separator: rules.meeting_defaults.separator,
      seat_provider: rules.meeting_defaults.seat_provider,
      all: rules.meeting_defaults.all_named,
      some: rules.meeting_defaults.some_named,
      none: rules.meeting_defaults.none_named,
    },
    split_parts: dispatchParts("dispatch_split"),
    decision_parts: dispatchParts("dispatch_decision"),
    t3: Object.fromEntries(Object.entries(rules.t3_order).map(([m, steps]) => [m, steps.map((s) => rules.sections[s]).join("")])),
    non_t3_heading: rules.sections.t3_non_t3_heading,
    t3_other: {
      template: rules.t3_other.map((step) => rules.sections[step]).join(""),
      names: rules.meeting_defaults.provider_names,
      separator: rules.meeting_defaults.separator,
    },
    role_sections: Object.fromEntries(roles.map((r) => [r.group, roleLine(r, false)])),
    role_sections_medium: Object.fromEntries(roles.map((r) => [r.group, roleLine(r, true)])),
    lead: { asked: rules.switches.lead_only.when_false, always: rules.switches.lead_only.when_true },
  })};`);
  const assemble = (mode, withT3 = false) => rules.order[mode].map((step) => {
    if (step === "meeting_defaults") {
      const names = ["Codex CLI（provider=`codex`）", "Claude Code（provider=`claude`）"].join(rules.meeting_defaults.separator);
      return rules.meeting_defaults.some_named.replace("{count}", "4").replace("{seats}", names);
    }
    if (step === "enabled_roles") {
      return roles.filter((r) => doc.enabled_roles.includes(r.group))
        .map((r) => roleLine(r, Boolean(doc[r.group]?.medium))).join("");
    }
    if (step === "image_generation") return rules.sections.image_generation;
    const sw = rules.switches[step];
    if (sw) {
      const val = sw.field.split(".").reduce((cur, k) => cur?.[k], doc);
      return rules.sections[(val ?? sw.default) ? sw.when_true : sw.when_false];
    }
    let text = rules.sections[step];
    if (withT3 && (step === "dispatch_split" || step === "dispatch_decision")) {
      text = rules.sections.t3_non_t3_heading + text;
    }
    if (doc.lead_only && rules.switches.lead_only.replaces_in.includes(step)) {
      text = text.replace(rules.switches.lead_only.when_false, rules.switches.lead_only.when_true);
    }
    return text;
  }).join("");
  assert.equal(app.run('orchestrationPresetText("split")'), assemble("split"));
  assert.equal(app.run('orchestrationPresetText("decision")'), assemble("decision"));
  // With T3 orchestration enabled, JS inserts non_t3_heading and appends the exact same t3_order sections as Rust.
  const t3Suffix = (mode) => rules.t3_order[mode].map((s) => rules.sections[s]).join("");
  app.run('window.AgentNaveT3Seats = { enabled: () => true, bosses: () => ["codex"] };');
  assert.equal(app.run('orchestrationPresetText("split")'), assemble("split", true) + t3Suffix("split"));
  assert.equal(app.run('orchestrationPresetText("decision")'), assemble("decision", true) + t3Suffix("decision"));
  assert.equal(app.run('orchestrationPresetText("split", false)'), assemble("split"));
  assert.equal(app.run('orchestrationPresetText("custom")'), t3Suffix("custom").trim());
  assert.equal(app.run('orchestrationPresetText("off")'), undefined);
  // Selected CLIs AgentNave cannot recognise in T3 get their own section, named, after the dispatch.
  const others = (names) => rules.t3_other.map((step) => rules.sections[step]).join("").replaceAll("{names}", names);
  app.run('window.AgentNaveT3Seats = { enabled: () => true, bosses: () => ["codex", "grok", "pi"] };');
  assert.equal(app.run('orchestrationPresetText("split")'), assemble("split", true) + t3Suffix("split") + others("Grok CLI、Pi Coding Agent"));
  assert.equal(app.run('orchestrationPresetText("custom")'), (t3Suffix("custom") + others("Grok CLI、Pi Coding Agent")).trim());
  app.run('window.AgentNaveT3Seats = { enabled: () => true, bosses: () => ["grok"] };');
  assert.equal(app.run('orchestrationPresetText("decision")'), assemble("decision", true) + others("Grok CLI"));
  app.run('window.AgentNaveT3Seats = { enabled: () => true, bosses: () => [] };');
  assert.equal(app.run('orchestrationPresetText("split")'), assemble("split"));
});

// The decision mode: its page, options, the first catalog and the key, on the same fake DOM.
function decisionEditor(preset, loaded, catalogError = null, catalog = () => ({ models: [] })) {
  const calls = [];
  const app = advancedEditor(async (url, options = {}) => {
    calls.push({ url, method: options.method || "GET", body: options.body ? JSON.parse(options.body) : null });
    if (url.startsWith("/app/decision-models")) {
      const answer = catalog(new URL(url, "http://local"));
      return answer.error ? { ok: false, status: 502, json: async () => answer } : { ok: true, json: async () => answer };
    }
    if (url === "/app/decision-seats" && options.method === "POST" && catalogError) return { ok: false, status: 500, json: async () => ({ error: catalogError }) };
    if (url === "/app/decision-seats" && !options.method) return { ok: true, json: async () => loaded };
    if (url === "/app/decision-key") {
      const { provider = "gemini", key } = JSON.parse(options.body);
      const previews = loaded.key_previews && { ...loaded.key_previews, ...(key ? { [provider]: "sk-p…9a8b" } : {}) };
      if (!key && previews) delete previews[provider];
      return { ok: true, json: async () => ({ provider, key_set: key !== "", keys: { ...(loaded.keys || {}), [provider]: key !== "" }, ...(previews ? { key_previews: previews } : {}) }) };
    }
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
  app.load("seat-rows.js");
  app.load("decision-seats.js");
  app.run(`orchestration.presets = {
    decision: "decision", image_generation: "|image", lead: { asked: "LEAD-ASKED", always: "LEAD-ALWAYS" },
    decision_parts: { base: "BASE|LEAD-ASKED|", parallel_writes_asked: "PW-ASKED|", parallel_writes_own: "PW-OWN|",
      meetings_asked: "M-ASKED|", meetings_own: "M-OWN|", tail: "TAIL|" } };`);
  app.run(`viewMode("${preset}")`);
  return { app, calls, decision };
}
const roleChildren = (app) => app.run('$("orchestration-roles")').children;
const NO_FILE = { document: null, error: null, key_set: false, key_error: null };
// What the tray answers once it knows the platforms: the table of ADR-0123, in the order of its page.
const PLATFORM = (id, name, key, model, models, extra = {}) => ({ id, name_zh: name, name_en: name, key, fixed_model: false, default_model: model, models: models.map((one) => ({ id: one, name: one })), catalog: false, custom_url: false, url_required: false, default_base_url: null, ...extra });
const PLATFORMS = [
  PLATFORM("gemini", "Google Gemini", "required", "gemini-decision-1", [], { fixed_model: true }),
  PLATFORM("openai", "OpenAI", "required", "gpt-6-luna", [], { fixed_model: true }),
  PLATFORM("typesafe", "TypeSafe AI", "required", "jev-latest", ["jev-latest", "jev-1.14.0"], { catalog: true }),
  PLATFORM("openrouter", "OpenRouter", "required", "liquid/d1.2-ultra", ["liquid/d1.2-ultra", "liquid/d1.2"], { catalog: true }),
  PLATFORM("ollama", "本机 Ollama", "none", null, [], { catalog: true, custom_url: true, default_base_url: "http://127.0.0.1:11434" }),
  PLATFORM("custom", "自定义端点", "optional", null, [], { catalog: true, custom_url: true, url_required: true }),
];
const KEYS_OFF = { gemini: false, openai: false, typesafe: false, openrouter: false, custom: false };
const PLATFORM_FILE = (document = null, keys = {}) => ({ document, error: null, key_set: Boolean(keys[document?.decision_provider || "gemini"]), keys: { ...KEYS_OFF, ...keys }, key_error: null, providers: PLATFORMS });
const topRows = (app) => Array.from(roleChildren(app)[0].children[0].children[0].children);
const topRow = (app, text) => topRows(app).find((row) => JSON.stringify(row).includes(text));
const findKey = (node, key) => {
  if (!node || typeof node !== "object") return null;
  if (node.dataset?.focusKey === key) return node;
  for (const child of node.children || []) {
    const found = findKey(child, key);
    if (found) return found;
  }
  return null;
};
const control = (app, key) => findKey({ children: roleChildren(app) }, key);
const settle = () => new Promise((resolve) => setImmediate(resolve));
const decisionFile = (app) => JSON.parse(app.run("JSON.stringify(window.AgentNaveDecisionSeats.document())"));

const boss = (app) => app.run("AN.settings.orchestration.bosses[0]");

const seatsOf = (app) => JSON.parse(app.run("JSON.stringify(window.AgentNaveDecisionSeats.document().seats)"));
// The catalog panel is the first group (picture, key, switch), one card holding all expandable seat rows (when non-empty), then the add menu.
const cardsOf = (app) => {
  const middle = roleChildren(app)[0].children.slice(1, -1);
  return middle.length ? middle[0].children[0].children : [];
};
const rowHead = (row) => {
  const [lead, meta, remove] = row.children[0].children;
  const toggle = lead.children[0];
  const tip = lead.children[1] || null;
  const [access, summary] = meta.children;
  return { toggle, name: toggle.children[1], tip, access, summary, remove };
};
const rowBody = (row) => row.children[1]?.children || [];
const toggleRow = (row) => rowHead(row).toggle.listeners.click();

test("a boss put into the decision mode makes the first catalog from the fixed seats once", async () => {
  const { app, decision } = decisionEditor("split", NO_FILE);
  await decision((d) => d.load());
  app.run('orchestration.document.worker.medium = { provider: "codex", model: "chat-model", effort: "low" }');
  app.run('viewMode("decision")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  const seats = seatsOf(app);
  // One card per bound group with each bound difficulty filled in and medium empty where unbound, then the image seat.
  assert.deepEqual(seats.map(({ kind, role, description, access, difficulty }) => [kind, role, description, access, difficulty]), [
    ["explorer", "通用探索", "读代码", "read", "easy"],
    ["explorer", "通用探索", "读代码", "read", "hard"],
    ["worker", "通用执行", "改代码", "write", "easy"],
    ["worker", "通用执行", "改代码", "write", "medium"],
    ["worker", "通用执行", "改代码", "write", "hard"],
    ["worker_frontend", "界面制作", "做界面", "write", "easy"],
    ["worker_frontend", "界面制作", "做界面", "write", "hard"],
    ["worker_writing", "写作", "写文章", "write", "easy"],
    ["image", "通用生图", "通用生图", "write", undefined],
  ]);
  assert.equal(seats[1].effort, "high");
  assert.equal(seats[3].effort, "low");
  assert.ok(!("difficulty" in seats[8]));
  assert.equal(new Set(seats.map((seat) => seat.id)).size, 9);
  assert.ok(seats.every((seat) => /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(seat.id) && !("tags" in seat)));
  assert.equal(cardsOf(app).length, 5);
  assert.equal(decision((d) => d.changed()), true);
  assert.equal(decision((d) => d.problem()), null);
  // Leaving the mode keeps the catalog, and coming back does not make another one.
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "off")');
  app.run('setBossMode(AN.settings.orchestration.bosses[0], "decision")');
  assert.equal(seatsOf(app).length, 9);
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
  const fixed = ["s-group", "s-group", "s-group"];
  app.run('$("custom-text").value = "我的规则"');
  // Another mode's page is what it was: fixed seats, its three switches, nothing of the catalog.
  assert.deepEqual(kinds(), fixed);
  assert.equal(options().length, 3);
  assert.equal(decision((d) => d.panel() === null), false);
  for (const mode of ["off", "meetings", "custom"]) {
    app.run(`viewMode("${mode}")`);
    assert.ok(!kinds().some((kind) => /decision/.test(kind)), mode);
  }
  app.run('viewMode("decision")');
  // The catalog sits where the fixed seats were, and the fixed image seat is gone; three switches.
  assert.deepEqual(kinds(), ["s-wrap decision-seats"]);
  assert.equal(options().length, 3);
  const keyRow = roleChildren(app)[0].children[0].children[0].children[1];
  const figure = roleChildren(app)[0].children[0].children[0].children[0].children[0];
  assert.equal(figure.tagName, "figure");
  // Under the picture, four who-does-what captions; what the decision model never does sits in the key's hint.
  const [image, captions] = figure.children;
  assert.equal(captions.children.length, 4);
  assert.ok(captions.children.every((cell) => cell.children[0].tagName === "strong" && cell.children[1].tagName === "span"));
  const text = JSON.stringify(captions);
  for (const part of ["boss", "写好任务，交给服务", "筛出现在能派的工位", "决策模型", "只回答选哪个工位", "启动选中的工位，把选了谁告诉 boss"]) assert.ok(text.includes(part), part);
  assert.match(JSON.stringify(keyRow), /决策模型不接触 CLI，也不改任务内容；任务跑完后，boss 照常自己去收结果。/);
  assert.equal(image.tagName, "img");
  assert.match(JSON.stringify(keyRow), /发给 Google/);
  assert.match(app.run('$("orchestration-effect")').attrs["aria-label"], /决策模型/);
  app.run('viewMode("split")');
  assert.deepEqual(kinds(), fixed);
});

test("cards: a group once, custom as often as wanted, one image card with a row for each specialty", async () => {
  const { app, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const add = (what) => app.run(`window.AgentNaveDecisionSeats.addCard(${JSON.stringify(what)})`);
  const pick = (row, provider) => row.children[1].children[0].listeners.change({ target: { value: provider } });
  const fill = (input, text) => input.listeners.input({ target: { value: text } });
  add("worker_writing");
  add("worker_writing");
  add("custom");
  add("image");
  add("image");
  add("gardening");
  assert.equal(cardsOf(app).length, 3);
  // All seat rows sit in one s-card between the top group and the add menu.
  assert.equal(roleChildren(app)[0].children.length, 3);
  // Newly added cards auto-expand.
  assert.deepEqual(Array.from(cardsOf(app)).map((row) => row.dataset.expanded), ["true", "true", "true"]);
  // The menu lists general seats, specialist seats, custom seats and image seats; a group or the image
  // card already on the page is disabled, custom stays open.
  const menu = roleChildren(app)[0].children.at(-1).children[0].children[0].children[1];
  const groups = menu.children.filter((child) => child.tagName === "optgroup");
  assert.deepEqual(Array.from(groups.map((one) => one.attrs.label)), ["通用工位", "专业工位", "自定义工位", "生图工位"]);
  const option = (value) => groups.flatMap((one) => one.children).find((one) => one.attrs.value === value);
  assert.deepEqual(Array.from(groups.map((one) => Array.from(one.children.map((child) => child.attrs.value)))), [["explorer", "worker"], ["worker_frontend", "worker_writing"], ["custom"], ["image"]]);
  assert.equal(option("worker_writing").disabled, true);
  assert.equal(option("worker_frontend").disabled, false);
  assert.equal(option("custom").disabled, false);
  assert.equal(option("image").disabled, true);
  const [group, custom, image] = Array.from(cardsOf(app));
  assert.match(JSON.stringify(rowHead(image).name), /生图工位/);
  assert.match(JSON.stringify(rowHead(image).access), /可写/);
  // A group says what it is for (the text the decision model gets); custom and image cards have none.
  assert.match(JSON.stringify(rowHead(group).tip), /写文章/);
  assert.equal(rowHead(custom).tip, null);
  assert.equal(rowHead(image).tip, null);
  // Expanded body: 3 tier rows for group; custom has specialty+access row first + 3 tier rows; image has 1 row + add button.
  assert.deepEqual([group, custom, image].map((card) => rowBody(card).length), [3, 4, 2]);
  assert.match(JSON.stringify(rowBody(image)[1]), /添加方向/);
  // Unset summary is flagged, then updates to list bound tiers by primary model name.
  assert.match(JSON.stringify(rowHead(group).summary), /未设置/);
  assert.match(rowHead(group).summary.className, /is-unset/);
  assert.match(decision((d) => d.problem()), /第 1 张卡片.*至少要绑定一档/);
  pick(rowBody(group)[1], "codex");
  assert.equal(rowHead(Array.from(cardsOf(app))[0]).summary.children[0], "中等 chat-model");
  assert.doesNotMatch(rowHead(Array.from(cardsOf(app))[0]).summary.className, /is-unset/);
  assert.equal(decision((d) => d.problem()), "第 2 张卡片缺少专业");
  const customInputs = (card) => rowBody(card)[0].children[1].children;
  fill(customInputs(Array.from(cardsOf(app))[1])[0], "数据库迁移");
  assert.match(decision((d) => d.problem()), /第 2 张卡片（数据库迁移）至少要绑定一档/);
  pick(rowBody(Array.from(cardsOf(app))[1])[3], "codex");
  // The image card: its rows are checked one by one.
  const imageRow = (n) => rowBody(Array.from(cardsOf(app))[2])[n];
  assert.equal(imageRow(0).children.length, 2);
  assert.equal(decision((d) => d.problem()), "生图工位第 1 行缺少专业");
  fill(imageRow(0).children[0].children[0], "漫画风格");
  assert.match(decision((d) => d.problem()), /生图工位第 1 行（漫画风格）还没选 CLI 和模型/);
  pick(imageRow(0), "codex");
  assert.equal(decision((d) => d.problem()), null);
  assert.equal(rowHead(Array.from(cardsOf(app))[2]).summary.children[0], "漫画风格");
  const shape = ({ kind, role, description, access, difficulty }) => [kind, role, description, access, difficulty];
  assert.deepEqual(seatsOf(app).map(shape), [
    ["worker_writing", "写作", "写文章", "write", "medium"],
    ["custom", "数据库迁移", "数据库迁移", "write", "hard"],
    ["image", "漫画风格", "漫画风格", "write", undefined],
  ]);
  assert.ok(seatsOf(app).every((seat) => seat.provider === "codex" && seat.model === "chat-model"));
  // A second specialty is a second row of the same card, saved as one more image seat; the same
  // specialty twice is refused.
  rowBody(Array.from(cardsOf(app))[2])[1].children[0].listeners.click();
  assert.equal(cardsOf(app).length, 3);
  assert.equal(rowBody(Array.from(cardsOf(app))[2]).length, 3);
  assert.equal(decision((d) => d.problem()), "生图工位第 2 行缺少专业");
  fill(imageRow(1).children[0].children[0], "漫画风格");
  assert.match(decision((d) => d.problem()), /生图工位第 2 行（漫画风格）还没选 CLI 和模型/);
  pick(imageRow(1), "codex");
  assert.equal(decision((d) => d.problem()), "生图工位第 2 行的专业“漫画风格”与前面重复");
  fill(imageRow(1).children[0].children[0], "写实");
  assert.equal(decision((d) => d.problem()), null);
  const images = seatsOf(app).filter((seat) => seat.kind === "image");
  assert.deepEqual(images.map((seat) => seat.role), ["漫画风格", "写实"]);
  assert.equal(new Set(images.map((seat) => seat.id)).size, 2);
  assert.ok(images.every((seat) => !("difficulty" in seat)));
  // A row goes with its own delete button; with none left the card cannot be saved.
  imageRow(0).children[1].children.at(-1).listeners.click();
  assert.deepEqual(seatsOf(app).filter((seat) => seat.kind === "image").map((seat) => seat.role), ["写实"]);
  imageRow(0).children[1].children.at(-1).listeners.click();
  assert.equal(decision((d) => d.problem()), "生图工位至少要有一行");
  rowBody(Array.from(cardsOf(app))[2])[0].children[0].listeners.click();
  fill(imageRow(0).children[0].children[0], "写实");
  pick(imageRow(0), "codex");
  assert.equal(decision((d) => d.problem()), null);
  // Two custom cards may not share a specialty.
  add("custom");
  fill(customInputs(Array.from(cardsOf(app))[3])[0], "数据库迁移");
  pick(rowBody(Array.from(cardsOf(app))[3])[1], "codex");
  assert.match(decision((d) => d.problem()), /两张自定义卡片的专业都是“数据库迁移”/);
  // A custom card's access is its own to set.
  customInputs(Array.from(cardsOf(app))[3])[1].listeners.change({ target: { value: "read" } });
  assert.equal(seatsOf(app).at(-1).access, "read");
  // Taking a level off a card leaves its other levels, and a card goes with its delete button.
  pick(rowBody(Array.from(cardsOf(app))[1])[3], "");
  assert.equal(seatsOf(app).filter((seat) => seat.kind === "custom" && seat.role === "数据库迁移").length, 1);
  rowHead(Array.from(cardsOf(app))[3]).remove.listeners.click();
  assert.equal(cardsOf(app).length, 3);
  // The image card may be added again once it is gone, with its one empty row.
  rowHead(Array.from(cardsOf(app))[2]).remove.listeners.click();
  add("image");
  assert.equal(rowBody(Array.from(cardsOf(app)).at(-1)).length, 2);
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
  // Loaded valid cards start collapsed, showing their summary.
  assert.equal(cards[0].dataset.expanded, "false");
  assert.equal(rowHead(cards[0]).summary.children[0], "漫画风格 · 写实");
  assert.equal(rowHead(cards[1]).summary.children[0], "简单 chat-model");
  toggleRow(cards[0]);
  const rows = rowBody(Array.from(cardsOf(app))[0]).slice(0, -1);
  assert.equal(rows.length, 2);
  assert.deepEqual(Array.from(rows.map((row) => row.children[0].children[0].attrs.value)), ["漫画风格", "写实"]);
  assert.equal(decision((d) => d.changed()), false);
  assert.deepEqual(Array.from(seatsOf(app).map((one) => one.id)), ["img-a", "img-b", "w-1"]);
  assert.equal(seatsOf(app)[1].effort, "high");
});

test("the decision options are saved with the orchestration and shape its rules text", async () => {
  const { app, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const toggle = (index, on) => app.run('$("orchestration-split")').children[index].children[1].listeners.change({ target: { checked: on } });
  const text = () => app.run('orchestrationPresetText("decision")');
  assert.equal(text(), "BASE|LEAD-ASKED|PW-ASKED|M-ASKED|TAIL|");
  toggle(0, true);
  assert.equal(text(), "BASE|LEAD-ASKED|PW-OWN|M-ASKED|TAIL|");
  assert.deepEqual(JSON.parse(app.run("JSON.stringify(orchestrationWanted().decision_options)")), { parallel_writes: true });
  toggle(1, true);
  assert.equal(text(), "BASE|LEAD-ASKED|PW-OWN|M-OWN|TAIL|");
  toggle(2, true);
  assert.equal(text(), "BASE|LEAD-ALWAYS|PW-OWN|M-OWN|TAIL|");
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
  toggleRow(card());
  const customInput = (row) => rowBody(row)[0].children[1].children[0];
  customInput(card()).listeners.input({ target: { value: "数据迁移" } });
  assert.deepEqual(Array.from(seatsOf(app).filter((one) => one.kind === "custom").map((one) => one.description)), ["数据迁移", "数据迁移"]);
  customInput(card()).listeners.input({ target: { value: "数据库迁移" } });
  assert.equal(decision((d) => d.changed()), false);
  // A card made now has its role for a description and starts expanded.
  app.run('window.AgentNaveDecisionSeats.addCard("custom")');
  const fresh = Array.from(cardsOf(app)).at(-1);
  customInput(fresh).listeners.input({ target: { value: "写作" } });
  rowBody(fresh)[1].children[1].children[0].listeners.change({ target: { value: "codex" } });
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

test("the random-candidates switch defaults to off, writes random_candidates when on, and omits it when off", async () => {
  const { app, calls, decision } = decisionEditor("decision", NO_FILE);
  await decision((d) => d.load());
  const row = () => topRow(app, "同一档的候选随机选");
  const input = () => row().children[1];
  assert.equal(row().className, "s-row");
  assert.equal(row().children.length, 2);
  assert.deepEqual(Array.from(row().children[0].children), ["同一档的候选随机选，分摊额度"]);
  assert.equal(input().checked, false);
  assert.equal(decision((d) => d.changed()), false);

  input().listeners.change({ target: { checked: true } });
  assert.equal(input().checked, true);
  assert.equal(decision((d) => d.changed()), true);
  await decision((d) => d.save());
  const savedOn = calls.filter((call) => call.url === "/app/decision-seats" && call.method === "POST").at(-1);
  assert.equal(savedOn.body.random_candidates, true);
  assert.equal(decision((d) => d.changed()), false);

  input().listeners.change({ target: { checked: false } });
  assert.equal(decision((d) => d.changed()), true);
  decision((d) => d.discard());
  assert.equal(decision((d) => d.changed()), false);
  app.run("renderOrchestration()");
  assert.equal(input().checked, true);

  input().listeners.change({ target: { checked: false } });
  await decision((d) => d.save());
  const savedOff = calls.filter((call) => call.url === "/app/decision-seats" && call.method === "POST").at(-1);
  assert.equal("random_candidates" in savedOff.body, false);

  const explicitFalse = decisionEditor("decision", { document: { version: 1, random_candidates: false, seats: [] }, error: null, key_set: false, key_error: null });
  await explicitFalse.decision((d) => d.load());
  assert.equal(explicitFalse.decision((d) => d.changed()), false);
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
  const keyRow = () => topRow(app, "Gemini 密钥");
  const [input, save, clear] = keyRow().children[1].children[0].children;
  assert.match(JSON.stringify(topRow(app, "决策平台")), /发给 Google/);
  assert.equal(clear.disabled, true);
  input.listeners.input({ target: { value: "AIza-secret" } });
  await save.listeners.click();
  const sent = calls.filter((call) => call.url === "/app/decision-key");
  assert.deepEqual(sent.map((call) => call.body), [{ provider: "gemini", key: "AIza-secret" }]);
  assert.ok(!JSON.stringify(roleChildren(app)).includes("AIza-secret"));
  assert.match(JSON.stringify(keyRow()), /已保存，粘贴新密钥可替换/);
  assert.equal(keyRow().children[1].children[0].children[2].disabled, false);
  // Clearing sends an empty key.
  await keyRow().children[1].children[0].children[2].listeners.click();
  assert.deepEqual(calls.filter((call) => call.url === "/app/decision-key").at(-1).body, { provider: "gemini", key: "" });
});

test("the default platform is Gemini with its one model, and what is saved for it stays what older services read", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE());
  await decision((d) => d.load());
  // Picture, platform, model, key, switch: no address row, and nothing asked of any platform.
  assert.equal(topRows(app).length, 5);
  assert.match(JSON.stringify(topRow(app, "决策平台")), /Google Gemini/);
  assert.ok(topRow(app, "gemini-decision-1"));
  assert.ok(topRows(app).every((row) => !JSON.stringify(row).includes("API 地址")));
  assert.deepEqual(Object.keys(decisionFile(app)), ["version", "decision_model", "seats"]);
  assert.equal(decisionFile(app).decision_model, "gemini-decision-1");
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).length, 0);
  assert.equal(decision((d) => d.changed()), false);
  assert.equal(control(app, "decision/provider").children.length, 6);
});

test("choosing another platform fills its own model, shows only what it takes and saves from the one bar", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(), null, (url) => ({
    models: url.searchParams.get("provider") === "openrouter" ? [{ id: "liquid/d1.2", name: "liquid/d1.2" }, { id: "zeta/decider-small", name: "Zeta Small" }] : [],
  }));
  await decision((d) => d.load());
  const choose = (id) => control(app, "decision/provider").listeners.change({ target: { value: id } });
  choose("openai");
  assert.equal(decision((d) => d.changed()), true);
  assert.deepEqual(decisionFile(app), { version: 1, decision_provider: "openai", decision_model: "gpt-6-luna", seats: [] });
  assert.match(JSON.stringify(topRow(app, "OpenAI 密钥")), /粘贴密钥/);
  assert.match(JSON.stringify(topRow(app, "决策平台")), /发给 OpenAI/);
  assert.ok(!JSON.stringify(topRow(app, "决策平台")).includes("免费层"));
  // Back to the saved one is no change, and the default platform leaves the file as it was.
  choose("gemini");
  assert.equal(decision((d) => d.changed()), false);
  assert.deepEqual(Object.keys(decisionFile(app)), ["version", "decision_model", "seats"]);

  // OpenRouter lists its models as it is chosen, once; the picks come first, the list after.
  choose("openrouter");
  await settle();
  const listed = calls.filter((call) => call.url.startsWith("/app/decision-models"));
  assert.deepEqual(listed.map((call) => call.url), ["/app/decision-models?provider=openrouter"]);
  assert.equal(decisionFile(app).decision_model, "liquid/d1.2-ultra");
  const options = (select) => JSON.parse(JSON.stringify(select.children.filter((child) => child && child.tagName === "optgroup").map((group) => ({ label: group.attrs.label, values: group.children.map((option) => option.attrs.value) }))));
  assert.deepEqual(options(control(app, "decision/model")), [
    { label: "推荐", values: ["liquid/d1.2-ultra", "liquid/d1.2"] },
    { label: "可用模型", values: ["zeta/decider-small"] },
  ]);
  choose("openai");
  choose("openrouter");
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).length, 1, "kept for the session");
  control(app, "decision/model-refresh").listeners.click();
  await settle();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).at(-1).url, "/app/decision-models?provider=openrouter&refresh=1");

  // A model from the list, or typed by hand, goes into the file; an empty one blocks saving.
  control(app, "decision/model").listeners.change({ target: { value: "zeta/decider-small" } });
  assert.equal(decisionFile(app).decision_model, "zeta/decider-small");
  assert.equal(control(app, "decision/model-id"), null, "a model from the list needs no typing");
  control(app, "decision/model").listeners.change({ target: { value: "__manual__" } });
  assert.equal(control(app, "decision/model-id").attrs.value, "zeta/decider-small");
  control(app, "decision/model-id").listeners.input({ target: { value: "vendor/custom:free" } });
  assert.equal(decisionFile(app).decision_model, "vendor/custom:free");
  assert.equal(decision((d) => d.problem()), null);
  control(app, "decision/model-id").listeners.input({ target: { value: "  " } });
  assert.equal(decision((d) => d.problem()), "请填写决策模型");
  control(app, "decision/model-id").listeners.input({ target: { value: "模型" } });
  assert.match(decision((d) => d.problem()), /可见 ASCII/);
  control(app, "decision/model-id").listeners.input({ target: { value: "vendor/custom:free" } });
  await decision((d) => d.save());
  const post = calls.filter((call) => call.url === "/app/decision-seats" && call.method === "POST").at(-1);
  assert.deepEqual(post.body, { version: 1, decision_provider: "openrouter", decision_model: "vendor/custom:free", seats: [] });
  assert.equal(decision((d) => d.changed()), false);

  // Dropping the draft puts back what is saved.
  choose("openai");
  decision((d) => d.discard());
  app.run("renderOrchestration()");
  assert.equal(decision((d) => d.changed()), false);
  assert.equal(decisionFile(app).decision_provider, "openrouter");
});

test("going back to Gemini saves its own model whatever the saved platform was", async () => {
  const written = { version: 1, decision_provider: "openai", decision_model: "gpt-6-luna", decision_base_url: "https://old.example.com/v1", seats: [] };
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(written));
  await decision((d) => d.load());
  control(app, "decision/provider").listeners.change({ target: { value: "gemini" } });
  assert.equal(decision((d) => d.changed()), true);
  assert.equal(decision((d) => d.problem()), null);
  await decision((d) => d.save());
  const post = calls.filter((call) => call.url === "/app/decision-seats" && call.method === "POST").at(-1);
  assert.deepEqual(post.body, { version: 1, decision_model: "gemini-decision-1", seats: [] });
  assert.equal(decision((d) => d.changed()), false);
});

test("the model list is asked for only when the decision panel is open, on choosing, after a key and on refresh, never by drawing the page", async () => {
  const { app, calls, decision } = decisionEditor("split", PLATFORM_FILE({ version: 1, decision_provider: "ollama", decision_model: "system-one:1b", seats: [] }), null, () => ({ models: [{ id: "system-one:1b", name: "system-one:1b" }] }));
  const asked = () => calls.filter((call) => call.url.startsWith("/app/decision-models")).length;
  // Loading while another mode or another settings tab is active asks nothing.
  await decision((d) => d.load());
  await settle();
  assert.equal(asked(), 0, "another orchestration mode");
  app.run('$("pane-orchestration").hidden = true; viewMode("decision");');
  await decision((d) => d.load());
  await settle();
  assert.equal(asked(), 0, "another settings tab");
  // Opening the decision panel in the orchestration tab asks once.
  app.run('$("pane-orchestration").hidden = false; viewMode("decision");');
  await settle();
  assert.equal(asked(), 1, "once the decision panel is open");
  // A half-typed address and any number of redraws ask nothing.
  const address = (value) => control(app, "decision/address").listeners.input({ target: { value } });
  address("http://localhost:1");
  address("http://localhost:11");
  app.run("renderOrchestration()");
  app.run("renderOrchestration()");
  await settle();
  assert.equal(asked(), 1);
  // Refresh asks for the address as it stands.
  control(app, "decision/model-refresh").listeners.click();
  await settle();
  assert.equal(asked(), 2);
  assert.match(calls.filter((call) => call.url.startsWith("/app/decision-models")).at(-1).url, /base_url=http%3A%2F%2Flocalhost%3A11&refresh=1$/);
  // Choosing another platform asks for its list once.
  control(app, "decision/provider").listeners.change({ target: { value: "openrouter" } });
  await settle();
  assert.equal(asked(), 3);
  app.run("renderOrchestration()");
  assert.equal(asked(), 3);
  // A draft that stands when the file is read again keeps the page from asking.
  await decision((d) => d.load());
  assert.equal(asked(), 3);
});

test("the choose-a-model entry cannot be picked, and an empty pick changes nothing", async () => {
  const { app, decision } = decisionEditor("decision", PLATFORM_FILE(), null, () => ({ models: [{ id: "zeta/decider-small", name: "zeta/decider-small" }] }));
  await decision((d) => d.load());
  control(app, "decision/provider").listeners.change({ target: { value: "openrouter" } });
  await settle();
  const options = control(app, "decision/model").children.filter((child) => child && child.tagName === "option");
  assert.equal(options[0].attrs.value, "");
  assert.equal(options[0].attrs.disabled, "");
  control(app, "decision/model").listeners.change({ target: { value: "__manual__" } });
  assert.ok(control(app, "decision/model-id"));
  control(app, "decision/model").listeners.change({ target: { value: "" } });
  assert.equal(decisionFile(app).decision_model, "liquid/d1.2-ultra");
  assert.equal(control(app, "decision/model-id"), null, "back to the list, nothing chosen");
});

test("each platform keeps its own key, saved at once and never shown again", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(null, { gemini: true }));
  await decision((d) => d.load());
  const choose = (id) => control(app, "decision/provider").listeners.change({ target: { value: id } });
  const keyRow = (name) => topRow(app, `${name} 密钥`);
  assert.match(JSON.stringify(keyRow("Gemini")), /已保存，粘贴新密钥可替换/);
  choose("openai");
  assert.ok(!topRow(app, "Gemini 密钥"));
  assert.match(JSON.stringify(keyRow("OpenAI")), /粘贴密钥/, "another platform's key is not this one's");
  const [input, save, clear] = keyRow("OpenAI").children[1].children[0].children;
  assert.equal(clear.disabled, true);
  input.listeners.input({ target: { value: "sk-openai-secret" } });
  await save.listeners.click();
  assert.deepEqual(calls.filter((call) => call.url === "/app/decision-key").map((call) => call.body), [{ provider: "openai", key: "sk-openai-secret" }]);
  assert.ok(!JSON.stringify(roleChildren(app)).includes("sk-openai-secret"));
  assert.match(JSON.stringify(keyRow("OpenAI")), /已保存，粘贴新密钥可替换/);
  choose("gemini");
  assert.match(JSON.stringify(keyRow("Gemini")), /已保存，粘贴新密钥可替换/);
  // A key is not a draft: it is stored, so the page has nothing to save for it.
  assert.equal(decision((d) => d.changed()), false);
  // Custom says its key is optional; a local Ollama has none.
  choose("custom");
  assert.ok(topRow(app, "自定义端点 密钥（可选）"));
  choose("ollama");
  assert.ok(!topRows(app).some((row) => JSON.stringify(row).includes("密钥")));
});

test("TypeSafe lists its models only once its key is saved", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(), null, () => ({ models: [{ id: "jev-mini", name: "jev-mini" }] }));
  await decision((d) => d.load());
  control(app, "decision/provider").listeners.change({ target: { value: "typesafe" } });
  await settle();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).length, 0);
  assert.equal(decisionFile(app).decision_model, "jev-latest");
  const [input, save] = topRow(app, "TypeSafe AI 密钥").children[1].children[0].children;
  input.listeners.input({ target: { value: "ts-key" } });
  await save.listeners.click();
  await settle();
  assert.deepEqual(calls.filter((call) => call.url.startsWith("/app/decision-models")).map((call) => call.url), ["/app/decision-models?provider=typesafe&refresh=1"]);
  assert.deepEqual(Array.from(control(app, "decision/model").children.filter((child) => child && child.tagName === "optgroup").map((group) => group.attrs.label)), ["推荐", "可用模型"]);
});

test("a local Ollama and a custom endpoint take an address, which is checked before saving", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(), null, (url) => (url.searchParams.get("provider") === "custom" ? { error: "模型目录返回 404" } : { models: [{ id: "system-one:1b", name: "system-one:1b" }] }));
  await decision((d) => d.load());
  const choose = (id) => control(app, "decision/provider").listeners.change({ target: { value: id } });
  const address = (value) => control(app, "decision/address").listeners.input({ target: { value } });
  choose("ollama");
  await settle();
  // The default address is asked for when none is typed, and what is typed is not required.
  assert.deepEqual(calls.filter((call) => call.url.startsWith("/app/decision-models")).map((call) => call.url), [`/app/decision-models?provider=ollama&base_url=${encodeURIComponent("http://127.0.0.1:11434")}`]);
  assert.equal(control(app, "decision/address").attrs.placeholder, "http://127.0.0.1:11434");
  assert.match(JSON.stringify(topRow(app, "决策平台")), /本机地址且运行本地模型时，内容不出本机/);
  control(app, "decision/model").listeners.change({ target: { value: "system-one:1b" } });
  assert.equal(decision((d) => d.problem()), null);
  assert.deepEqual(decisionFile(app), { version: 1, decision_provider: "ollama", decision_model: "system-one:1b", seats: [] });
  address(" http://localhost:11434/v1/ ");
  assert.equal(decisionFile(app).decision_base_url, "http://localhost:11434/v1");
  for (const bad of ["http://example.com", "https://user@example.com", "https://example.com/v1?key=1", "https://example.com/#x", "https://example.com/v1?", "https://example.com/v1#", "https://example.com:0/v1", "https://example.com:/v1", "http://localhost:0", "http://[::ffff:127.0.0.1]:11434", "ftp://localhost", "not a url", `https://example.com/${"a".repeat(512)}`]) {
    address(bad);
    assert.match(decision((d) => d.problem()), /API/, bad);
  }
  address("https://models.example.com/v1");
  assert.equal(decision((d) => d.problem()), null);

  // A custom endpoint needs its address, and its list is read only when asked for.
  choose("custom");
  await settle();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).length, 1);
  control(app, "decision/model-refresh").listeners.click();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).length, 1, "no address, nothing to ask");
  assert.ok(topRow(app, "请填写 API 地址"));
  assert.equal(decision((d) => d.problem()), "请填写 API 地址");
  address("http://127.0.0.1:8080/v1");
  assert.equal(decision((d) => d.problem()), "请填写决策模型");
  control(app, "decision/model-id").listeners.input({ target: { value: "decider-large" } });
  assert.equal(decision((d) => d.problem()), null);
  control(app, "decision/model-refresh").listeners.click();
  await settle();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).at(-1).url, `/app/decision-models?provider=custom&base_url=${encodeURIComponent("http://127.0.0.1:8080/v1")}&refresh=1`);
  assert.ok(topRow(app, "模型目录返回 404"));
  await decision((d) => d.save());
  assert.deepEqual(calls.filter((call) => call.url === "/app/decision-seats" && call.method === "POST").at(-1).body, { version: 1, decision_provider: "custom", decision_model: "decider-large", decision_base_url: "http://127.0.0.1:8080/v1", seats: [] });
});

test("a saved platform comes back as it was written and is no draft", async () => {
  const written = { version: 1, decision_provider: "ollama", decision_model: "system-one:7b", decision_base_url: "http://127.0.0.1:11434", seats: [] };
  const { app, decision } = decisionEditor("decision", PLATFORM_FILE(written), null, () => ({ models: [{ id: "system-one:7b", name: "system-one:7b" }] }));
  await decision((d) => d.load());
  await settle();
  assert.equal(decision((d) => d.changed()), false);
  assert.deepEqual(decisionFile(app), written);
  assert.equal(control(app, "decision/address").attrs.value, "http://127.0.0.1:11434");
  assert.equal(control(app, "decision/model-id"), null);
  assert.equal(control(app, "decision/model").children.flatMap((child) => child.children || [child]).find((option) => option.attrs?.selected === "").attrs.value, "system-one:7b");
  // The tray that does not know the platforms yet still gives the one it always had.
  const old = decisionEditor("decision", { document: null, error: null, key_set: true, key_error: null });
  await old.decision((d) => d.load());
  assert.equal(control(old.app, "decision/provider").children.length, 1);
  assert.match(JSON.stringify(topRow(old.app, "Gemini 密钥")), /已保存，粘贴新密钥可替换/);
  assert.deepEqual(Object.keys(decisionFile(old.app)), ["version", "decision_model", "seats"]);
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

test("a Claude seat can run as a named account, which dedupes by CLI, account and model", () => {
  const app = advancedEditor();
  app.run(`
    detected.claude = { installed: true, models: [{ id: "opus" }] };
    AN.settings.accounts = [{ id: "a1", label: "工作", dir: "/a1", managed: true }];
    reviewSeats = { "security-reviewer": { provider: "claude", model: "opus", account: "a1" } };
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
  const clis = find("security-reviewer/cli").children.flat().map((option) => option.attrs.value);
  assert.ok(clis.includes("claude") && clis.includes("claude@a1"));
  find("security-reviewer/fb-add").listeners.click();
  const [fallback] = seat().fallbacks;
  assert.notDeepEqual([fallback.provider, fallback.account ?? null], ["claude", "a1"], "never the seat's own account and model");
  find("security-reviewer/cli").listeners.change({ target: { value: "claude" } });
  assert.equal(seat().account, undefined, "the default account carries no field");
  find("security-reviewer/cli").listeners.change({ target: { value: "claude@a1" } });
  assert.equal(seat().account, "a1");
  assert.equal(seat().model, "opus", "the model stays when the account changes");
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
  toggleRow(cardsOf(app)[0]);
  const rows = rowBody(cardsOf(app)[0]).filter((child) => child.className === "s-wrap");
  assert.equal(rows.length, 1, "only the easy level has a fallback line");
  assert.equal(rows[0].children[1].className, "s-row is-sub");
  assert.equal(rows[0].children[0].children.at(-1).tagName, "button");
});

test("an image seat row adds and removes fallbacks, with the delete button kept last", async () => {
  const img = { id: "img-a", kind: "image", role: "漫画风格", description: "漫画风格", provider: "codex", model: "chat-model", access: "write" };
  const saved = { document: { version: 1, seats: [img] }, error: null, key_set: false, key_error: null };
  const { app, decision } = decisionEditor("decision", saved);
  app.run('detected.codex.models = [{ id: "chat-model" }, { id: "m2" }]');
  await decision((d) => d.load());
  toggleRow(cardsOf(app)[0]);
  const imageRow = () => rowBody(cardsOf(app)[0])[0];
  const fallbacks = () => JSON.parse(app.run("JSON.stringify(window.AgentNaveDecisionSeats.document().seats[0].fallbacks || null)"));
  const [add, remove] = [imageRow().children.at(-1), imageRow().children[1].children.at(-1)];
  assert.equal(add.children[0], "添加备选");
  assert.equal(remove.children[0], "删除");
  add.listeners.click();
  assert.deepEqual(fallbacks(), [{ provider: "codex", model: "m2" }]);
  const block = rowBody(cardsOf(app)[0])[0];
  assert.equal(block.className, "s-wrap");
  assert.equal(block.children[0].children[1].children.at(-1).children[0], "删除");
  block.children[1].children[1].children.at(-1).listeners.click();
  assert.equal(fallbacks(), null, "no fallback left, no field");
});

test("expandable decision seat rows keep state across re-renders, force-open invalid cards, and summarize tiers", async () => {
  const seat = (id, kind, role, difficulty, model) => ({ id, kind, role, description: role, provider: "codex", model, access: "write", difficulty });
  const saved = { document: { version: 1, seats: [
    seat("w-1", "worker", "通用执行", "easy", "m-easy"),
    seat("w-2", "worker", "通用执行", "hard", "m-hard"),
    seat("c-1", "custom", "脚本", "medium", "m-mid"),
  ] }, error: null, key_set: false, key_error: null };
  const { app, decision } = decisionEditor("decision", saved);
  app.run('detected.codex.models = [{ id: "m-easy" }, { id: "m-mid" }, { id: "m-hard" }]');
  await decision((d) => d.load());

  // Loaded valid cards start collapsed with tier summaries omitting unset levels.
  const rows = () => Array.from(cardsOf(app));
  assert.deepEqual(rows().map((r) => r.dataset.expanded), ["false", "false"]);
  assert.equal(rowHead(rows()[0]).summary.children[0], "简单 m-easy · 困难 m-hard");
  assert.equal(rowHead(rows()[1]).summary.children[0], "中等 m-mid");

  // Expanding multiple cards persists across re-renders (e.g. changing a tier pick).
  toggleRow(rows()[0]);
  toggleRow(rows()[1]);
  assert.deepEqual(rows().map((r) => r.dataset.expanded), ["true", "true"]);
  rowBody(rows()[0])[1].children[1].children[0].listeners.change({ target: { value: "codex" } });
  assert.deepEqual(rows().map((r) => r.dataset.expanded), ["true", "true"]);
  assert.equal(rowHead(rows()[0]).summary.children[0], "简单 m-easy · 中等 m-easy · 困难 m-hard");

  // Collapsing card 1, then adding a duplicate custom card forces both colliding cards open.
  toggleRow(rows()[1]);
  assert.equal(rows()[1].dataset.expanded, "false");
  app.run('window.AgentNaveDecisionSeats.addCard("custom")');
  assert.equal(rows()[2].dataset.expanded, "true", "newly added card auto-expands");
  // Trying to collapse an invalid card (missing specialty + no tiers) keeps it open.
  toggleRow(rows()[2]);
  assert.equal(rows()[2].dataset.expanded, "true");
  // Typing a duplicate specialty forces the previously collapsed colliding card open too.
  rowBody(rows()[2])[0].children[1].children[0].listeners.input({ target: { value: "脚本" } });
  assert.equal(rows()[1].dataset.expanded, "true");
  assert.equal(rows()[1].dataset.invalid, "true");

  // Preset rule text fold is hidden for presets without text and shown (collapsed) when text exists.
  assert.equal(app.run('$("orchestration-text-fold").hidden'), false);
  app.run('viewMode("off")');
  assert.equal(app.run('$("orchestration-text-fold").hidden'), true);
});

function accountCardsEditor(fetch) {
  const app = editor(fetch);
  app.run(`
    AN.L = (zh) => zh;
    AN.cliName = (p) => ({ claude: "Claude Code", codex: "Codex" }[p] || p);
    AN.hostTile = (p) => AN.settings.el("span", { className: "host-tile", dataset: { provider: p } });
    AN.settings.L = AN.L;
    AN.settings.renderLeaveConfirm = () => {};
    AN.settings.renderAdvancedBar = () => {};
    AN.settings.renderReviewSeats = () => {};
    AN.settings.seatConflicts = () => [];
    AN.settings.seatConflictText = () => "";
    AN.settings.memberSaveErrors = () => [];
    const nodes = new Map();
    const dialogs = [];
    const makeNode = (tag) => {
      const n = {
        tag, tagName: tag, attrs: {}, children: [], dataset: {}, listeners: {}, options: [], value: "",
        classList: { toggle() {} },
        setAttribute(k, v) { this.attrs[k] = v; if (k === "value") this.value = v; },
        removeAttribute(k) { delete this.attrs[k]; },
        addEventListener(type, fn) { this.listeners[type] = fn; },
        appendChild(child) {
          if (child == null) return;
          if (child.tag === "fragment") this.children.push(...child.children);
          else this.children.push(child);
        },
        append(...kids) {
          for (const k of kids.flat(Infinity)) this.appendChild(k);
        },
        replaceChildren(...kids) {
          this.children = [];
          this.append(...kids);
        },
        querySelectorAll: () => [],
        querySelector: () => null,
        getBoundingClientRect: () => ({ top: 0 }),
        focus() {},
        blur() {},
        showModal() { this.open = true; },
        close() { this.open = false; },
        remove() { this.removed = true; },
      };
      return n;
    };
    document.createElement = makeNode;
    document.createDocumentFragment = () => makeNode("fragment");
    document.createTextNode = (text) => text;
    document.getElementById = (id) => nodes.get(id) || nodes.set(id, makeNode("div")).get(id);
    document.querySelector = () => null;
    document.body = {
      children: dialogs,
      appendChild(child) { dialogs.push(child); },
    };
    globalThis.__dialogs = dialogs;
    function findKey(nodes, key) {
      for (const node of [nodes].flat(Infinity)) {
        if (!node || typeof node !== "object") continue;
        if (node.dataset?.focusKey === key || node.attrs?.dataset?.focusKey === key) return node;
        const found = findKey(node.children, key);
        if (found) return found;
      }
      return null;
    }
    function collectByClass(nodes, cls, out = []) {
      for (const node of [nodes].flat(Infinity)) {
        if (!node || typeof node !== "object") continue;
        const cn = node.className || node.attrs?.className || "";
        if (cn.split(/\\s+/).includes(cls)) out.push(node);
        collectByClass(node.children, cls, out);
      }
      return out;
    }
    globalThis.findKey = findKey;
    globalThis.collectByClass = collectByClass;
    detected = {
      claude: { installed: true, version: "2.1.0", models: [{ id: "sonnet" }], efforts: [] },
      codex: { installed: true, version: "0.161.0", models: [{ id: "gpt-6.1-sol" }], efforts: [] },
    };
    currentDoc = {
      version: 1,
      clis: {},
      modes: {
        task: { claude: { models: [], overrides: {} }, codex: { models: [], overrides: {} } },
        discussion: { claude: { models: [], overrides: {} }, codex: { models: [], overrides: {} } },
      },
    };
    markSaved();
  `);
  app.load("member-updates.js");
  app.load("account-settings.js");
  app.load("member-cards.js");
  return app;
}

test("employees render as a compact left CLI list, with Claude default status at top and accounts expanding in place inside Claude Code detail", async () => {
  let defaultSignedIn = true;
  const loginCalls = [];
  const app = accountCardsEditor(async (url, options = {}) => {
    if (url === "/app/accounts/login") {
      loginCalls.push(JSON.parse(options.body));
      return { ok: true, json: async () => ({ session: 55 }) };
    }
    if (url.startsWith("/app/accounts/status?id=default")) {
      return {
        ok: true,
        json: async () => defaultSignedIn
          ? { logged_in: true, method: "claude.ai", email: "tim@example.com", plan: "max", error: null }
          : { logged_in: false, method: "none", email: null, plan: null, error: null },
      };
    }
    if (url.startsWith("/app/accounts/status?id=a1")) {
      return { ok: true, json: async () => ({ logged_in: true, method: "claude.ai", email: "work@example.com", plan: "pro", error: null }) };
    }
    if (url.startsWith("/app/accounts/status?id=a2")) {
      return { ok: true, json: async () => ({ logged_in: false, method: "none", email: null, plan: null, error: null }) };
    }
    return { ok: true, json: async () => ({}) };
  });
  app.run(`
    AN.settings.accounts = [];
    renderCards();
  `);
  await new Promise(setImmediate);

  // Left list lists CLIs only, with no Add account button in the list pane; Claude shows sign-in status without email.
  const listItems = app.run('collectByClass($("cli-cards").children, "member-list-item")');
  assert.deepEqual(Array.from(listItems, (c) => c.dataset.memberKey), ["cli:codex", "cli:claude"]);
  for (const item of listItems) {
    assert.equal(app.run(`((n) => JSON.stringify(n))`)(item).includes('"select"'), false, "left list rows have no selects");
    const buttons = app.run(`((node) => { const out = []; const walk = (n) => { if (!n || typeof n !== "object") return; if (n.tag === "button" || n.tagName === "button") out.push(n); for (const c of n.children || []) walk(c); }; for (const c of node.children || []) walk(c); return out.length; })`)(item);
    assert.equal(buttons, 0, "left list rows have no nested buttons");
  }
  assert.equal(app.run('findKey(collectByClass($("cli-cards").children, "members-list-pane"), "claude/accounts/add")'), null);
  const claudeListJson = JSON.stringify(listItems[1]);
  assert.match(claudeListJson, /已登录 · max/);
  assert.doesNotMatch(claudeListJson, /tim@example\.com|ti\*\*\*/);

  // Default selection is the first row (codex); Remove sits in detail footer, not in header.
  assert.equal(listItems[0].attrs["aria-pressed"], "true");
  const codexDetail = app.run('collectByClass($("cli-cards").children, "detail-panel")[0]');
  assert.equal(codexDetail.dataset.provider, "codex");
  assert.equal(app.run('findKey(collectByClass($("cli-cards").children, "detail-head"), "codex/membership")'), null);
  assert.ok(app.run('findKey(collectByClass($("cli-cards").children, "detail-foot"), "codex/membership")'));
  assert.equal(app.run('collectByClass(collectByClass($("cli-cards").children, "detail-panel")[0], "claude-accounts-row").length'), 0);

  // Selecting Claude Code shows default account status bar at top and Default row in Accounts list even with 0 named accounts.
  app.run('findKey($("cli-cards").children, "select/cli:claude").listeners.click()');
  const topBar = () => app.run('collectByClass($("cli-cards").children, "detail-account-bar")[0]');
  assert.ok(topBar());
  assert.match(JSON.stringify(topBar()), /已登录/);
  assert.match(JSON.stringify(topBar()), /ti\*\*\*@\*\*\*\.com/);
  assert.match(JSON.stringify(topBar()), /max/);
  assert.doesNotMatch(JSON.stringify(topBar()), /tim@example\.com/);

  // Clicking the masked email toggles reveal/hide.
  app.run('findKey($("cli-cards").children, "claude/default-email").listeners.click()');
  assert.match(JSON.stringify(topBar()), /tim@example\.com/);
  app.run('findKey($("cli-cards").children, "claude/default-email").listeners.click()');
  assert.match(JSON.stringify(topBar()), /ti\*\*\*@\*\*\*\.com/);

  assert.equal(app.run('collectByClass($("cli-cards").children, "claude-accounts-row").length'), 1);
  assert.equal(app.run('collectByClass($("cli-cards").children, "account-item").length'), 1);
  assert.ok(app.run('findKey($("cli-cards").children, "claude/accounts/default/toggle")'));
  assert.ok(app.run('findKey(collectByClass($("cli-cards").children, "claude-accounts-row")[0], "claude/accounts/add")'));

  // With named accounts, Default sits first, followed by named accounts (all collapsed initially).
  app.run(`
    AN.settings.accounts = [
      { id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true },
      { id: "a2", provider: "claude", label: "个人", dir: "/ext/p", managed: false },
    ];
    renderCards();
  `);
  await new Promise(setImmediate);

  const leftAfter = app.run('collectByClass($("cli-cards").children, "member-list-item")');
  assert.deepEqual(Array.from(leftAfter, (c) => c.dataset.memberKey), ["cli:codex", "cli:claude"]);

  const detailRows = app.run('collectByClass($("cli-cards").children, "detail-rows")[0]');
  assert.equal(detailRows.children.at(-2).className, "s-row claude-accounts-row", "Accounts group sits after settings items and before footer");

  const accountItems = app.run('collectByClass($("cli-cards").children, "account-item")');
  assert.equal(accountItems.length, 3);
  assert.equal(app.run('collectByClass($("cli-cards").children, "account-item-body").length'), 0);
  assert.match(JSON.stringify(accountItems[0]), /默认/);
  assert.match(JSON.stringify(accountItems[0]), /ti\*\*\*@\*\*\*\.com · max/);
  assert.match(JSON.stringify(accountItems[1]), /wo\*\*\*@\*\*\*\.com · pro/);
  assert.match(JSON.stringify(accountItems[2]), /未登录/);

  // Expanding Default shows only Sign-in status + Sign in again (no display name input, no remove button).
  app.run('findKey($("cli-cards").children, "claude/accounts/default/toggle").listeners.click()');
  assert.equal(app.run('expandedAccountId'), "default");
  assert.equal(app.run('findKey($("cli-cards").children, "claude/accounts/default/label-input")'), null);
  assert.equal(app.run('findKey($("cli-cards").children, "claude/accounts/default/remove")'), null);
  const defaultLoginBtn = app.run('findKey($("cli-cards").children, "claude/accounts/default/login")');
  assert.ok(defaultLoginBtn);
  defaultLoginBtn.listeners.click();
  await new Promise(setImmediate);
  assert.deepEqual(loginCalls[0], { id: "default", cols: 96, rows: 22 });

  // Clicking account a1 collapses default and expands a1; clicking a2 collapses a1 and expands a2.
  app.run('findKey($("cli-cards").children, "claude/accounts/a1/toggle").listeners.click()');
  assert.equal(app.run('expandedAccountId'), "a1");
  const a1Bodies = app.run('collectByClass($("cli-cards").children, "account-item-body")');
  assert.equal(a1Bodies.length, 1);
  const a1Json = JSON.stringify(a1Bodies[0]);
  assert.match(a1Json, /显示名/);
  assert.match(a1Json, /登录状态/);
  assert.match(a1Json, /已登录/);
  assert.match(a1Json, /wo\*\*\*@\*\*\*\.com/);
  assert.match(a1Json, /pro/);
  assert.match(a1Json, /重新登录/);
  assert.match(a1Json, /移除/);
  assert.doesNotMatch(a1Json, /托管|由谁管理/);

  app.run('findKey($("cli-cards").children, "claude/accounts/a2/toggle").listeners.click()');
  assert.equal(app.run('expandedAccountId'), "a2");
  const a2Bodies = app.run('collectByClass($("cli-cards").children, "account-item-body")');
  assert.equal(a2Bodies.length, 1);
  const a2Json = JSON.stringify(a2Bodies[0]);
  assert.match(a2Json, /未登录/);
  assert.match(a2Json, /"登录"/);

  // Clicking the expanded row again collapses it.
  app.run('findKey($("cli-cards").children, "claude/accounts/a2/toggle").listeners.click()');
  assert.equal(app.run('expandedAccountId'), null);
  assert.equal(app.run('collectByClass($("cli-cards").children, "account-item-body").length'), 0);

  // When default is signed out, both top status and left list show 未登录.
  defaultSignedIn = false;
  app.run('findKey($("cli-cards").children, "claude/accounts/default/toggle").listeners.click()');
  // Re-reading via mock message for session 55 triggers readState("default", true).
  app.listeners.message({ data: { type: "agentnave:terminal", action: "exited", session: 55, code: 0 } });
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.match(JSON.stringify(topBar()), /未登录/);
  assert.match(JSON.stringify(app.run('findKey($("cli-cards").children, "select/cli:claude")')), /未登录/);
});

test("left list reflects cooldown, update available, unsaved changes, and account/CLI validation errors on CLI rows", async () => {
  const app = accountCardsEditor(async (url) => {
    if (url.startsWith("/app/accounts/status?id=default")) {
      return { ok: true, json: async () => ({ logged_in: true, method: "claude.ai", email: "tim@example.com", plan: "max", error: null }) };
    }
    if (url.startsWith("/app/accounts/status?id=a1")) {
      return { ok: true, json: async () => ({ logged_in: true, method: "claude.ai", email: "w@x.com", plan: "Pro", error: null }) };
    }
    return { ok: true, json: async () => ({}) };
  });
  const until = new Date(Date.now() + 60000).toISOString();
  app.run(`
    AN.settings.accounts = [{ id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true }];
    cliUpdates = { clis: { codex: { update: true, latest: "0.162.0", version: "0.161.0" } } };
    providerHealth = { claude: { accounts: { a1: { default: { code: "quota", until: ${JSON.stringify(until)} } } } } };
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  await new Promise(setImmediate);

  const codexRow = () => app.run('findKey($("cli-cards").children, "select/cli:codex")');
  const claudeRow = () => app.run('findKey($("cli-cards").children, "select/cli:claude")');
  assert.match(JSON.stringify(codexRow()), /有更新/);
  const a1Toggle = app.run('findKey($("cli-cards").children, "claude/accounts/a1/toggle")');
  assert.match(JSON.stringify(a1Toggle), /冷却中/);

  // Unsaved rename draft and invalid empty rename draft mark the Claude Code row in the left list.
  app.run('findKey($("cli-cards").children, "claude/accounts/a1/toggle").listeners.click()');
  const labelInput = app.run('findKey($("cli-cards").children, "claude/accounts/a1/label-input")');
  labelInput.listeners.input({ target: { value: "新工作" } });
  app.run("renderCards()");
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-dirty").length'), 1);
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-error").length'), 0);

  labelInput.listeners.input({ target: { value: "   " } });
  app.run("renderCards()");
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-error").length'), 1);

  labelInput.listeners.input({ target: { value: "工作" } });
  app.run("renderCards()");
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-dirty").length'), 0);
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-error").length'), 0);

  // Making an unsaved pool edit and clearing the pool also marks the Claude CLI row with dirty & error flags.
  app.run('writePool("claude", [])');
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-dirty").length'), 1);
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:claude"), "is-error").length'), 1);
  assert.equal(app.run('collectByClass(findKey($("cli-cards").children, "select/cli:codex"), "is-dirty").length'), 0);
  assert.match(JSON.stringify(app.run('collectByClass($("cli-cards").children, "detail-panel")[0]')), /至少选一个模型/);
  assert.ok(claudeRow());
});

test("account detail display name edits directly and commits rename on Enter", async () => {
  const calls = [];
  const app = accountCardsEditor(async (url, options) => {
    if (url === "/app/accounts/rename") {
      const body = JSON.parse(options.body);
      calls.push(body);
      return { ok: true, json: async () => ({ state: { accounts: [{ id: "a1", provider: "claude", label: body.label, dir: "/m/a1", managed: true }], error: null } }) };
    }
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  app.run(`
    AN.settings.accounts = [{ id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true }];
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
    findKey($("cli-cards").children, "claude/accounts/a1/toggle").listeners.click();
  `);
  await new Promise(setImmediate);

  const input = app.run('findKey($("cli-cards").children, "claude/accounts/a1/label-input")');
  assert.ok(input);
  assert.equal(input.value, "工作");

  input.value = "公司";
  input.listeners.keydown({ key: "Enter", target: input, preventDefault() {} });
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.deepEqual(calls, [{ id: "a1", label: "公司" }]);
  assert.match(JSON.stringify(app.run('findKey($("cli-cards").children, "claude/accounts/a1/toggle")')), /公司/);
});

test("add account wizard creates a managed account in step 1 and runs in-dialog login in step 2", async () => {
  const calls = [];
  let signedIn = false;
  const app = accountCardsEditor(async (url, options = {}) => {
    const body = options.body ? JSON.parse(options.body) : null;
    calls.push([url, body]);
    if (url === "/app/accounts/create") {
      const account = { id: "a1", provider: "claude", label: body.label, dir: "/m/a1", managed: true };
      return { ok: true, json: async () => ({ account, sync: { ok: true }, state: { accounts: [account], error: null } }) };
    }
    if (url === "/app/accounts/login") {
      return { ok: true, json: async () => ({ session: 77 }) };
    }
    if (url.startsWith("/app/accounts/status?id=a1")) {
      return {
        ok: true,
        json: async () => signedIn
          ? { logged_in: true, method: "claude.ai", email: "work@example.com", plan: "max", error: null }
          : { logged_in: false, method: "none", email: null, plan: null, error: null },
      };
    }
    if (url.startsWith("/app/accounts/status?id=default")) {
      return { ok: true, json: async () => ({ logged_in: true, method: "claude.ai", email: "tim@example.com", plan: "max", error: null }) };
    }
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  app.run(`
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/add").listeners.click()');
  await new Promise(setImmediate);

  const addDlg = () => app.run("__dialogs[0]");
  assert.equal(addDlg().open, true);
  assert.match(JSON.stringify(addDlg()), /添加账号/);
  assert.doesNotMatch(JSON.stringify(addDlg()), /\.agentnave-accounts|\/m\//);

  const nameInput = app.run('findKey(__dialogs[0].children, "claude/accounts/new")');
  const submitBtn = app.run('findKey(__dialogs[0].children, "claude/accounts/create")');
  assert.equal(submitBtn.disabled, true);
  nameInput.value = "工作";
  nameInput.listeners.input({ target: nameInput });
  assert.equal(submitBtn.disabled, false);

  app.run('findKey(__dialogs[0].children, "claude/accounts/new") && findKey(__dialogs[0].children, "claude/accounts/new").value && __dialogs[0].children[2].children[0].listeners.submit({ preventDefault() {} })');
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.deepEqual(calls.filter(([u]) => !u.startsWith("/app/accounts/status")).map(([u, b]) => [u, b]), [
    ["/app/accounts/create", { label: "工作" }],
    ["/app/accounts/login", { id: "a1", cols: 96, rows: 22 }],
  ]);

  // Same dialog transitions to Step 2 with embedded login terminal and "稍后登录".
  assert.equal(app.run("__dialogs.length"), 1);
  assert.equal(addDlg().open, true);
  assert.match(addDlg().className, /is-login-step/);
  assert.match(JSON.stringify(addDlg()), /\/terminal\/\?session=77/);
  assert.equal(app.run("selectedMember"), "cli:claude");
  assert.equal(app.run("expandedAccountId"), "a1");
  assert.ok(app.run('findKey($("cli-cards").children, "claude/accounts/a1/label-input")'));
  const finishBtn = () => app.run('findKey(__dialogs[0].children, "claude/accounts/wizard-finish")');
  assert.equal(finishBtn().children[0], "稍后登录");
  assert.equal(finishBtn().className, "");

  // When sign-in finishes, button turns into primary "完成" and masked email is shown.
  signedIn = true;
  app.listeners.message({ data: { type: "agentnave:terminal", action: "exited", session: 77, code: 0 } });
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.equal(finishBtn().children[0], "完成");
  assert.equal(finishBtn().className, "primary");
  assert.match(JSON.stringify(addDlg()), /wo\*\*\*@\*\*\*\.com/);

  // Closing the wizard closes the login terminal session.
  finishBtn().listeners.click();
  await new Promise(setImmediate);
  assert.equal(addDlg().open, false);
  assert.ok(calls.some(([u]) => u === "/app/terminal/close?session=77"));
});

test("add account wizard links a discovered or typed folder as an external account", async () => {
  const calls = [];
  const app = accountCardsEditor(async (url, options = {}) => {
    const body = options.body ? JSON.parse(options.body) : null;
    calls.push([url, body]);
    if (url === "/app/accounts/discover") {
      const state = { logged_in: true, method: "claude.ai", email: "me@work.example", plan: "pro", error: null };
      return { ok: true, json: async () => ({ candidates: [{ name: ".claude_work", dir: "/h/.claude_work", state }] }) };
    }
    if (url === "/app/accounts/link") {
      if (body.dir === "~/nope") return { ok: false, status: 400, json: async () => ({ error: "目录不存在" }) };
      const account = { id: "a1", provider: "claude", label: body.label, dir: body.dir, managed: false };
      return { ok: true, json: async () => ({ account, sync: { ok: true }, state: { accounts: [account], error: null } }) };
    }
    if (url.startsWith("/app/accounts/status?id=a1")) {
      return { ok: true, json: async () => ({ logged_in: true, method: "claude.ai", email: "me@work.example", plan: "pro", error: null }) };
    }
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  const flush = async () => { for (let i = 0; i < 4; i++) await new Promise(setImmediate); };
  const key = (name) => app.run(`findKey(__dialogs[0].children, "claude/accounts/${name}")`);
  app.run(`
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/add").listeners.click()');
  await flush();

  // A discovered folder: picked from the list, linked, and no sign-in step as it is signed in.
  key("mode-existing").listeners.click();
  await flush();
  const pick = key("pick");
  assert.match(JSON.stringify(pick), /\.claude_work · m\*\*\*@\*\*\*\.example/);
  const name = key("new");
  name.value = "工作";
  name.listeners.input({ target: name });
  assert.equal(key("create").disabled, true);
  pick.listeners.change({ target: { value: "/h/.claude_work" } });
  assert.equal(key("create").disabled, false);
  app.run('__dialogs[0].children[2].children[0].listeners.submit({ preventDefault() {} })');
  await flush();
  assert.deepEqual(calls.filter(([u]) => u === "/app/accounts/link"), [["/app/accounts/link", { label: "工作", dir: "/h/.claude_work" }]]);
  assert.ok(!calls.some(([u]) => u === "/app/accounts/login"));
  assert.equal(app.run("__dialogs[0].open"), false);
  assert.equal(app.run("AN.settings.accounts[0].managed"), false);
});

test("add account wizard keeps a failed folder scan out of another mode and retries it", async () => {
  const scans = [];
  const app = accountCardsEditor(async (url) => {
    if (url === "/app/accounts/discover") return new Promise((resolve) => scans.push(resolve));
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  const flush = async () => { for (let i = 0; i < 4; i++) await new Promise(setImmediate); };
  const key = (name) => app.run(`findKey(__dialogs[0].children, "claude/accounts/${name}")`);
  app.run(`
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/add").listeners.click()');
  await flush();
  key("mode-existing").listeners.click();
  key("mode-path").listeners.click();
  scans[0]({ ok: false, json: async () => ({ error: "boom" }) });
  await flush();
  assert.doesNotMatch(JSON.stringify(app.run("__dialogs[0].children")), /boom/);
  key("mode-existing").listeners.click();
  await flush();
  assert.equal(scans.length, 2);
});

test("add account wizard shows a link error in place and keeps the typed path", async () => {
  const app = accountCardsEditor(async (url, options = {}) => {
    if (url === "/app/accounts/link") return { ok: false, status: 400, json: async () => ({ error: "目录不存在" }) };
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  const flush = async () => { for (let i = 0; i < 4; i++) await new Promise(setImmediate); };
  const key = (name) => app.run(`findKey(__dialogs[0].children, "claude/accounts/${name}")`);
  app.run(`
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/add").listeners.click()');
  await flush();
  key("mode-path").listeners.click();
  const name = key("new");
  name.value = "个人";
  name.listeners.input({ target: name });
  assert.equal(key("create").disabled, true);
  const path = key("path");
  path.value = "~/nope";
  path.listeners.input({ target: path });
  assert.equal(key("create").disabled, false);
  app.run('__dialogs[0].children[2].children[0].listeners.submit({ preventDefault() {} })');
  await flush();
  const error = app.run('collectByClass(__dialogs[0].children, "account-add-error")[0].textContent');
  assert.match(error, /目录不存在/);
  assert.equal(app.run("__dialogs[0].open"), true);
  assert.equal(key("path").value, "~/nope");
  assert.equal(key("create").disabled, false);
});

test("an emptied display name falls back on blur and clears its flag", async () => {
  const app = accountCardsEditor(async () => ({ ok: true, json: async () => ({ logged_in: false, method: "none" }) }));
  app.run(`
    AN.settings.accounts = [{ id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true }];
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
    findKey($("cli-cards").children, "claude/accounts/a1/toggle").listeners.click();
    globalThis.__flagRefreshes = 0;
    AN.settings.refreshClaudeFlags = () => { globalThis.__flagRefreshes += 1; };
  `);
  await new Promise(setImmediate);
  const input = app.run('findKey($("cli-cards").children, "claude/accounts/a1/label-input")');
  input.value = "";
  input.listeners.input({ target: input });
  const before = app.run("__flagRefreshes");
  input.listeners.blur({ target: input });
  await new Promise(setImmediate);
  assert.equal(input.value, "工作");
  assert.equal(app.run("__flagRefreshes"), before + 1);
  assert.equal(app.run("AN.settings.accountHasError()"), false);
});

test("a sign-in terminal that starts after the wizard closed is closed too", async () => {
  const calls = [];
  let started;
  const app = accountCardsEditor(async (url, options = {}) => {
    calls.push(url);
    if (url === "/app/accounts/create") {
      const account = { id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true };
      return { ok: true, json: async () => ({ account, sync: { ok: true }, state: { accounts: [account], error: null } }) };
    }
    if (url === "/app/accounts/login") {
      return new Promise((resolve) => { started = () => resolve({ ok: true, json: async () => ({ session: 88 }) }); });
    }
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  app.run(`
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/add").listeners.click()');
  await new Promise(setImmediate);
  const nameInput = app.run('findKey(__dialogs[0].children, "claude/accounts/new")');
  nameInput.value = "工作";
  nameInput.listeners.input({ target: nameInput });
  app.run('__dialogs[0].children[2].children[0].listeners.submit({ preventDefault() {} })');
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.ok(started, "the sign-in terminal was requested");
  app.run('findKey(__dialogs[0].children, "claude/accounts/wizard-finish").listeners.click()');
  assert.equal(app.run("__dialogs[0].open"), false);
  started();
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.ok(calls.includes("/app/terminal/close?session=88"));
});

test("account removal confirmation shows folder deletion only for managed accounts", async () => {
  const calls = [];
  const app = accountCardsEditor(async (url, options = {}) => {
    if (url === "/app/accounts/remove") {
      calls.push(JSON.parse(options.body));
      return { ok: true, json: async () => ({ state: { accounts: [], error: null }, deleted: false }) };
    }
    return { ok: true, json: async () => ({ logged_in: false, method: "none" }) };
  });
  app.run(`
    AN.settings.accounts = [
      { id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true },
      { id: "a2", provider: "claude", label: "个人", dir: "/ext/p", managed: false },
    ];
    renderCards();
    findKey($("cli-cards").children, "select/cli:claude").listeners.click();
    findKey($("cli-cards").children, "claude/accounts/a2/toggle").listeners.click();
  `);
  await new Promise(setImmediate);

  // Linked account: no delete-dir checkbox.
  app.run('findKey($("cli-cards").children, "claude/accounts/a2/remove").listeners.click()');
  assert.equal(app.run('findKey($("cli-cards").children, "claude/accounts/a2/delete-dir")'), null);
  app.run('findKey($("cli-cards").children, "claude/accounts/a2/confirm-remove").listeners.click()');
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.deepEqual(calls[0], { id: "a2", delete_dir: false });
  assert.equal(app.run("expandedAccountId"), null);

  // Managed account: has delete-dir checkbox, defaults to false, passes true when checked.
  app.run(`
    AN.settings.accounts = [{ id: "a1", provider: "claude", label: "工作", dir: "/m/a1", managed: true }];
    renderCards();
    findKey($("cli-cards").children, "claude/accounts/a1/toggle").listeners.click();
  `);
  app.run('findKey($("cli-cards").children, "claude/accounts/a1/remove").listeners.click()');
  const checkbox = app.run('findKey($("cli-cards").children, "claude/accounts/a1/delete-dir")');
  assert.ok(checkbox);
  assert.equal(checkbox.checked, false);
  checkbox.listeners.change({ target: { checked: true } });
  app.run('findKey($("cli-cards").children, "claude/accounts/a1/confirm-remove").listeners.click()');
  await new Promise(setImmediate);
  await new Promise(setImmediate);
  assert.deepEqual(calls[1], { id: "a1", delete_dir: true });
});

// What a select shows as chosen, among its options and the options inside its groups.
const chosenOption = (select) => select.children.flatMap((child) => (child.tagName === "optgroup" ? child.children : [child])).find((option) => option.attrs.selected === "").attrs.value;

test("the model id is typed only on demand, and alone while there is no list", async () => {
  const written = { version: 1, decision_provider: "openrouter", decision_model: "liquid/d1.2", seats: [] };
  const { app, decision } = decisionEditor("decision", PLATFORM_FILE(written), null, () => ({ models: [{ id: "zeta/decider-small", name: "Zeta Small" }] }));
  await decision((d) => d.load());
  await settle();
  const choose = (id) => control(app, "decision/provider").listeners.change({ target: { value: id } });
  const model = () => control(app, "decision/model");
  const typed = () => control(app, "decision/model-id");
  const lastOption = () => model().children.at(-1);
  // A model in the list is one select and its refresh; the last entry asks for the typed id.
  assert.equal(typed(), null);
  assert.equal(chosenOption(model()), "liquid/d1.2");
  assert.equal(lastOption().attrs.value, "__manual__");
  assert.equal(lastOption().children[0], "手动输入模型 ID…");
  assert.ok(control(app, "decision/model-refresh"));
  model().listeners.change({ target: { value: "__manual__" } });
  assert.equal(chosenOption(model()), "__manual__");
  assert.equal(typed().attrs.value, "liquid/d1.2", "the model in use stays until something else is typed");
  typed().listeners.input({ target: { value: "vendor/custom:free" } });
  assert.equal(decisionFile(app).decision_model, "vendor/custom:free");
  // Picking a listed model closes the typed row again.
  model().listeners.change({ target: { value: "zeta/decider-small" } });
  assert.equal(typed(), null);
  assert.equal(chosenOption(model()), "zeta/decider-small");
  // Asking for it and leaving by another platform, or by dropping the draft, closes it too.
  model().listeners.change({ target: { value: "__manual__" } });
  assert.ok(typed());
  choose("typesafe");
  assert.equal(typed(), null);
  choose("openrouter");
  assert.equal(typed(), null);
  decision((d) => d.discard());
  app.run("renderOrchestration()");
  assert.equal(typed(), null);
  // A model that is in no list opens the row without asking, and shows as the typed entry.
  decisionFile(app);
  const unknown = decisionEditor("decision", PLATFORM_FILE({ ...written, decision_model: "vendor/custom:free" }), null, () => ({ models: [] }));
  await unknown.decision((d) => d.load());
  await settle();
  assert.equal(control(unknown.app, "decision/model-id").attrs.value, "vendor/custom:free");
  assert.equal(chosenOption(control(unknown.app, "decision/model")), "__manual__");
  unknown.decision((d) => d.discard());
});

test("without any list the model id is the row's one field, and the list brings the select back", async () => {
  const { app, calls, decision } = decisionEditor("decision", PLATFORM_FILE(), null, (url) => (url.searchParams.get("provider") === "custom" ? { models: [{ id: "decider-large", name: "decider-large" }] } : { models: [] }));
  await decision((d) => d.load());
  const choose = (id) => control(app, "decision/provider").listeners.change({ target: { value: id } });
  // Ollama has nothing pulled yet: no empty select, only the id and the refresh in one field row.
  choose("ollama");
  await settle();
  assert.equal(control(app, "decision/model"), null);
  const field = topRows(app).find((row) => findKey(row, "decision/model-refresh")).children[1].children[0];
  assert.deepEqual(Array.from(field.children.map((child) => child.dataset.focusKey)), ["decision/model-id", "decision/model-refresh"]);
  // A custom endpoint is asked for its list; once it answers, the select takes over and the typed id stays.
  choose("custom");
  await settle();
  assert.equal(control(app, "decision/model"), null);
  control(app, "decision/address").listeners.input({ target: { value: "http://127.0.0.1:8080/v1" } });
  control(app, "decision/model-id").listeners.input({ target: { value: "decider-small" } });
  control(app, "decision/model-refresh").listeners.click();
  await settle();
  assert.equal(calls.filter((call) => call.url.startsWith("/app/decision-models")).at(-1).url.includes("provider=custom"), true);
  assert.equal(chosenOption(control(app, "decision/model")), "__manual__");
  assert.equal(control(app, "decision/model-id").attrs.value, "decider-small");
  control(app, "decision/model").listeners.change({ target: { value: "decider-large" } });
  assert.equal(control(app, "decision/model-id"), null);
  assert.equal(decisionFile(app).decision_model, "decider-large");
});

test("a saved key is shown by its masked preview, which is never the field's value", async () => {
  const withPreview = { ...PLATFORM_FILE(null, { gemini: true, openai: true }), key_previews: { gemini: "AIza…KLMN", openai: "••••-key" } };
  const { app, decision, calls } = decisionEditor("decision", withPreview);
  await decision((d) => d.load());
  const input = () => topRow(app, "密钥").children[1].children[0].children[0];
  assert.equal(input().attrs.placeholder, "AIza…KLMN · 粘贴新密钥可替换");
  assert.equal(input().attrs.title, "AIza…KLMN");
  assert.equal(input().value, "");
  control(app, "decision/provider").listeners.change({ target: { value: "openai" } });
  assert.equal(input().attrs.placeholder, "••••-key · 粘贴新密钥可替换");
  // A platform without a key says to paste one, and shows no preview or title.
  control(app, "decision/provider").listeners.change({ target: { value: "typesafe" } });
  assert.equal(input().attrs.placeholder, "粘贴密钥");
  assert.equal(input().attrs.title, undefined);
  // Saving takes the new preview from the answer; clearing takes it away.
  input().listeners.input({ target: { value: "sk-proj-0123456789a8b" } });
  const [, save] = topRow(app, "密钥").children[1].children[0].children;
  await save.listeners.click();
  assert.equal(input().attrs.placeholder, "sk-p…9a8b · 粘贴新密钥可替换");
  assert.equal(input().value, "");
  assert.ok(!JSON.stringify(roleChildren(app)).includes("0123456789a8b"));
  await topRow(app, "密钥").children[1].children[0].children[2].listeners.click();
  assert.equal(input().attrs.placeholder, "粘贴密钥");
  assert.equal(calls.filter((call) => call.url === "/app/decision-key").length, 2);
  // A tray that does not send previews keeps the old wording.
  control(app, "decision/provider").listeners.change({ target: { value: "gemini" } });
  const old = decisionEditor("decision", PLATFORM_FILE(null, { gemini: true }));
  await old.decision((d) => d.load());
  assert.equal(topRow(old.app, "密钥").children[1].children[0].children[0].attrs.placeholder, "已保存，粘贴新密钥可替换");
});
