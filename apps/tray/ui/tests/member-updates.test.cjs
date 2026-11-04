const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

function app(fetch) {
  const context = vm.createContext({ AN: {}, fetch,
    document: { getElementById: () => ({ options: [] }) },
    window: { addEventListener() {} },
  });
  const run = (code) => vm.runInContext(code, context);
  for (const file of ["settings-state.js", "member-updates.js"]) {
    run(readFileSync(`${__dirname}/../${file}`, "utf8"));
  }
  run(`
    el = (tag, attrs, ...children) => ({ tag, attrs, children });
    function renderErrorBanner() {}
    let errors = [], summaries = [];
    function renderCards() {
      errors = PROVIDERS.map(renderCliUpdateError);
      summaries.push(cliBatch ? { ...cliBatch } : null);
    }
    updateControls = () => {};
    detected = { codex: { installed: true }, claude: { installed: true } };
    cliUpdates = { clis: { codex: { update: true, latest: '2.0.0', command: 'codex update' },
      claude: { update: true, latest: '2.0.0', command: 'claude update' } } };
  `);
  return { run, json: (code) => JSON.parse(run(`JSON.stringify(${code})`)) };
}

const members = { detected: { codex: { installed: true }, claude: { installed: true } } };

test("update all runs sequential POSTs, continues after a failure and summarizes", async () => {
  const calls = [];
  let active = 0, maxActive = 0;
  const ui = app(async (url, options) => {
    if (url === "/settings/members") return { ok: true, json: async () => members };
    assert.equal(url, "/app/cli-updates/update");
    assert.equal(options.method, "POST");
    assert.equal(options.headers["X-AgentNave-Action"], "1");
    const provider = JSON.parse(options.body).provider;
    calls.push(provider);
    maxActive = Math.max(maxActive, ++active);
    await new Promise((resolve) => setImmediate(resolve));
    active--;
    return provider === "codex"
      ? { ok: false, json: async () => ({ error: "codex update: failed" }) }
      : { ok: true, json: async () => ({ clis: {} }) };
  });
  await ui.run("updateAllClis()");
  assert.deepEqual(calls, ["codex", "claude"]);
  assert.equal(maxActive, 1);
  assert.equal(ui.run("cliSummary"), "更新完成：成功 1，失败 1");
  assert.equal(ui.run("cliBatch"), null);
  assert.equal(ui.run("cliUpdating"), null);
  assert.deepEqual(ui.json("summaries.filter(Boolean).map(s => s.index).filter((s, i, a) => i === 0 || s !== a[i - 1])"), [1, 2]);
  assert.equal(ui.json("errors[0]").children[0], "codex update: failed");
});

test("single update disables buttons and displays failure inside that card", async () => {
  let finish;
  const ui = app((url) => url === "/settings/members"
    ? Promise.resolve({ ok: true, json: async () => members })
    : new Promise(resolve => { finish = resolve; }));
  const updating = ui.run("updateSingleCli('claude')");
  const button = ui.json("renderCliUpdate('claude').children[0]");
  assert.equal(button.attrs.disabled, true);
  assert.equal(button.children[0], "更新中…");
  assert.equal(ui.json("renderCliUpdate('codex').children[0]").attrs.disabled, true);
  finish({ ok: false, json: async () => ({ error: "claude update: second\nthird\nfourth" }) });
  await updating;
  const error = ui.json("errors[1]");
  assert.equal(error.attrs.className, "member-error cli-update-error");
  assert.equal(error.children[0], "claude update: second\nthird\nfourth");
  assert.equal(ui.json("errors[0]").children[0], "");
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
