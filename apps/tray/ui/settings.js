"use strict";
// Store user text as text nodes. Profiles are data, never executable page content.
const $ = (id) => document.getElementById(id);
let profiles = [];
let editing = null;
let busy = false;
let loaded = false;
let dirty = false;
let deleting = false;
const cli = { codex: "Codex CLI", claude: "Claude Code", codebuddy: "CodeBuddy Code", grok: "Grok CLI", antigravity: "Antigravity CLI", pi: "Pi Coding Agent" };
function node(tag, text, className) {
  const element = document.createElement(tag);
  if (text != null) element.textContent = text;
  if (className) element.className = className;
  return element;
}
function notice(text, error = false) {
  $("notice").textContent = text;
  $("notice").classList.toggle("error", error);
}
function controls() {
  const locked = busy || !loaded || deleting;
  $("refresh").disabled = busy || dirty || deleting;
  $("add").disabled = locked || dirty;
  $("fields").disabled = locked;
  $("save").disabled = locked;
  $("cancel").disabled = locked;
  $("delete").disabled = locked;
  $("confirm").disabled = busy;
  $("keep").disabled = busy;
  for (const button of $("list").querySelectorAll("button")) button.disabled = locked || dirty;
  $("dirty").textContent = dirty ? "有未保存的修改" : "";
}
function changed() { dirty = true; controls(); }
function render() {
  $("total").textContent = String(profiles.length).padStart(2, "0");
  $("empty").hidden = profiles.length !== 0;
  $("list").replaceChildren(...profiles.map((profile) => {
    const button = node("button", null, `profile${profile.id === editing ? " active" : ""}`);
    button.type = "button";
    button.append(node("strong", profile.name), node("small", cli[profile.provider]), node("p", profile.notes || "尚未填写适用场景"));
    button.addEventListener("click", () => edit(profile));
    return button;
  }));
  controls();
}
function option(key = "", value = "") {
  const row = node("div", null, "option-row");
  const name = node("input");
  name.type = "text"; name.value = key; name.placeholder = "参数键";
  name.setAttribute("aria-label", "参数键"); name.dataset.key = "";
  const type = node("select");
  type.setAttribute("aria-label", "值类型");
  for (const [id, label] of [["string", "字符串"], ["number", "数字"], ["boolean", "布尔"]]) {
    const choice = node("option", label); choice.value = id; type.append(choice);
  }
  type.value = typeof value;
  const input = node("input");
  input.type = "text"; input.value = String(value); input.dataset.value = "";
  input.setAttribute("aria-label", "参数值"); input.placeholder = "参数值";
  const remove = node("button", "移除"); remove.type = "button";
  remove.setAttribute("aria-label", "移除此参数");
  remove.addEventListener("click", () => { row.remove(); changed(); });
  row.append(name, type, input, remove);
  $("options").append(row);
}
function edit(profile = null) {
  if (busy || dirty || !loaded) return;
  editing = profile?.id || null;
  $("editor-title").textContent = profile ? "编辑档案" : "新增档案";
  $("name").value = profile?.name || "";
  $("provider").value = profile?.provider || "codex";
  $("model").value = profile?.provider_options.model ?? "";
  $("effort").value = profile?.provider_options.effort ?? "";
  $("notes").value = profile?.notes || "";
  $("options").replaceChildren();
  for (const [key, value] of Object.entries(profile?.provider_options || {})) {
    if (!["model", "effort"].includes(key)) option(key, value);
  }
  $("editor").hidden = false;
  $("placeholder").hidden = true;
  $("delete").hidden = !profile;
  render();
  $("name").focus();
}
function closeEditor() {
  editing = null; dirty = false; deleting = false;
  $("editor").hidden = true;
  $("placeholder").hidden = false;
  $("confirmation").hidden = true;
  render();
}
async function refresh() {
  if (busy || dirty || deleting) return;
  busy = true; controls();
  try {
    const response = await fetch("/settings/state", { cache: "no-store" });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `读取失败（${response.status}）`);
    profiles = data.profiles; loaded = true;
    closeEditor();
    notice(`共 ${profiles.length} 条档案 · 保存后供主控读取`);
  } catch (error) {
    loaded = false;
    notice(`${error.message}。请检查 profiles.json 后重新读取；当前禁止保存。`, true);
  } finally { busy = false; render(); }
}
function options() {
  const entries = [];
  for (const key of ["model", "effort"]) if ($(key).value !== "") entries.push([key, $(key).value]);
  for (const row of $("options").children) {
    const key = row.querySelector("[data-key]").value.trim();
    const type = row.querySelector("select").value;
    const raw = row.querySelector("[data-value]").value;
    if (!key) throw new Error("请填写参数键，或移除空行");
    if (["model", "effort"].includes(key) || entries.some(([existing]) => existing === key)) throw new Error(`参数键重复：${key}`);
    let value = raw;
    if (type === "number") {
      // JSON numbers exclude hex, Infinity and empty strings.
      try { value = JSON.parse(raw); } catch { throw new Error(`${key} 必须为 JSON 数字`); }
      if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${key} 必须为有限数字`);
    }
    if (type === "boolean") {
      if (!["true", "false"].includes(raw.trim())) throw new Error(`${key} 的布尔值应为 true 或 false`);
      value = raw.trim() === "true";
    }
    entries.push([key, value]);
  }
  return Object.fromEntries(entries);
}
async function persist(next, message) {
  if (busy || !loaded) return;
  busy = true; controls();
  try {
    const response = await fetch("/app/profiles", {
      method: "POST", headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ version: 1, profiles: next }),
    });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `保存失败（${response.status}）`);
    profiles = next; closeEditor(); notice(message);
  } catch (error) {
    notice(`${error.message}。编辑内容已保留，请核对文件后重试。`, true);
  } finally { busy = false; render(); }
}
$("editor").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (busy || !loaded || deleting) return;
  try {
    if (!$("name").value.trim()) throw new Error("名称不可为空");
    const id = editing || (crypto.randomUUID ? crypto.randomUUID() : Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, "0")).join(""));
    const profile = { id, name: $("name").value.trim(), provider: $("provider").value, provider_options: options(), notes: $("notes").value };
    const next = editing ? profiles.map((item) => item.id === editing ? profile : item) : [...profiles, profile];
    await persist(next, "档案已保存 · 参数是否有效以主控读取时的校验为准");
  } catch (error) { notice(error.message, true); }
});
$("editor").addEventListener("input", changed);
$("editor").addEventListener("change", changed);
$("add-option").addEventListener("click", () => { option(); changed(); });
$("add").addEventListener("click", () => edit());
$("cancel").addEventListener("click", closeEditor);
$("refresh").addEventListener("click", refresh);
$("delete").addEventListener("click", () => {
  if (busy || !editing) return;
  deleting = true;
  $("confirm-title").textContent = `删除“${profiles.find((item) => item.id === editing).name}”？`;
  $("confirmation").hidden = false; controls(); $("confirmation").focus();
});
$("keep").addEventListener("click", () => { deleting = false; $("confirmation").hidden = true; controls(); });
$("confirm").addEventListener("click", () => persist(profiles.filter((item) => item.id !== editing), "档案已删除"));
window.addEventListener("beforeunload", (event) => { if (dirty || busy) { event.preventDefault(); event.returnValue = ""; } });
refresh();
