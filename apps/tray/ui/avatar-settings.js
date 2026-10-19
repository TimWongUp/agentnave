"use strict";
(function () {
  const { h } = AN;
  const root = document.getElementById("avatar-settings");
  const status = h("p", { role: "status" });
  let busy = false;
  const customRows = new Map();
  function editor(kind, provider, model, label) {
    const row = h("div.avatar-editor");
    const preview = Avatars.create(kind, provider, model);
    const input = h("input", { type: "file", accept: "image/png,image/jpeg,image/webp", hidden: true, "aria-label": `上传${label}头像` });
    const upload = h("button", { type: "button" }, "上传图片");
    const reset = h("button", { type: "button" }, "恢复默认");
    function update() { reset.disabled = busy || !Avatars.custom(kind, provider, model); upload.disabled = busy; }
    async function run(action) {
      busy = true; root.querySelectorAll("button").forEach((b) => { b.disabled = true; });
      try { await action(); status.textContent = "头像已保存，所有视图将自动更新"; }
      catch (error) { status.textContent = error.message; }
      finally { busy = false; input.value = ""; window.dispatchEvent(new Event("agentnave:avatars")); }
    }
    upload.addEventListener("click", () => input.click());
    input.addEventListener("change", () => { if (input.files[0]) run(() => Avatars.upload(kind, provider, model, input.files[0])); });
    reset.addEventListener("click", () => run(() => Avatars.save(kind, provider, model, null)));
    window.addEventListener("agentnave:avatars", update);
    update();
    row.append(preview, h("span.avatar-label", {}, label), input, upload, reset);
    return row;
  }
  root.append(status, h("h2", {}, "模型家族"));
  const grid = h("div.avatar-grid");
  for (const [id, [label]] of Object.entries(Avatars.families)) grid.append(editor("family", id, "", label));
  root.append(grid, h("h2", {}, "指定模型"), h("p.guidance", {}, "同一模型可在不同 CLI 下使用不同头像，优先于模型家族设置。"));
  const models = h("div.avatar-grid");
  function add(provider, model) {
    const key = Avatars.key("model", provider, model);
    if (customRows.has(key)) return;
    const row = editor("model", provider, model, `${provider} / ${model}`);
    customRows.set(key, row); models.append(row);
  }
  const provider = h("select", { "aria-label": "执行者 CLI" });
  for (const id of ["antigravity", "claude", "codebuddy", "codex", "deepseek", "gemini", "grok", "hermes", "opencode", "pi", "workbuddy"]) provider.append(h("option", { value: id }, id));
  const model = h("input", { type: "text", placeholder: "模型 ID，例如 gpt-6", "aria-label": "模型 ID", required: true, maxlength: 500 });
  const submit = h("button", { type: "submit" }, "设置此模型头像");
  const form = h("form.avatar-add", {}, provider, model, submit);
  form.addEventListener("submit", (event) => { event.preventDefault(); if (!busy && model.value.trim()) add(provider.value, model.value.trim()); });
  window.addEventListener("agentnave:avatars", () => { submit.disabled = busy; });
  root.append(form, models);
  Avatars.load().then(async () => {
    const response = await fetch("/avatar-settings", {cache:"no-store"});
    if (!response.ok) throw new Error("无法读取头像设置");
    for (const key of Object.keys(await response.json())) {
      const [kind, provider, model] = JSON.parse(key);
      if (kind === "model") add(provider, model);
    }
    window.dispatchEvent(new Event("agentnave:avatars"));
  }).catch((error) => { status.textContent = error.message; root.querySelectorAll("button").forEach((b) => { b.disabled = true; }); });
})();
