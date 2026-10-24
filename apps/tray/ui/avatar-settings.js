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
  // Models come from what each CLI lists and what Settings already configures, never typed in.
  const provider = h("select", { "aria-label": "执行者 CLI" });
  const model = h("select", { "aria-label": "模型" });
  const submit = h("button", { type: "submit" }, "设置此模型头像");
  const form = h("form.avatar-add", {}, provider, model, submit);
  const choices = new Map();
  function fillModels() {
    model.replaceChildren(...(choices.get(provider.value) || []).map((id) => h("option", { value: id }, id)));
  }
  provider.addEventListener("change", fillModels);
  form.addEventListener("submit", (event) => { event.preventDefault(); if (!busy && model.value) add(provider.value, model.value); });
  window.addEventListener("agentnave:avatars", () => { submit.disabled = busy || !model.value; });
  fetch("/settings/members", { cache: "no-store" }).then((response) => response.ok ? response.json() : null).then((data) => {
    for (const [id, info] of Object.entries(data?.detected || {})) {
      const ids = [...(info.models || []).map((m) => m.id), ...Object.keys(data.document?.clis?.[id]?.models || {})];
      if (ids.length) choices.set(id, [...new Set(ids)]);
    }
    for (const id of choices.keys()) provider.append(h("option", { value: id }, id));
    fillModels();
    form.hidden = choices.size === 0;
    window.dispatchEvent(new Event("agentnave:avatars"));
  }).catch(() => { form.hidden = true; });
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
