"use strict";
(function () {
"use strict";
// Settings -> 技能. Optional Skills from GET /app/skills, switched per host with POST /app/hosts/skill.
// The review gate's seats are drawn by settings.js (renderReviewSeats) into the card built here.
const { h, $, load, post } = AN;
const S = { catalog: null, hosts: null, open: new Set(), busy: new Set() };

// Settings each Skill adds under its own row; the review gate's seats are drawn by settings.js.
const SETTINGS = {
  "agentnave-review": () => [
    h("h3", {}, "评审工位"),
    h("p.guidance", {}, "选“自动”时由主控按组员挑选；你在对话里点名的 CLI 和模型优先于这里。宿主用不了所选 CLI 时改用宿主自己的子代理，所选模型不在组员里时改找提供同一模型的其他 CLI。改动立即保存。"),
    h("div.review-grid", { id: "review-seats" }),
  ],
};

const why = (host, state) =>
  state.state === "external" ? "由其他方式管理，App 不会改动"
    : !state.enabled ? `点选为 ${host.label} 安装`
      : state.state === "app" ? "已安装，点选停用" : "已选，接入此宿主时自动安装";

// One chip per detected host, so a host added later simply appears as another chip.
function chip(skill, host) {
  const state = host.skills?.[skill.name];
  if (!state || state.state === "manual" || host.status === "orphaned") return null;
  const key = `${skill.name}/${host.id}`;
  const external = state.state === "external";
  const on = state.enabled && !external;
  const b = h("button.skill-chip", {
    type: "button", "data-focus-key": key, "aria-pressed": String(on), title: why(host, state),
    disabled: external || S.busy.has(key),
  }, AN.hostTile ? AN.hostTile(host.id, host.label) : null, host.label,
    on && state.state !== "app" && h("small", {}, "待接入"));
  b.addEventListener("click", async () => {
    S.busy.add(key);
    render();
    try {
      const result = await post("/app/hosts/skill", { id: host.id, skill: skill.name, enabled: !on });
      S.hosts = S.hosts.map((x) => (x.id === host.id ? result.host : x));
      showNotice(`${host.label}：${result.message}`, !result.ok);
    } catch (e) {
      showNotice(`${host.label}：${e.message || "操作没有完成，请刷新核对"}`, true);
    }
    S.busy.delete(key);
    render();
  });
  return b;
}

function row(skill) {
  const chips = S.hosts.map((host) => chip(skill, host)).filter(Boolean);
  const installed = S.hosts.filter((host) => host.skills?.[skill.name]?.enabled && host.skills[skill.name].state !== "external").length;
  const open = S.open.has(skill.name);
  const reading = S.open.has(`${skill.name}/content`);
  const toggle = (key) => () => {
    if (S.open.has(key)) S.open.delete(key);
    else S.open.add(key);
    render();
  };
  const head = h("button.skill-head", { type: "button", "aria-expanded": String(open), "data-focus-key": `${skill.name}/open` },
    h("span.skill-name", {}, h("strong", {}, skill.title), h("small", {}, skill.description)),
    h("span.skill-count", {}, installed ? `已装到 ${installed} 个宿主` : "未使用"),
    h("span.skill-chevron", { "aria-hidden": "true" }));
  head.addEventListener("click", toggle(skill.name));
  const more = h("button.skill-more", { type: "button", "aria-expanded": String(reading) }, reading ? "收起写法" : "查看写法");
  more.addEventListener("click", toggle(`${skill.name}/content`));
  return h("div.skill-row", { "data-open": String(open) }, head, open && h("div.skill-body", {},
    h("h3", {}, "安装到"),
    chips.length ? h("div.skill-chips", {}, chips) : h("p.guidance", {}, "还没有可安装的宿主；在“接入”里接入后会出现在这里。"),
    h("p.skill-hint", {}, "点选宿主即安装或停用；灰色的由其他方式管理。新接入的宿主会自动出现。"),
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

async function refresh() {
  try {
    const [catalog, hosts] = await Promise.all([load("/app/skills"), load("/app/hosts")]);
    S.catalog = catalog;
    S.hosts = hosts.hosts;
    render();
  } catch (e) {
    $("skill-list").replaceChildren(h("p.guidance", {}, `读取技能失败：${e.message || "未知错误"}`));
  }
}

window.AgentNaveSkills = { load: refresh };
// Hosts connected elsewhere (the 接入 page, another window) show up when the page is looked at again.
const visible = () => !$("pane-skills").hidden && !document.hidden;
document.addEventListener("visibilitychange", () => visible() && refresh());
addEventListener("focus", () => visible() && refresh());
if (location.hash === "#skills") refresh();
})();
