const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const context = vm.createContext({ window: {} });
vm.runInContext(readFileSync(`${__dirname}/../onboarding-steps.js`, "utf8"), context);
const { evaluate, isNewUser, isFinished } = context.window.ANOnboarding;

const host = (id, status, tested = true) => ({ id, label: id, status, tested });
const everything = (over = {}) => ({
  status: { service: null },
  hosts: { service: { installed: true }, hosts: [host("claude", "connected")] },
  members: { document: { modes: { task: { claude: { models: [] } }, discussion: { claude: { models: [] } } } } },
  orchestration: { document: { version: 1, preset: "split" } },
  ...over,
});
const states = (steps) => Array.from(steps, (step) => step.state);

test("a new user starts with every step open and only the service available", () => {
  const steps = evaluate({
    status: { service: null },
    hosts: { service: { installed: false }, hosts: [host("codex", "available")] },
    members: { document: { version: 1, clis: {}, modes: {} } },
    orchestration: { document: { version: 1, preset: "off" } },
  });
  assert.deepEqual(states(steps), ["todo", "todo", "todo", "todo", "todo"]);
  assert.equal(steps[1].locked, true);
  assert.equal(steps[4].locked, true);
  assert.equal(isNewUser(steps), true);
});

test("the service step follows the install state, and a failure keeps its reason", () => {
  const fresh = (service) => ({ ...everything({ status: { service } }), hosts: { service: { installed: false }, hosts: [] } });
  assert.equal(evaluate(fresh({ state: "installing" }))[0].state, "busy");
  assert.deepEqual({ ...evaluate(fresh({ state: "failed", error: "无网络" }))[0] }, { id: "service", state: "failed", error: "无网络" });
  assert.equal(evaluate(everything({ status: { service: { state: "outdated" } } }))[0].state, "done");
});

test("connecting targets one available app, or only the tested ones among several", () => {
  const targets = (...hosts) => evaluate(everything({ hosts: { service: { installed: true }, hosts } }))[1].targets;
  assert.deepEqual(Array.from(targets(host("a", "available", false))), ["a"]);
  assert.deepEqual(Array.from(targets(host("a", "available", false), host("b", "available", true))), ["b"]);
  assert.deepEqual(Array.from(targets(host("a", "needs_attention"), host("b", "orphaned"))), []);
});

test("an employee counts once confirmed in either scene, a model only when picked", () => {
  const members = (modes) => evaluate(everything({ members: { document: { modes } } }));
  assert.deepEqual(states(members({ discussion: { codex: { models: [] } } }).slice(2, 4)), ["done", "todo"]);
  assert.equal(members({ task: { codex: { models: [], default: "gpt-x" } } })[3].state, "done");
  assert.equal(members({ task: { codex: { models: [], default: "gpt-x" } } })[2].count, 1);
});

test("orchestration is done once a preset or any app's own mode is on", () => {
  const orchestration = (document) => evaluate(everything({ orchestration: { document } }))[4].state;
  assert.equal(orchestration({ preset: "off" }), "todo");
  assert.equal(orchestration({ preset: "off", apps: { codex: "split" } }), "done");
  assert.equal(orchestration({ preset: "meetings" }), "done");
});

test("a set-up user is not new, and the list finishes once every step but models is done", () => {
  const steps = evaluate(everything({ orchestration: { document: { preset: "off" } } }));
  assert.equal(isNewUser(steps), false);
  assert.equal(isFinished(steps), false);
  const defaults = evaluate(everything());
  assert.equal(defaults[3].state, "todo");
  assert.equal(isFinished(defaults), true);
});

test("nothing is decided while hosts or employees cannot be read", () => {
  assert.equal(evaluate(everything({ hosts: null })), null);
  assert.equal(evaluate(everything({ members: { error: "损坏", document: {} } })), null);
});
