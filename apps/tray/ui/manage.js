"use strict";
// Management is tray-only. Render all stored text as text nodes, never HTML.
const $ = (id) => document.getElementById(id);
const labels = {
  running: "运行中", review: "待审核", idle: "空闲", finished: "轮次完成",
  succeeded: "已回复", failed: "失败", blocked: "受阻", interrupted: "已中断", cancelled: "已取消",
};
let items = [];
let selected = new Set();
let pending = null;
let busy = false;
let loading = false;
let loadFailed = true;
let editing = null; // id of the conversation whose title is being edited
const day = 86400000;
const updated = (item) => Date.parse(item.updated) || item.updated_ms || NaN;
const title = (item) => item.title || "未命名会话";
function node(tag, text, className) {
  const el = document.createElement(tag);
  if (text != null) el.textContent = text;
  if (className) el.className = className;
  return el;
}
function notice(text, error = false) {
  $("notice").textContent = text;
  $("notice").classList.toggle("error", error);
}
function visibleItems() {
  const filter = $("filter").value;
  const before = $("before").value ? new Date(`${$("before").value}T00:00:00`).getTime() : Infinity;
  return items.filter((item) => {
    const matches = filter === "all"
      || (filter === "idle" && item.deletable)
      || (filter === "failed" && ["failed", "blocked", "interrupted"].includes(item.status));
    return matches && (before === Infinity || updated(item) < before);
  });
}
function controls() {
  const locked = busy || pending !== null || loading || loadFailed;
  $("delete").disabled = locked || selected.size === 0;
  $("delete").textContent = selected.size ? `删除所选 ${selected.size} 项…` : "删除所选…";
  $("cleanup").disabled = locked || items.every((item) => !item.deletable);
  for (const id of ["filter", "before", "days", "select-all"]) $(id).disabled = locked;
  $("refresh").disabled = busy || pending !== null || loading;
  $("confirm").disabled = busy;
  $("cancel").disabled = busy;
  for (const input of $("list").querySelectorAll("input[type=checkbox]")) {
    const item = items.find((item) => item.id === input.value);
    input.disabled = locked || !item?.deletable;
  }
}
function render() {
  const rows = visibleItems();
  const selectable = rows.filter((item) => item.deletable);
  $("total").textContent = String(items.length).padStart(2, "0");
  $("empty").hidden = rows.length !== 0;
  $("select-all").checked = selectable.length > 0 && selectable.every((item) => selected.has(item.id));
  $("select-all").indeterminate = selectable.some((item) => selected.has(item.id)) && !$("select-all").checked;
  $("list").replaceChildren(...rows.map((item) => {
    const row = node("tr");
    const checkCell = node("td", null, "check");
    const check = node("input");
    check.type = "checkbox";
    check.value = item.id;
    check.checked = selected.has(item.id);
    check.setAttribute("aria-label", `选择 ${title(item)}${item.reason ? `，${item.reason}` : ""}`);
    check.addEventListener("change", () => {
      if (check.checked) selected.add(item.id); else selected.delete(item.id);
      render();
      // Rendering replaces the checkbox; restore keyboard focus to the same row.
      for (const input of $("list").querySelectorAll("input")) if (input.value === item.id) input.focus();
    });
    checkCell.append(check);
    const name = node("td");
    // A readable snapshot lets the chat page locate this conversation's messages.
    const link = node(item.group ? "a" : "span", title(item), "title");
    if (item.group) link.href = `/chat/${encodeURIComponent(item.group)}#conv:${encodeURIComponent(item.id)}`;
    if (editing === item.id) {
      const input = node("input", null, "rename");
      input.value = title(item);
      input.maxLength = 200;
      input.setAttribute("aria-label", `新标题：${title(item)}`);
      input.addEventListener("keydown", (event) => {
        // Enter that confirms an input-method candidate is not a save.
        if (event.key === "Enter" && !event.isComposing && event.keyCode !== 229) rename(item, input.value);
        if (event.key === "Escape") { event.stopPropagation(); editing = null; render(); }
      });
      const save = node("button", "保存");
      save.type = "button";
      save.addEventListener("click", () => rename(item, input.value));
      const back = node("button", "取消");
      back.type = "button";
      back.addEventListener("click", () => { editing = null; render(); });
      name.append(input, save, back);
      queueMicrotask(() => { input.focus(); input.select(); });
    } else {
      const edit = node("button", "改名", "edit");
      edit.type = "button";
      // Renaming takes the same lock as deleting, so a busy or pending record cannot be renamed.
      edit.disabled = busy || pending !== null || !item.deletable;
      if (item.reason) edit.title = item.reason;
      edit.setAttribute("aria-label", `改名：${title(item)}`);
      edit.addEventListener("click", () => { editing = item.id; render(); });
      name.append(link, node("span", item.mode === "show" ? "节目" : "任务", `kind ${item.mode}`), edit);
    }
    const status = node("td");
    status.append(node("span", labels[item.status] || item.status || "未知", `status ${item.status}`));
    const date = node("td");
    const timestamp = updated(item);
    const time = node("time", Number.isFinite(timestamp) ? new Date(timestamp).toLocaleString("zh-CN", { hour12: false }) : "时间未知");
    if (Number.isFinite(timestamp)) time.dateTime = new Date(timestamp).toISOString();
    date.append(time);
    const reason = node("td");
    if (item.reason) reason.append(node("span", item.reason, "reason"));
    row.append(checkCell, name, status, date, reason);
    return row;
  }));
  controls();
}
async function rename(item, value) {
  const next = value.trim();
  if (!next || next === title(item)) { editing = null; render(); return; }
  busy = true;
  controls();
  try {
    const response = await fetch("/app/conversations/rename", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ id: item.id, title: next }),
    });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `改名失败（${response.status}）`);
    if (data.status !== "renamed") throw new Error(data.message);
    editing = null;
    busy = false;
    await refresh();
    notice(`已改名为“${data.title}”`);
  } catch (error) {
    busy = false;
    controls();
    notice(`未能改名：${error.message}`, true);
  }
}
async function refresh() {
  // Keep a title being edited: re-rendering the list would discard it.
  if (busy || pending || loading || editing) return;
  loading = true;
  controls();
  try {
    const response = await fetch("/manage/state", { cache: "no-store" });
    if (!response.ok) throw new Error(`读取失败（${response.status}）`);
    const data = await response.json();
    if (data.error) throw new Error(data.error);
    items = data.items.sort((a, b) => (updated(b) || 0) - (updated(a) || 0) || a.id.localeCompare(b.id));
    selected = new Set([...selected].filter((id) => items.some((item) => item.id === id && item.deletable)));
    loadFailed = false;
    notice(data.missing ? "尚未找到工作室数据目录" : `共 ${items.length} 个会话 · ${items.filter((item) => item.deletable).length} 个可删除`);
    const unavailable = data.unavailable || [];
    $("unavailable").hidden = unavailable.length === 0;
    $("unavailable-list").replaceChildren(...unavailable.map((item) => node("li", `${item.id || "记录"}：${item.reason}`)));
    render();
  } catch (error) {
    loadFailed = true;
    notice(`无法刷新：${error.message}。已保留上次列表，请刷新后再删除。`, true);
  } finally {
    loading = false;
    controls();
  }
}
function ask(candidates, source) {
  if (busy || pending || loading || loadFailed) return;
  if (!candidates.length) { notice("没有符合条件的可删除会话"); return; }
  pending = candidates.map((item) => ({ id: item.id, title: title(item) }));
  $("confirm-title").textContent = `${source}：删除 ${pending.length} 个会话？`;
  $("confirm-list").replaceChildren(...pending.map((item) => node("li", item.title)));
  $("confirmation").hidden = false;
  controls();
  $("confirmation").focus();
  $("confirmation").scrollIntoView({ block: "nearest" });
}
function cancel() {
  if (busy) return;
  pending = null;
  $("confirmation").hidden = true;
  controls();
  $("delete").focus();
}
async function remove() {
  if (!pending || busy) return;
  const batch = pending;
  busy = true;
  controls();
  notice(`正在删除 ${batch.length} 个会话，请稍候…`);
  try {
    const response = await fetch("/app/conversations/delete", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ ids: batch.map((item) => item.id) }),
    });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || `删除失败（${response.status}）`);
    $("results").hidden = false;
    $("result-list").replaceChildren(...data.results.map((result) => {
      const li = node("li", null, ["deleted", "missing"].includes(result.status) ? "" : "failure");
      li.append(node("strong", batch.find((item) => item.id === result.id)?.title || result.id), ` · ${result.message}`);
      if (result.status === "deleted" || result.status === "missing") selected.delete(result.id);
      return li;
    }));
  } catch (error) {
    $("results").hidden = false;
    $("result-list").replaceChildren(node("li", `未收到完整删除结果：${error.message}。请核对刷新后的列表再重试。`, "failure"));
  } finally {
    busy = false;
    pending = null;
    $("confirmation").hidden = true;
    await refresh();
  }
}
$("refresh").addEventListener("click", refresh);
// A filter may hide the row being renamed; drop that edit so refreshes resume.
for (const id of ["filter", "before"]) $(id).addEventListener("change", () => { selected.clear(); editing = null; render(); });
$("select-all").addEventListener("change", () => {
  for (const item of visibleItems().filter((item) => item.deletable)) {
    if ($("select-all").checked) selected.add(item.id); else selected.delete(item.id);
  }
  render();
});
$("delete").addEventListener("click", () => ask(items.filter((item) => selected.has(item.id) && item.deletable), "删除所选"));
$("cleanup").addEventListener("click", () => {
  const days = Number($("days").value);
  const cutoff = Date.now() - days * day;
  ask(items.filter((item) => item.deletable && updated(item) < cutoff), `清理 ${days} 天前的会话（全部会话）`);
});
$("cancel").addEventListener("click", cancel);
$("confirm").addEventListener("click", remove);
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && pending) cancel(); });
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
setInterval(() => { if (!document.hidden) refresh(); }, 5000);
refresh();
