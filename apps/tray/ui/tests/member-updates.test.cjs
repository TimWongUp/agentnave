const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function app(fetch, AN = {}) {
  const controls = new Map();
  const control = (id) => {
    if (!controls.has(id)) controls.set(id, { options: [], dataset: {}, classList: { toggle() {}, contains() { return false; } } });
    return controls.get(id);
  };
  const context = vm.createContext({ AN, fetch,
    document: { getElementById: control },
    window: { addEventListener() {} },
  });
  const run = (code) => vm.runInContext(code, context);
  for (const file of ["settings-state.js", "member-updates.js"]) {
    run(readFileSync(`${__dirname}/../${file}`, "utf8"));
  }
  run(`
    el = (tag, attrs, ...children) => ({ tag, attrs, children, appendChild(child) { this.children.push(child); } });
    textNode = (s) => s;
    function renderErrorBanner() {}
    let errors = [], summaries = [];
    function renderCards() {
      errors = PROVIDERS.map(renderCliUpdateError);
      summaries.push(cliBatch ? { ...cliBatch } : null);
    }
    const productionUpdateControls = updateControls;
    updateControls = () => {};
    detected = { codex: { installed: true }, claude: { installed: true } };
    cliUpdates = { clis: { codex: { update: true, latest: '2.0.0', command: 'codex update' },
      claude: { update: true, latest: '2.0.0', command: 'claude update' } } };
  `);
  return { run, json: (code) => JSON.parse(run(`JSON.stringify(${code})`)) };
}

const members = { detected: { codex: { installed: true }, claude: { installed: true } } };

test("update all starts every POST together, survives a failure and reads members once", async () => {
  const bodies = [];
  let active = 0, maxActive = 0, reads = 0;
  const ui = app(async (url, options) => {
    if (url === "/settings/members") { reads++; return { ok: true, json: async () => members }; }
    assert.equal(url, "/app/cli-updates/update");
    assert.equal(options.method, "POST");
    assert.equal(options.headers["X-AgentNave-Action"], "1");
    const body = JSON.parse(options.body);
    bodies.push(body);
    maxActive = Math.max(maxActive, ++active);
    await new Promise((resolve) => setImmediate(resolve));
    active--;
    return body.provider === "codex"
      ? { ok: false, json: async () => ({ error: "codex update: failed" }) }
      : { ok: true, json: async () => ({ clis: {} }) };
  });
  await ui.run("updateAllClis()");
  assert.deepEqual(bodies, [{ provider: "codex" }, { provider: "claude" }]);
  assert.equal(maxActive, 2);
  assert.equal(reads, 1);
  assert.equal(ui.run("cliSummary"), "更新完成：成功 1，失败 1");
  assert.equal(ui.run("cliBatch"), null);
  assert.equal(ui.run("cliBusy()"), false);
  assert.equal(ui.json("errors[0]").children[0], "codex update: failed");
});

test("a response cannot roll back a target another response already updated", async () => {
  const finish = {};
  const ui = app((url, options) => url === "/settings/members"
    ? Promise.resolve({ ok: true, json: async () => members })
    : new Promise(resolve => { finish[JSON.parse(options.body).provider] = resolve; }));
  const old = { update: true, latest: "2.0.0", version: "1.0.0" };
  const fresh = { update: false, latest: "2.0.0", version: "2.0.0" };
  const all = ui.run("updateAllClis()");
  finish.claude({ ok: true, json: async () => ({ clis: { codex: old, claude: fresh } }) });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(ui.run("cliUpdating.has('claude')"), false);
  assert.equal(ui.run("cliUpdating.has('codex')"), true);
  finish.codex({ ok: true, json: async () => ({ clis: { codex: fresh, claude: old } }) });
  await all;
  assert.equal(ui.run("cliUpdates.clis.claude.version"), "2.0.0");
  assert.equal(ui.run("cliUpdates.clis.codex.version"), "2.0.0");
  assert.equal(ui.json("updatableClis()").length, 0);
});

test("a members read overtaken by a later update cannot replace its detection", async () => {
  const reads = [], posts = [];
  const ui = app((url) => new Promise(resolve => (url === "/settings/members" ? reads : posts).push(resolve)));
  const reply = (body) => ({ ok: true, json: async () => body });
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const first = ui.run("updateSingleCli('codex')");
  posts[0](reply({ clis: {} }));
  await tick();
  assert.equal(reads.length, 1);
  assert.equal(ui.run("cliBusy()"), true);
  const second = ui.run("updateSingleCli('claude')");
  posts[1](reply({ clis: {} }));
  await tick();
  reads[1](reply({ detected: { claude: { version: "new" } } }));
  await second;
  assert.equal(ui.run("cliBusy()"), true);
  reads[0](reply({ detected: { claude: { version: "old" } } }));
  await first;
  assert.equal(ui.run("detected.claude.version"), "new");
  assert.equal(ui.run("cliBusy()"), false);
});

test("a running update leaves other buttons usable and ignores a repeat of itself", async () => {
  let finish, posts = 0;
  const ui = app((url) => {
    if (url === "/settings/members") return Promise.resolve({ ok: true, json: async () => members });
    posts++;
    return new Promise(resolve => { finish = resolve; });
  });
  const updating = ui.run("updateSingleCli('claude')");
  const button = ui.json("renderCliUpdate('claude').children[0]");
  assert.equal(button.attrs.disabled, true);
  assert.equal(button.children[0], "更新中…");
  assert.equal(ui.json("renderCliUpdate('codex').children[0]").attrs.disabled, false);
  await ui.run("updateSingleCli('claude')");
  assert.equal(posts, 1);
  finish({ ok: false, json: async () => ({ error: "claude update: second\nthird\nfourth" }) });
  await updating;
  const error = ui.json("errors[1]");
  assert.equal(error.attrs.className, "member-error cli-update-error");
  assert.equal(error.children[0], "claude update: second\nthird\nfourth");
  assert.equal(ui.json("errors[0]").children[0], "");
});

test("Codex lists both installations and updates only the standalone one by target", async () => {
  const bodies = [];
  const ui = app(async (url, options) => {
    if (url === "/settings/members") return { ok: true, json: async () => members };
    bodies.push(JSON.parse(options.body));
    return { ok: true, json: async () => ({ clis: {} }) };
  });
  ui.run(`cliUpdates = { clis: { codex: { update: false, version: '0.167.0-alpha.3.2', installations: [
    { id: 'desktop', label: 'ChatGPT 内置 CLI', path: '/app/codex', version: '0.167.0-alpha.3.2', active: true, update: false },
    { id: 'standalone', label: 'Codex CLI', path: '/bin/codex', version: '0.166.0', active: false, update: true, latest: '0.167.0', command: 'npm i' },
  ] } } }`);
  const [desktop, standalone] = ui.json("renderCliInstallations('codex').children[0]");
  const text = (row) => JSON.stringify(row);
  assert.match(text(desktop), /0\.167\.0-alpha\.3\.2/);
  assert.match(text(desktop), /当前使用/);
  assert.match(desktop.attrs.title, /随桌面应用更新/);
  assert.doesNotMatch(text(desktop), /cli-update"/);
  assert.match(text(standalone), /0\.166\.0/);
  assert.doesNotMatch(text(standalone), /当前使用/);
  assert.match(text(standalone), /更新到 0\.167\.0/);
  assert.match(text(standalone), /通过 npm i 更新到 0\.167\.0/);
  assert.equal(ui.json("renderCliUpdate('codex')"), null);
  assert.equal(ui.json("renderCliVersion('codex', { installed: true, version: '1' })"), null);
  assert.deepEqual(ui.json("updatableClis()"), [{ provider: "codex", target: "standalone" }]);
  await ui.run("updateSingleCli('codex', 'standalone')");
  assert.deepEqual(bodies, [{ provider: "codex", target: "standalone" }]);
});

test("a lone standalone installation keeps the single version and default update", async () => {
  const bodies = [];
  const ui = app(async (url, options) => {
    if (url === "/settings/members") return { ok: true, json: async () => members };
    bodies.push(JSON.parse(options.body));
    return { ok: true, json: async () => ({ clis: {} }) };
  });
  ui.run(`cliUpdates = { clis: { codex: { update: true, latest: '2.0.0', version: '1.0.0', command: 'npm i', installations: [
    { id: 'standalone', label: 'Codex CLI', version: '1.0.0', active: true, update: true, latest: '2.0.0', command: 'npm i' },
  ] } } }`);
  assert.equal(ui.json("renderCliInstallations('codex')"), null);
  assert.equal(ui.json("renderCliVersion('codex', { installed: true })").children[0], "1.0.0");
  assert.equal(ui.json("renderCliUpdate('codex').children[0]").children[0], "更新到 2.0.0");
  assert.deepEqual(ui.json("updatableClis()"), [{ provider: "codex" }]);
  await ui.run("updateSingleCli('codex')");
  assert.deepEqual(bodies, [{ provider: "codex" }]);
});

test("a later response cannot restore the standalone version it read before that update", async () => {
  const finish = {};
  const ui = app((url, options) => url === "/settings/members"
    ? Promise.resolve({ ok: true, json: async () => members })
    : new Promise(resolve => { finish[JSON.parse(options.body).provider] = resolve; }));
  const codex = (version, update) => ({ update: false, installations: [
    { id: "desktop", label: "ChatGPT 内置 CLI", version: "3.0.0", active: true, update: false },
    { id: "standalone", label: "Codex CLI", version, active: false, update, latest: "2.0.0", command: "npm i" },
  ] });
  ui.run(`cliUpdates = { clis: ${JSON.stringify({ codex: codex("1.0.0", true), claude: { update: true, latest: "2.0.0" } })} }`);
  const all = ui.run("updateAllClis()");
  finish.codex({ ok: true, json: async () => ({ clis: { codex: codex("2.0.0", false) } }) });
  await new Promise((resolve) => setImmediate(resolve));
  finish.claude({ ok: true, json: async () => ({ clis: { codex: codex("1.0.0", true), claude: { update: false } } }) });
  await all;
  assert.equal(ui.run("cliUpdates.clis.codex.installations[1].version"), "2.0.0");
  assert.equal(ui.json("updatableClis()").length, 0);
});

test("English labels name each installation by its id and app", () => {
  const ui = app(async () => { throw new Error("unexpected request"); }, { L: (_zh, en) => en });
  const row = (install) => JSON.stringify(ui.json(`renderCliInstallation('codex', ${JSON.stringify(install)})`));
  const active = row({ id: "desktop", label: "ChatGPT 内置 CLI", active: true });
  assert.match(active, /ChatGPT built-in CLI/);
  assert.match(active, /In use/);
  assert.match(active, /Updates with the desktop app/);
  assert.match(row({ id: "desktop", label: "Codex 内置 CLI" }), /Codex built-in CLI/);
  assert.match(row({ id: "standalone", label: "Codex CLI" }), /Standalone CLI/);
  assert.doesNotMatch(active, /内置/);
});

test("without installations a card keeps its single version and has no source rows", () => {
  const ui = app(async () => { throw new Error("unexpected request"); });
  assert.equal(ui.json("renderCliInstallations('codex')"), null);
  assert.equal(ui.json("renderCliVersion('codex', { installed: true, version: '1.0.0' })").children[0], "1.0.0");
});

test("failed latest query clears stale update availability", async () => {
  const ui = app(async () => { throw new Error("offline"); });
  await ui.run("loadCliUpdates()");
  assert.deepEqual(ui.json("cliUpdates.clis"), {});
});

test("a latest query started during an update cannot restore the old version afterwards", async () => {
  let finishPost, finishGet;
  const ui = app((url) => {
    if (url === "/settings/members") return Promise.resolve({ ok: true, json: async () => members });
    return new Promise(resolve => {
      if (url === "/app/cli-updates") finishGet = resolve;
      else finishPost = resolve;
    });
  });
  const update = ui.run("updateSingleCli('codex')");
  const query = ui.run("loadCliUpdates()");
  finishPost({ ok: true, json: async () => ({ clis: { codex: { update: false, version: "2.0.0" } } }) });
  await update;
  finishGet({ ok: true, json: async () => ({ clis: { codex: { update: true, version: "1.0.0" } } }) });
  await query;
  assert.equal(ui.run("cliUpdates.clis.codex.version"), "2.0.0");
  assert.equal(ui.run("cliUpdates.clis.codex.update"), false);
});

test("static installation commands remain available while latest queries stall or fail", async () => {
  let rejectLatest;
  const ui = app((url) => {
    if (url === "/app/cli-install-commands") return Promise.resolve({ ok: true, json: async () => ({
      codex: [{ label: "npm", command: "npm install -g @openai/codex" }], grok: [],
    }) });
    return new Promise((_resolve, reject) => { rejectLatest = reject; });
  });
  await ui.run("loadCliInstallCommands()");
  ui.run("cliSummary = 'previous batch'; missingOpen = true");
  const latest = ui.run("loadCliUpdates()");
  assert.equal(ui.run("cliSummary"), "");
  const rendered = ui.json("renderMissingClis(['codex', 'grok'])");
  assert.match(JSON.stringify(rendered), /npm install -g @openai\/codex/);
  assert.match(JSON.stringify(rendered), /Grok CLI/);
  rejectLatest(new Error("offline"));
  await latest;
  assert.equal(ui.run("cliInstallCommands.codex[0].command"), "npm install -g @openai/codex");
});

test("updating disables refresh controls and skips both manual and focus detection", async () => {
  let requests = 0;
  const ui = app(async () => { requests++; throw new Error("unexpected detection"); });
  ui.run(readFileSync(`${__dirname}/../member-cards.js`, "utf8"));
  // Exercise the shared controls with their other sections stubbed out.
  ui.run(`
    function advancedChanges() { return []; }
    function renderLeaveConfirm() {}
    function renderAdvancedBar() {}
    emptyScene = () => null;
    function memberSaveErrors() { return []; }
    countPermissionOptions = () => 0;
    document.visibilityState = 'visible';
    cliUpdating.add('codex');
  `);
  ui.run("productionUpdateControls()");
  assert.equal(ui.run("$('refresh-detection').disabled"), true);
  await ui.run("refreshDetection()");
  ui.run("probeWhenStale()");
  assert.equal(requests, 0);
  ui.run("cliUpdating.clear(); productionUpdateControls()");
  assert.equal(ui.run("$('refresh-detection').disabled"), false);
});

test("a check with nothing new leaves the cards alone", async () => {
  const ui = app(async () => ({ ok: true, json: async () => ui.json("cliUpdates") }));
  ui.run("summaries = []");
  await ui.run("loadCliUpdates()");
  assert.equal(ui.run("summaries.length"), 0);
});

test("a save during the read after an update is not overwritten", async () => {
  const ui = app(async (url) => {
    if (String(url).includes("cli-updates")) return { ok: true, json: async () => ({ clis: {} }) };
    ui.run("savedSnapshot = '{\"saved\":true}'; currentDoc = { saved: true }");
    return { ok: true, json: async () => ({ ...members, document: { stale: true } }) };
  });
  ui.run("savedSnapshot = '{}'; currentDoc = {}");
  await ui.run('performCliUpdate("codex")');
  assert.deepEqual(ui.json("currentDoc"), { saved: true });
});
