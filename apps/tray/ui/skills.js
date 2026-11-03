"use strict";
{
"use strict";
// Settings -> 高级 -> 技能. Optional Skills from GET /app/skills, switched per host with POST
// /app/hosts/skill when 保存设置 is pressed; until then a picked switch only sits in S.want.
// The review gate's seats are drawn by advanced.js (renderReviewSeats) into the card built here.
const { h, $, load, post, L } = AN;
const S = { catalog: null, hosts: null, open: new Set(), want: new Map() };

// Settings each Skill adds under its own row; the review gate's seats are drawn by advanced.js.
const SETTINGS = {
  "agentnave-review": () => [
    h("h3", {}, L("评审工位", "Review seats")),
    h("p.guidance", {}, L("选“自动”时由 boss 按员工挑选；你在对话里点名的 CLI 和模型优先于这里。boss 应用用不了所选 CLI 时改用它自己的子代理，所选模型不在员工里时改找提供同一模型的其他 CLI。", "When set to “Auto”, the boss picks from configured employees; CLIs and models you name in chat take precedence. If the boss app cannot use the chosen CLI, it falls back to its own subagent; if the chosen model is not in employees, it looks for another CLI offering the same model.")),
    h("div.review-grid", { id: "review-seats" }),
  ],
};

const why = (host, state) =>
  state.state === "blocked" ? L("Skill 目录经过符号链接，无法安装", "Skill directory goes through a symlink and cannot be installed")
    : !state.enabled ? L(`点选为 ${host.label} 安装`, `Click to install for ${host.label}`)
      : state.state === "app" ? L("已安装，点选停用", "Installed; click to disable") : L("已选，接入这个应用时自动安装", "Selected; will install automatically when connected");

// One chip per detected host, so a host added later simply appears as another chip.
function chip(skill, host) {
  const state = host.skills?.[skill.name];
  if (!state || state.state === "manual" || host.status === "orphaned") return null;
  const key = `${skill.name}/${host.id}`;
  const blocked = state.state === "blocked";
  const saved = state.enabled && !blocked;
  const pending = S.want.has(key);
  const on = pending ? S.want.get(key).enabled : saved;
  const b = h("button.skill-chip", {
    type: "button", "data-focus-key": key, "aria-pressed": String(on), "data-pending": String(pending), title: why(host, state),
    disabled: blocked,
  }, AN.hostTile ? AN.hostTile(host.mark || host.id, host.label) : null, host.label,
    pending ? h("small", {}, L("待保存", "Unsaved")) : on && state.state !== "app" && h("small", {}, L("待接入", "Pending connection")));
  b.addEventListener("click", () => {
    if (pending) S.want.delete(key);
    else S.want.set(key, { host, skill: skill.name, enabled: !saved });
    render();
    advancedEdited();
  });
  return b;
}

function row(skill) {
  const chips = S.hosts.map((host) => chip(skill, host)).filter(Boolean);
  const installed = S.hosts.filter((host) => host.skills?.[skill.name]?.enabled && host.skills[skill.name].state !== "blocked").length;
  const open = S.open.has(skill.name);
  const reading = S.open.has(`${skill.name}/content`);
  const toggle = (key) => () => {
    if (S.open.has(key)) S.open.delete(key);
    else S.open.add(key);
    render();
  };
  const head = h("button.skill-head", { type: "button", "aria-expanded": String(open), "data-focus-key": `${skill.name}/open` },
    h("span.skill-name", {}, h("strong", {}, skill.title), h("small", {}, skill.description)),
    h("span.skill-count", {}, installed ? L(`已装到 ${installed} 个应用`, `Installed in ${installed} ${installed === 1 ? "app" : "apps"}`) : L("未使用", "Not used")),
    h("span.skill-chevron", { "aria-hidden": "true" }));
  head.addEventListener("click", toggle(skill.name));
  const more = h("button.skill-more", { type: "button", "aria-expanded": String(reading) }, reading ? L("收起写法", "Hide content") : L("查看写法", "View content"));
  more.addEventListener("click", toggle(`${skill.name}/content`));
  return h("div.skill-row", { "data-open": String(open) }, head, open && h("div.skill-body", {},
    h("h3", {}, L("安装到", "Install to")),
    chips.length ? h("div.skill-chips", {}, chips) : h("p.guidance", {}, L("还没有可安装的应用；在“boss”里接入后会出现在这里。", "No apps available yet; connect apps in “Boss” to see them here.")),
    h("p.skill-hint", {}, L("点选后按保存设置才会安装或停用。新接入的应用会自动出现。", "Click to select, then press Save settings to install or disable. Newly connected apps appear automatically.")),
    SETTINGS[skill.name]?.(),
    more,
    reading && h("pre.skill-content", {}, skill.content),
  ));
}

function render() {
  const box = $("skill-list");
  if (!S.catalog || !S.hosts) return;
  const focusKey = document.activeElement?.dataset?.focusKey;
  box.replaceChildren(h("div.skill-rows", {}, S.catalog.skills.map(row)));
  if (typeof renderReviewSeats === "function") renderReviewSeats();
  if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
}

// A host that left, or that reached the picked state some other way, has nothing to save.
function dropStale() {
  for (const [key, want] of S.want) {
    const host = S.hosts.find((x) => x.id === want.host.id);
    const state = host?.skills?.[want.skill];
    if (!state || state.state === "manual" || state.state === "blocked" || host.status === "orphaned" || state.enabled === want.enabled) S.want.delete(key);
  }
}

// Returns what failed; a failed switch stays picked for another try.
async function save() {
  const failures = [];
  for (const [key, want] of [...S.want]) {
    try {
      const result = await post("/app/hosts/skill", { id: want.host.id, skill: want.skill, enabled: want.enabled });
      S.hosts = S.hosts.map((x) => (x.id === want.host.id ? result.host : x));
      if (result.ok) S.want.delete(key);
      else failures.push(L(`${want.host.label}：${result.message}`, `${want.host.label}: ${result.message}`));
    } catch (e) {
      failures.push(L(`${want.host.label}：${e.message || "操作没有完成，请刷新核对"}`, `${want.host.label}: ${e.message || "Operation did not complete; refresh to check"}`));
    }
  }
  render();
  return failures;
}

async function refresh() {
  try {
    const [catalog, hosts] = await Promise.all([load("/app/skills"), load("/app/hosts")]);
    S.catalog = catalog;
    S.hosts = hosts.hosts;
    dropStale();
    render();
    renderAdvancedBar();
  } catch (e) {
    $("skill-list").replaceChildren(h("p.guidance", {}, L(`读取技能失败：${e.message || "未知错误"}`, `Failed to load skills: ${e.message || "Unknown error"}`)));
  }
}

function discard() {
  S.want.clear();
  render();
}

window.AgentNaveSkills = { load: refresh, changed: () => S.want.size > 0, save, discard };
// Hosts connected elsewhere (the 接入 page, another window) show up when the page is looked at again.
const visible = () => !$("pane-advanced").hidden && !document.hidden;
document.addEventListener("visibilitychange", () => visible() && refresh());
addEventListener("focus", () => visible() && refresh());
if (!$("pane-advanced").hidden) refresh();
}
