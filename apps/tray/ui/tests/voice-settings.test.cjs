const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const vm = require("node:vm");

// The voice settings page on a DOM just deep enough to read its rows and press its buttons.
function page(config, models = [], english = false) {
  const posts = [];
  const node = (tag, attrs = {}) => ({
    tag, attrs: { ...attrs }, children: [], listeners: {}, style: {}, dataset: {}, className: "", open: false, value: "", disabled: false,
    setAttribute(key, value) { this.attrs[key] = value; if (key === "value") this.value = value; if (key === "disabled") this.disabled = true; },
    append(...kids) { this.children.push(...kids); }, replaceChildren(...kids) { this.children = kids; },
    addEventListener(type, fn) { this.listeners[type] = fn; }, querySelector: () => null, querySelectorAll: () => [], contains: () => false,
  });
  const h = (tag, props, ...kids) => {
    const [name, ...classes] = tag.split(".");
    const element = node(name || "div");
    element.className = classes.join(" ");
    for (const [key, value] of Object.entries(props || {})) if (value != null && value !== false) element.setAttribute(key, value === true ? "" : value);
    element.append(...kids.flat(Infinity).filter((kid) => kid != null && kid !== false));
    return element;
  };
  const root = node("div"), notice = node("div");
  const state = { config, models };
  const body = (data) => ({ ok: true, status: 200, json: async () => structuredClone(data) });
  const context = vm.createContext({
    AN: { h, L: (zh, en) => (english ? en : zh), art: { url: (name) => name, head: () => node("span"), character: () => "" }, settings: { settingTip: () => null } },
    Avatars: { families: { gpt: ["GPT"] } }, structuredClone, setTimeout: () => 0, clearTimeout() {}, Promise, JSON, AbortController, Audio: class {}, URL,
    document: { getElementById: (id) => (id === "voice-settings" ? root : id === "notice" ? notice : node("div")), addEventListener() {}, hidden: false, activeElement: null },
    location: { hash: "#voice" }, addEventListener() {}, window: {},
    fetch: async (path, options = {}) => {
      if (options.method === "POST") posts.push({ path, body: JSON.parse(options.body) });
      if (path === "/app/voice" && options.method === "POST" && state.failSave) return { ok: false, status: 500, json: async () => ({ error: "保存失败" }) };
      if (path === "/app/voice" && options.method === "POST") {
        const sent = JSON.parse(options.body);
        state.config = { ...state.config, ...sent, summary: { ...state.config.summary, ...sent.summary } };
        return body(state.config);
      }
      if (path === "/app/voice") return body(state.config);
      if (path === "/app/voice/summary/models") return body(state.models);
      if (path === "/app/voice/cache") return body({ bytes: 0 });
      return body([]);
    },
  });
  vm.runInContext(readFileSync(`${__dirname}/../voice-settings.js`, "utf8"), context);
  const walk = (item, found = []) => {
    if (item && typeof item === "object") { found.push(item); item.children.forEach((child) => walk(child, found)); }
    return found;
  };
  const text = (item) => (typeof item === "string" ? item : (item.children || []).map(text).join(""));
  const rows = () => walk(root).filter((item) => item.className === "s-row");
  const rowOf = (label) => rows().find((row) => text(row.children[0]) === label);
  const labelled = (label) => walk(root).find((item) => item.attrs["aria-label"] === label);
  const open = async () => { await context.window.AgentNaveVoiceSettings.load(); await new Promise(setImmediate); };
  return { open, rows, rowOf, labelled, walk, text, posts, state, root };
}

const provider = (model, extra = {}) => ({
  enabled: true, listen_minutes: 60, continue_read: false, read_scope: "all", code: "skip", cache_limit_mb: 200, engine: "elevenlabs", minimax_region: "cn",
  model: "", models: {}, voices: {}, personas: {}, chatgpt: { signed_in: false },
  keys: { elevenlabs: true, "summary:openai": true }, key_previews: {},
  summary_platforms: [{ id: "chatgpt", name: "ChatGPT" }, { id: "magpie", name: "Magpie" }, { id: "custom", name: "自定义" }, { id: "openai", name: "OpenAI" }],
  summary: { enabled: true, level: "standard", language: "source", provider: "openai", model, effort: "", base_url: "", prompt: "" },
  ...extra,
});
const LISTED = [{ id: "gpt-a", owned_by: "openai" }, { id: "gpt-b", owned_by: "openai" }];
const saves = (app) => app.posts.filter((post) => post.path === "/app/voice");
const MANUAL = "手动输入模型 ID…";
const optionsOf = (page, label) => page.walk(page.labelled(label).parent || page.root).filter(() => false);
const pickerOptions = (app, label) => app.walk(app.root).find((item) => item.attrs.role === "listbox" && item.attrs["aria-label"] === label).children;

test("the summary model is a list, and its id is typed only on demand", async () => {
  const app = page(provider("gpt-a"), LISTED);
  await app.open();
  const list = () => app.rowOf("摘要模型");
  assert.ok(list());
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
  assert.deepEqual(pickerOptions(app, "摘要模型").map((item) => app.text(item)), ["openai/gpt-a", "openai/gpt-b", MANUAL]);
  // Asking for it opens the typed row with its Use button, and changes nothing yet.
  pickerOptions(app, "摘要模型").at(-1).listeners.click();
  const typed = () => app.labelled("手填摘要模型 ID");
  assert.ok(typed());
  assert.equal(app.text(app.rowOf("摘要模型 ID")).endsWith("使用"), true);
  assert.equal(saves(app).length, 0);
  assert.equal(app.text(app.labelled("摘要模型")).includes(MANUAL), true);
  typed().value = " vendor/custom-1 ";
  typed().listeners.input();
  app.rowOf("摘要模型 ID").children[1].children[1].listeners.click();
  await new Promise(setImmediate);
  assert.deepEqual(saves(app).at(-1).body.summary.model, "vendor/custom-1");
  assert.equal(saves(app).at(-1).body.summary.effort, "");
  // The model is in no list now, so the row stays; a listed model closes it.
  assert.ok(typed());
  assert.equal(typed().value, "vendor/custom-1");
  pickerOptions(app, "摘要模型")[1].listeners.click();
  await new Promise(setImmediate);
  assert.equal(saves(app).at(-1).body.summary.model, "gpt-b");
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
});

test("a typed summary model that fails to save stays in its box for another try", async () => {
  const app = page(provider("gpt-a"), LISTED);
  await app.open();
  pickerOptions(app, "摘要模型").at(-1).listeners.click();
  const typed = () => app.labelled("手填摘要模型 ID");
  typed().value = "gpt-b";
  typed().listeners.input();
  app.state.failSave = true;
  app.rowOf("摘要模型 ID").children[1].children[1].listeners.click();
  await new Promise(setImmediate);
  assert.ok(typed(), "the box is still there");
  assert.equal(app.text(app.labelled("摘要模型")).includes(MANUAL), true);
  assert.equal(typed().value, "gpt-b");
  // Trying again after the cause is gone saves it and puts the list back.
  app.state.failSave = false;
  app.rowOf("摘要模型 ID").children[1].children[1].listeners.click();
  await new Promise(setImmediate);
  assert.equal(saves(app).at(-1).body.summary.model, "gpt-b");
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
});

test("a model that is in no list opens the typed row, and the platform choice closes it", async () => {
  const app = page(provider("vendor/odd"), LISTED);
  await app.open();
  assert.equal(app.labelled("手填摘要模型 ID").value, "vendor/odd");
  assert.equal(app.text(app.labelled("摘要模型")).includes(MANUAL), true);
  pickerOptions(app, "摘要平台").find((item) => app.text(item) === "OpenAI").listeners.click();
  await new Promise(setImmediate);
  assert.equal(saves(app).at(-1).body.summary.provider, "openai");
});

test("without a list the typed id, Use and Refresh share one row", async () => {
  const app = page(provider("gpt-a"), []);
  await app.open();
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
  assert.equal(app.labelled("摘要模型"), undefined, "no empty list to open");
  const row = app.rowOf("摘要模型");
  const field = row.children[1];
  assert.deepEqual(field.children.map((item) => item.tag === "button" ? app.text(item) : item.attrs["aria-label"]), ["手填摘要模型 ID", "使用", "刷新"]);
  assert.equal(field.children[0].value, "gpt-a");
  // A list that arrives on refresh puts the select back in place of the typed row.
  app.state.models = LISTED;
  field.children[2].listeners.click();
  await new Promise(setImmediate);
  assert.ok(app.labelled("摘要模型"));
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
});

test("magpie only chooses from its catalog", async () => {
  const app = page(provider("gone-model", { summary: { ...provider("").summary, provider: "magpie" } }), LISTED);
  await app.open();
  assert.deepEqual(pickerOptions(app, "摘要模型").map((item) => app.text(item)), ["openai/gpt-a", "openai/gpt-b"]);
  assert.equal(app.rowOf("摘要模型 ID"), undefined);
  assert.equal(app.labelled("手填摘要模型 ID"), undefined);
  const empty = page(provider("", { summary: { ...provider("").summary, provider: "magpie" } }), []);
  await empty.open();
  assert.equal(empty.labelled("手填摘要模型 ID"), undefined);
});

test("saved voice and summary keys show a masked preview that is never a value", async () => {
  const app = page(provider("gpt-a", { key_previews: { elevenlabs: "sk_3…a8b2", "summary:openai": "••••-key" } }), LISTED);
  await app.open();
  const eleven = app.labelled("elevenlabs API 密钥"), summary = app.labelled("summary:openai API 密钥");
  assert.equal(eleven.attrs.placeholder, "sk_3…a8b2 · 粘贴新密钥可替换");
  assert.equal(eleven.attrs.title, "sk_3…a8b2");
  assert.equal(summary.attrs.placeholder, "••••-key · 粘贴新密钥可替换");
  assert.equal(eleven.value, "");
  assert.equal(summary.value, "");
  assert.ok(!app.walk(app.root).some((item) => item.value === "sk_3…a8b2" || item.value === "••••-key"));
  // Without previews, or with no key, the older wording stands.
  const old = page(provider("gpt-a", { key_previews: undefined, keys: { elevenlabs: true, "summary:openai": false } }), LISTED);
  await old.open();
  assert.equal(old.labelled("elevenlabs API 密钥").attrs.placeholder, "已保存，粘贴新密钥可替换");
  assert.equal(old.labelled("elevenlabs API 密钥").attrs.title, undefined);
  assert.equal(old.labelled("summary:openai API 密钥").attrs.placeholder, "粘贴密钥");
  const english = page(provider("gpt-a", { key_previews: { elevenlabs: "sk_3…a8b2" } }), LISTED, true);
  await english.open();
  assert.equal(english.labelled("elevenlabs API key").attrs.placeholder, "sk_3…a8b2 · Paste a new key to replace");
  assert.equal(pickerOptions(english, "Summary model").at(-1).children.at(-1), "Enter model ID manually…");
});

test("failed summary platform switch preserves an open manual model input and typed draft", async () => {
  const app = page(provider("gpt-a"), LISTED);
  await app.open();
  pickerOptions(app, "摘要模型").at(-1).listeners.click();
  const input = app.labelled("手填摘要模型 ID");
  input.value = "my-draft-model";
  input.listeners.input();
  app.state.failSave = true;
  pickerOptions(app, "摘要平台").find((item) => app.text(item) === "自定义").listeners.click();
  await new Promise(setImmediate);
  assert.equal(app.labelled("手填摘要模型 ID").value, "my-draft-model");
  // Re-clicking the already-selected manual option keeps the typed draft too.
  pickerOptions(app, "摘要模型").at(-1).listeners.click();
  assert.equal(app.labelled("手填摘要模型 ID").value, "my-draft-model");
});
