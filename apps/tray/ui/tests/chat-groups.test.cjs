const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

const context = vm.createContext({ window: {}, Date });
vm.runInContext(readFileSync(`${__dirname}/../chat-groups.js`, "utf8"), context);
const { other, caps, capSections, capFlat } = context.window.ChatGroups;
// Results come from another realm; copy them so deepEqual compares plain values.
const plain = (value) => JSON.parse(JSON.stringify(value));
const arrange = (...args) => plain(context.window.ChatGroups.arrange(...args));
const cap = (...args) => plain(capSections(...args));
const flat = (...args) => plain(capFlat(...args));
const env = { urgent: status => status === "running" || status === "review", L: zh => zh };
const must = g => env.urgent(g.status);

const ws = (group, project, updated, status = "idle", extra = {}) => ({
  group, status, updated, label: "Codex", boss_key: "codex|Codex",
  project_key: project, project_label: project === other ? null : project.split("/").pop(),
  project_path: project === other ? null : project, ...extra,
});

test("projects follow their latest activity, work going on leads inside a group, other comes last", () => {
  const sections = arrange([
    ws("a", other, "2026-11-12T12:00:00Z"),
    ws("b", "/w/old", "2026-11-10T00:00:00Z", "running"),
    ws("c", "/w/new", "2026-11-12T00:00:00Z"),
    ws("d", "/w/old", "2026-11-11T00:00:00Z"),
    ws("e", "/w/old", "2026-11-09T00:00:00Z", "review"),
  ], "project", env);
  assert.deepEqual(sections.map(s => s.key), ["/w/new", "/w/old", other]);
  assert.deepEqual(sections[1].items.map(g => g.group), ["b", "e", "d"]);
  assert.equal(sections[2].title, "其他");
});

test("projects that share a name show where they are", () => {
  const sections = arrange([
    ws("a", "/w/app", "2026-11-12T00:00:00Z"),
    ws("b", "/x/app", "2026-11-11T00:00:00Z"),
    ws("c", "/w/lib", "2026-11-10T00:00:00Z"),
  ], "project", env);
  assert.deepEqual(sections.map(s => s.hint), ["/w/app", "/x/app", ""]);
});

test("boss groups use the boss key and the latest label", () => {
  const sections = arrange([
    ws("a", "/w/a", "2026-11-12T00:00:00Z", "idle", { boss_key: "claude|Claude Code", label: "Claude Code" }),
    ws("b", "/w/b", "2026-11-13T00:00:00Z"),
    ws("c", "/w/c", "2026-11-11T00:00:00Z", "idle", { boss_key: "claude|Claude Code", label: "Claude Code" }),
  ], "boss", env);
  assert.deepEqual(sections.map(s => [s.title, s.items.length]), [["Codex", 1], ["Claude Code", 2]]);
});

test("a long list keeps its first groups and first entries, and counts each group whole", () => {
  const groups = [];
  for (let p = 0; p < 12; p++)
    for (let i = 0; i < 7; i++)
      groups.push(ws(`g${p}-${i}`, `/w/p${p}`, `2026-11-${String(20 - i).padStart(2, "0")}T00:00:00Z`));
  const sections = arrange(groups, "project", env);
  const capped = cap(sections, { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.equal(capped.sections.length, caps.groups);
  assert.equal(capped.hiddenGroups, 2);
  assert.equal(capped.sections[0].items.length, 7); // the header keeps counting the whole group
  assert.deepEqual(capped.sections[0].shown.map(g => g.group), ["g0-0", "g0-1", "g0-2", "g0-3", "g0-4"]);
  assert.equal(capped.sections[0].hidden, 2);
  // One more ask opens five more entries, and a group that fits hides nothing.
  const wider = cap(sections, { groupLimit: caps.groups, itemLimit: caps.items + caps.itemStep, must });
  assert.equal(wider.sections[0].shown.length, 7);
  assert.equal(wider.sections[0].hidden, 0);
});

test("a group past the limit still shows what the cap may not hide", () => {
  const deep = (statuses) => statuses.map((status, i) => ws(`deep${i}`, "/w/deep", `2026-11-0${8 - i}T00:00:00Z`, status));
  // Exactly the cap in front of it, so the deep group is the only one past the limit.
  const ahead = Array.from({ length: caps.groups }, (_, p) => ws(`a${p}`, `/w/p${p}`, `2026-11-${String(20 - p).padStart(2, "0")}T00:00:00Z`));
  const idle = ["idle", "idle", "idle", "idle", "idle", "idle", "idle", "idle"];

  // Work going on in a group past the limit keeps the group, at its own place in the order.
  const running = cap(arrange([...ahead, ...deep([...idle.slice(1), "running"])], "project", env),
    { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.deepEqual(running.sections.at(-1).key, "/w/deep");
  assert.deepEqual(running.sections.at(-1).shown.map(g => g.group), ["deep7", "deep0", "deep1", "deep2", "deep3"]);
  assert.equal(running.hiddenGroups, 0);

  // The workspace already open comes along even when it sorts past the group's first entries.
  const opened = cap(arrange([...ahead, ...deep(idle)], "project", env),
    { groupLimit: caps.groups, itemLimit: caps.items, must: g => g.group === "deep7" });
  assert.deepEqual(opened.sections.at(-1).shown.map(g => g.group), ["deep0", "deep1", "deep2", "deep3", "deep4", "deep7"]);
  assert.equal(opened.sections.at(-1).hidden, 2);

  // Nothing in it to keep: the group waits behind the "more" button.
  const hidden = cap(arrange([...ahead, ...deep(idle)], "project", env), { groupLimit: caps.groups, itemLimit: caps.items, must });
  assert.equal(hidden.sections.length, caps.groups);
  assert.equal(hidden.hiddenGroups, 1);
});

test("the ungrouped list keeps its first entries plus the ones it must show", () => {
  const items = Array.from({ length: 40 }, (_, i) => ws(`g${i}`, "/w/p", "2026-11-12T00:00:00Z", i === 35 ? "review" : "idle"));
  const capped = flat(items, { limit: caps.flat, must });
  assert.equal(capped.items.length, caps.flat + 1);
  assert.equal(capped.items.at(-1).group, "g35");
  assert.equal(capped.hidden, items.length - capped.items.length);
  const wider = flat(items, { limit: caps.flat + caps.flatStep, must });
  assert.equal(wider.items.length, items.length);
  assert.equal(wider.hidden, 0);
});
