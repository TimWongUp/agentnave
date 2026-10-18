"use strict";
(function () {
"use strict";
// Settings -> 宿主接入. Pure DOM construction (h() = createElement/textContent), no innerHTML, no inline style.
// Data comes from GET /app/hosts; every write is a POST with X-AgentNave-Action (AN.post). The page never
// writes on its own: auto-takeover is the server's job at startup and on 刷新检测.
const { h, $, load, post, installService, relTime } = AN;


const KEEP_BACKUPS = 30;
const STATUS = {
  available: ["可接入", "info"],
  connected: ["已接入", "ok"],
  needs_attention: ["需要处理", "warn"],
  orphaned: ["已卸载但仍有条目", "idle"],
};
const ENTRY = { agentnave: "AgentNave", legacy: "旧版命令（待迁移）", other: "自定义命令" };

const S = {
  data: null,
  appVersion: "",
  busy: new Map(), // host id -> "connect" | "remove"
  confirm: null, // host id whose removal is being confirmed
  adv: new Set(), // host ids whose 高级 panel is open
  roots: new Map(),
  localErr: new Map(), // host id -> message when a request itself failed
  newIds: new Set(),
  seen: new Set(),
  firstRun: false, // nothing was ever seen: no per-card "新检测到" badges, the callout covers it
  refreshing: false,
  batch: false,
  install: { busy: false, error: null },
  focus: null, // {id, act} restored after a re-render
  updateKey: null,
  updating: false,
};

// ---------------------------------------------------------------- small helpers
const say = (text, bad = false) => {
  $("notice").textContent = text;
  $("notice").classList.toggle("error", bad);
};
const shortPath = (p) =>
  String(p || "").replace(/^(\/Users|\/home)\/[^/]+/, "~");
function copyButton(text, label = "复制") {
  const b = h(
    "button.link-btn",
    { type: "button", "aria-label": `${label}：${shortPath(text)}` },
    label,
  );
  b.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      b.textContent = "复制失败";
      setTimeout(() => (b.textContent = label), 1600);
      return;
    }
    b.textContent = "已复制";
    setTimeout(() => (b.textContent = label), 1600);
  });
  return b;
}
// "~/.gemini/config/" truncates in the middle of the directory part; the file name never does.
function pathRow(full, copy = true) {
  const short = shortPath(full);
  const i = short.lastIndexOf("/");
  return h(
    "span.pathrow",
    {},
    h(
      "code.path",
      { title: full },
      h("span.p-dir", {}, short.slice(0, i + 1)),
      h("span.p-file", {}, short.slice(i + 1)),
    ),
    copy && copyButton(full),
  );
}
const sourceText = (s) =>
  s === "path"
    ? "在 PATH 中找到可执行文件"
    : s === "config"
      ? "只在配置里发现条目"
      : s?.startsWith("/")
        ? `应用：${s}`
        : s === "app" ? "应用包" : s || "—";
const spinBtn = (label, cls = "") =>
  h(
    "button" + cls + ".pending",
    { type: "button", disabled: true },
    h("i.spin", { "aria-hidden": "true" }),
    label,
  );
function button(label, cls, act, onClick, disabled) {
  const b = h(
    "button" + cls,
    { type: "button", "data-act": act, disabled: disabled || null },
    label,
  );
  b.addEventListener("click", onClick);
  return b;
}

// ---------------------------------------------------------------- seen ("新检测到")
const store = (() => {
  try {
    return localStorage;
  } catch {
    return null;
  }
})();
const seenKey = "agentnave.hosts.seen";
function loadSeen() {
  try {
    const v = JSON.parse(store.getItem(seenKey) || "null");
    if (Array.isArray(v)) return new Set(v);
  } catch {
    /* fall through */
  }
  return new Set();
}
const saveSeen = () => {
  try {
    store.setItem(seenKey, JSON.stringify([...S.seen]));
  } catch {
    /* ignore */
  }
};
function markSeen(ids) {
  for (const id of ids) { const host = S.data.hosts.find((h) => h.id === id); S.seen.add(`${id}@${host?.first_seen || ""}`); }
  saveSeen();
}

// ---------------------------------------------------------------- data
function setData(data) {
  S.data = data;
  S.newIds = new Set(
    data.hosts
      .filter((x) => x.status === "available" && !S.seen.has(`${x.id}@${x.first_seen || ""}`))
      .map((x) => x.id),
  );
  renderAll();
}
async function reload() {
  try {
    setData(await load("/app/hosts"));
  } catch {
    /* keep what is shown */
  }
}
async function run(id, kind, request) {
  S.busy.set(id, kind);
  S.confirm = null;
  S.localErr.delete(id);
  renderAll();
  let result = null;
  try {
    result = await request();
  } catch (e) {
    S.localErr.set(
      id,
      `请求没有完成（${e.message || "未知错误"}），请刷新核对实际状态。`,
    );
  }
  S.busy.delete(id);
  if (result) say(result.message, !result.ok);
  await reload();
  return result;
}
const connect = (host) =>
  run(host.id, "connect", () =>
    post("/app/hosts/connect", { id: host.id }),
  ).then((r) => {
    if (r?.ok) {
      markSeen([host.id]);
      renderAll();
    }
    return r;
  });
const removeEntry = (host) =>
  run(host.id, "remove", () => post("/app/hosts/remove", { id: host.id }));
async function refreshAll() {
  S.refreshing = true;
  renderAll();
  const before = new Map((S.data?.hosts || []).map((x) => [x.id, x.last_action?.at]));
  try {
    const next = await post("/app/hosts/refresh");
    S.data = {
      service: next.service,
      hosts: next.hosts,
      backups: next.backups,
    };
    const added = next.hosts
      .filter((x) => !before.has(x.id))
      .map((x) => x.label);
    const touched = next.hosts.filter(
      (x) => before.has(x.id) && x.last_action?.at !== before.get(x.id),
    ).length;
    const parts = [
      added.length && `发现新宿主 ${added.join("、")}`,
      touched && `更新了 ${touched} 个宿主的条目或 Skill`,
    ].filter(Boolean);
    say(
      parts.length ? `检测完成：${parts.join("，")}。` : "检测完成，没有变化。",
    );
  } catch (e) {
    say(`刷新检测失败：${e.message || "未知错误"}`, true);
  }
  S.refreshing = false;
  setData(S.data);
}
async function installSvc() {
  S.install = { busy: true, error: null };
  renderAll();
  try {
    await installService();
    say("本机服务已安装。");
  } catch (e) {
    S.install.error = e.message || "安装失败，请重试。";
  }
  S.install.busy = false;
  await reload();
  window.dispatchEvent(new CustomEvent("agentnave:service-installed"));
}
async function connectAll(ids) {
  S.batch = true;
  renderAll();
  for (const id of ids) {
    const host = S.data.hosts.find((x) => x.id === id);
    if (host) await connect(host);
  }
  S.batch = false;
  renderAll();
}

// ---------------------------------------------------------------- page chrome
function renderBanner() {
  const el = $("svc-banner");
  const svc = S.data.service;
  el.hidden = false;
  el.className = "banner service";
  el.setAttribute("role", "status");
  if (S.install.busy) {
    el.classList.add("busy");
    el.replaceChildren(
      h(
        "div.banner-content",
        {},
        h("strong", {}, "正在安装本机服务…"),
        h("p", {}, "从 GitHub 安装，约需一分钟。可以先看看下面检测到的宿主。"),
      ),
      h("div.banner-actions", {}, spinBtn("安装中…", ".primary")),
    );
  } else if (S.install.error) {
    el.className = "banner error";
    el.setAttribute("role", "alert");
    el.replaceChildren(
      h(
        "div.banner-content",
        {},
        h("strong", {}, "安装本机服务失败"),
        h("p", {}, S.install.error),
      ),
      h(
        "div.banner-actions",
        {},
        copyButton(S.data.service.command, "复制命令"),
        button("重试", "", "install", installSvc),
      ),
    );
  } else if (!svc.installed) {
    const title = "请安装或更新本机服务";
    const desc = "宿主统一通过 agentnave-mcp 启动 AgentNave，保存任务与协作历史。旧版服务需要先升级。";
    el.replaceChildren(
      h("div.banner-content", {}, h("strong", {}, title), h("p", {}, desc)),
      h(
        "div.banner-actions",
        {},
        copyButton(S.data.service.command, "复制命令"),
        button(
          svc.installed ? "重新安装本机服务" : "安装本机服务",
          ".primary",
          "install",
          installSvc,
        ),
      ),
    );
  } else el.hidden = true;
}

function renderCallout() {
  const el = $("new-callout");
  const svcOk = S.data.service.installed;
  const fresh = S.data.hosts.filter((x) => S.newIds.has(x.id));
  el.hidden = !svcOk || !fresh.length;
  if (el.hidden) return;
  const tested = fresh.filter((x) => x.tested);
  const untested = fresh.length - tested.length;
  const names = fresh.map((x) => x.label).join("、");
  const title =
    fresh.length === 1
      ? `新检测到 ${names}`
      : `检测到 ${fresh.length} 个可接入的宿主`;
  const sub =
    fresh.length === 1
      ? untested
        ? "它还没有经过真实接入验证，仍可接入。"
        : "统一接入 AgentNave，已有条目的其他字段保留。"
      : `${names}${untested ? `；其中 ${untested} 个未实测，需在各自卡片上单独接入` : ""}`;
  const targets = fresh.length === 1 ? fresh : tested;
  const go = targets.length
    ? button(fresh.length === 1 ? `接入 ${fresh[0].label}` : `接入其中 ${tested.length} 个已实测的`, ".primary", "new-connect", () => connectAll(targets.map((x) => x.id)), S.batch || S.refreshing)
    : null;
  const ignore = button(
    "忽略",
    "",
    "new-ignore",
    () => {
      markSeen(S.data.hosts.map((x) => x.id));
      renderAll();
    },
    S.batch,
  );
  el.replaceChildren(
    h("div.callout-text", {}, h("strong", {}, title), h("small", {}, sub)),
    h("div.banner-actions", {}, go, ignore),
  );
}

function renderSummary() {
  const hosts = S.data.hosts;
  const n = (s) => hosts.filter((x) => x.status === s).length;
  const svc = S.data.service;
  const chip = (color, text) =>
    h("span.sum-chip", {}, h("i.dot", { style: `--c:${color}` }), text);
  const attention = n("needs_attention");
  const kids = [
    h("span", {}, h("strong", {}, String(hosts.length)), " 个宿主"),
  ];
  if (n("connected")) kids.push(chip("var(--run)", `${n("connected")} 已接入`));
  if (n("available"))
    kids.push(chip("var(--task)", `${n("available")} 可接入`));
  if (attention) {
    const b = h(
      "button.sum-chip",
      { type: "button" },
      `${attention} 需要处理 ↓`,
    );
    b.addEventListener("click", () => {
      const card = document.querySelector(".hc.tone-warn");
      card?.scrollIntoView({ block: "center", behavior: "smooth" });
      card
        ?.querySelector("button:not(:disabled)")
        ?.focus({ preventScroll: true });
    });
    kids.push(b);
  }
  if (n("orphaned"))
    kids.push(chip("var(--muted)", `${n("orphaned")} 个已卸载但仍有条目`));
  kids.push(
    svc.installed
      ? chip(
          "var(--run)",
          "本机服务：已安装 ✓",
        )
      : chip("var(--accent)", "本机服务：未安装"),
  );
  $("hosts-summary").replaceChildren(...kids);
  const refresh = $("refresh-hosts");
  refresh.disabled = S.refreshing || S.batch;
  refresh.replaceChildren(
    ...(S.refreshing
      ? [h("i.spin", { "aria-hidden": "true" }), "检测中…"]
      : ["刷新检测"]),
  );
  refresh.classList.toggle("pending", S.refreshing);
  $("hosts-badge").hidden = !attention;
}

function renderBackups() {
  const b = S.data.backups;
  $("backups-line").replaceChildren(
    h(
      "span",
      {},
      b.count
        ? `配置备份：已备份 ${b.count} 份，保留最近 ${KEEP_BACKUPS} 份`
        : `配置备份：还没有备份（每次写入前自动备份，保留最近 ${KEEP_BACKUPS} 份）`,
    ),
    h("span.sep", { "aria-hidden": "true" }, "·"),
    pathRow(b.dir),
    button("打开备份目录", "", "reveal", async () => { try { const r = await post("/app/hosts/reveal-backups"); say(r.message, !r.ok); } catch { say("无法打开备份目录", true); } }),
  );
}

// ---------------------------------------------------------------- host card
function skillNode(host) {
  const s = host.skill;
  if (s.state === "app") {
    const old = s.version && S.appVersion && s.version !== S.appVersion;
    return h(
      "span",
      {},
      `App 管理 v${s.version}`,
      old && h("span.sub", {}, `· 刷新检测时更新到 v${S.appVersion}`),
    );
  }
  if (s.state === "external")
    return h(
      "span",
      { title: s.path || null },
      "由其他方式管理",
      h("span.sub", {}, "· App 不会改动"),
    );
  if (s.state === "manual")
    return h(
      "span",
      {},
      h("span.warn-text", {}, "无法自动安装"),
      h("span.sub", {}, "· 该宿主的 Skill 目录尚未确定"),
    );
  const auto =
    host.status === "connected" &&
    host.entry === "agentnave";
  return h(
    "span",
    {},
    h("span.dim", {}, "未安装"),
    auto && h("span.sub", {}, "· 刷新检测时自动安装"),
  );
}

function entryNode(host) {
  if (host.status === "available")
    return h(
      "span",
      {},
      h("span.chip", {}, "AgentNave"),
      h("span.sub", {}, "接入后使用"),
    );
  const kids = [h("span.chip", {}, ENTRY[host.entry] || "—")];
  if (host.entry === "other")
    kids.push(h("span.sub.block", {}, "命令不是 App 提供的入口"));
  if (host.status !== "orphaned" && !host.has_host_env)
    kids.push(h("span.sub.block.warn-text", {}, "缺少 AGENTNAVE_HOST"));
  return h("span", {}, ...kids);
}

function advPanel(host) {
  const id = host.id;
  const busy = S.busy.has(id);
  const rootInput = h("input.root-input", { id: `root-${id}`, type: "text", value: S.roots.get(id) ?? host.root.path, disabled: busy || null, "aria-label": `${host.label} 的配置根目录`, placeholder: "绝对路径" });
  rootInput.addEventListener("input", () => S.roots.set(id, rootInput.value));
  const saveRoot = (path) => run(id, "root", () => post("/app/hosts/root", { id, path })).then((r) => { if (r?.ok) { S.roots.delete(id); renderAll(); } });
  const rootEditor = h("div.root-editor", {},
    h("label", { for: `root-${id}` }, "配置根目录"), rootInput,
    h("p.fine", {}, "通常保持自动即可。只有把这个应用的用户配置放在其他位置时才需要修改；填写绝对路径，保存后立即生效。“恢复自动”会重新使用检测到的目录。"),
    h("span.fine", {}, `来源：${({ user: "App 中的覆盖", env: "App 环境变量", shell: "登录 shell", default: "默认目录" })[host.root.source] || host.root.source}`),
    h("div.adv-actions", {}, button("保存根目录", "", "root-save", () => saveRoot(rootInput.value.trim()), busy), button("恢复自动", "", "root-auto", () => saveRoot(null), busy)));
  const kv = [
    [
      "命令",
      host.command ? pathRow(host.command) : h("span.dim", {}, "尚未写入"),
    ],
    [
      "AGENTNAVE_HOST",
      host.has_host_env
        ? h("span", {}, `已设置（${host.id}）`)
        : h(
            "span.dim",
            {},
            host.status === "available" ? "接入时写入" : "未设置",
          ),
    ],
    [
      "Skill 目录",
      host.skill.path
        ? pathRow(host.skill.path)
        : h("span.dim", {}, "未确定，无法自动安装"),
    ],
    ["检测来源", h("span", {}, host.detect_source === "app" ? `应用：${host.app_path || "已安装"}` : sourceText(host.detect_source))],
  ];
  return h(
    "div.hc-adv",
    { id: `adv-${id}` },
    rootEditor,
    h("div.fine", {}, (host.layers || []).filter((l) => l.has_entry).map((l) => `${l.scope === "project" ? "项目（只读）" : "用户层"}：${l.path}`).join("；")),
    h(
      "dl.adv-kv",
      {},
      kv.map(([k, v]) => [h("dt", {}, k), h("dd", {}, v)]),
    ),
  );
}

function confirmBlock(host) {
  const orphan = host.status === "orphaned";
  const skill =
    host.skill.state === "app"
      ? `由 App 安装的 Skill（v${host.skill.version}）会一并删除`
      : host.skill.state === "external"
        ? "该宿主的 Skill 由其他方式管理，不会改动"
        : "";
  const paths = (host.layers || []).filter((l) => l.scope !== "project" && l.has_entry).map((l) => shortPath(l.path));
  const where = `${(paths.length ? paths : [shortPath(host.config_path)]).join("、")} 里名为 agentnave 的条目`;
  const msg = orphan
    ? `将删除 ${where}（写入前会备份）。${host.label} 本身不会受影响${skill ? `；${skill}` : ""}。`
    : `只删除 ${where}（写入前会备份），其他配置不动${skill ? `；${skill}` : ""}。之后可以随时重新接入。`;
  return h(
    "div.hc-confirm",
    { role: "group", "aria-label": `确认移除 ${host.label}` },
    h(
      "div.msg",
      {},
      h(
        "strong",
        {},
        orphan
          ? `移除 ${host.label} 的残留条目？`
          : `移除 ${host.label} 的接入？`,
      ),
      msg,
    ),
    h(
      "div.acts",
      {},
      button("取消", "", "cancel", () => {
        S.confirm = null;
        S.focus = { id: host.id, act: "remove" };
        renderAll();
      }),
      button(
        orphan ? "确认移除条目" : "确认移除",
        ".danger-solid",
        "confirm",
        () => removeEntry(host),
      ),
    ),
  );
}

function actionsRow(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const svcOk = S.data.service.installed;
  const left = [];
  if (host.status !== "orphaned") {
    const open = S.adv.has(id);
    const adv = h(
      "button.adv-btn",
      {
        type: "button",
        "data-act": "adv",
        "aria-expanded": String(open),
        "aria-controls": `adv-${id}`,
      },
      h("i.chev", { "aria-hidden": "true" }),
      "高级",
    );
    adv.addEventListener("click", () => {
      if (open) S.adv.delete(id);
      else S.adv.add(id);
      S.focus = { id, act: "adv" };
      renderAll();
    });
    left.push(adv);
  }
  const right = [];
  const dis = !!busy;
  if (host.status === "available") {
    if (!svcOk)
      right.push(
        h(
          "span.fine",
          {},
          S.install.busy ? "正在安装本机服务…" : "需先安装本机服务",
        ),
      );
    right.push(
      busy === "connect"
        ? spinBtn("接入中…", ".primary")
        : button(
            "接入",
            ".primary",
            "connect",
            () => connect(host),
            dis || !svcOk || S.batch,
          ),
    );
  } else if (host.status === "needs_attention") {
    right.push(
      busy === "remove"
        ? spinBtn("移除中…")
        : button(
            "移除",
            ".danger",
            "remove",
            () => {
              S.confirm = id;
              S.focus = { id, act: "cancel" };
              renderAll();
            },
            dis,
          ),
    );
    right.push(
      busy === "connect"
        ? spinBtn("重试中…", ".primary")
        : button(
            "重试接入",
            ".primary",
            "connect",
            () => connect(host),
            dis || !svcOk,
          ),
    );
  } else if (host.status === "connected") {
    right.push(
      busy === "remove"
        ? spinBtn("移除中…")
        : button(
            "移除",
            ".danger",
            "remove",
            () => {
              S.confirm = id;
              S.focus = { id, act: "cancel" };
              renderAll();
            },
            dis,
          ),
    );
  } else {
    right.push(
      busy === "remove"
        ? spinBtn("移除中…")
        : button(
            "移除条目",
            ".danger",
            "remove",
            () => {
              S.confirm = id;
              S.focus = { id, act: "cancel" };
              renderAll();
            },
            dis,
          ),
    );
  }
  return h("footer.hc-foot", {}, ...left, h("span.spacer"), ...right);
}

function buildCard(host) {
  const id = host.id;
  const [word, tone] = STATUS[host.status] || [host.status, "idle"];
  const busy = S.busy.get(id);
  const orphan = host.status === "orphaned";
  const isNew = !S.firstRun && S.newIds.has(id);
  const card = h(
    "article.hc.tone-" +
      tone +
      (busy ? ".is-busy" : "") +
      (orphan ? ".is-orphan" : ""),
    {
      "data-host": id,
      "aria-labelledby": `name-${id}`,
      "aria-busy": busy ? "true" : null,
    },
  );
  card.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && S.confirm === id) {
      S.confirm = null;
      S.focus = { id, act: "remove" };
      renderAll();
    }
  });
  const badges = [
    host.reload?.state === "required" && h("span.badge.badge-warning", {}, "待重启确认"),
    host.reload?.state === "confirmed" && h("span.badge.badge-run", {}, "已确认重启"),
    !host.tested &&
      h(
        "span.badge.badge-muted",
        {
          title:
            "还没有经过真实接入验证：写入逻辑按官方文档实现，遇到问题请反馈。",
        },
        "未实测",
      ),
    isNew && h("span.badge.badge-accent", {}, "新检测到"),
  ];
  const main = [
    h(
      "div.hc-line",
      {},
      h(
        "div.hc-id",
        {},
        h("h3.hc-name", { id: `name-${id}` }, host.label),
        ...badges,
      ),
      h("span.hc-status", {}, h("i.dot", { "aria-hidden": "true" }), word),
    ),
  ];
  if (orphan)
    main.push(
      h(
        "p.hc-orphan",
        {},
        `本机已检测不到 ${host.label}，但它的配置里还留着 agentnave 条目。留着不影响使用；不再需要时可以移除。`,
      ),
    );
  main.push(
    h(
      "dl.hc-facts",
      {},
      h("div", {}, h("dt", {}, "入口"), h("dd", {}, entryNode(host))),
      h("div", {}, h("dt", {}, "Skill"), h("dd", {}, skillNode(host))),
      h(
        "div",
        {},
        h("dt", {}, "生效"),
        h(
          "dd",
          {},
          host.reload_hint || "接入、更新服务或 Skill 后，重启宿主应用以加载新版工具接口",
        ),
      ),
      h(
        "div.wide",
        {},
        h(
          "dt",
          {},
          host.status === "available" ? "配置文件（接入时写入）" : "配置文件",
        ),
        h("dd", {}, pathRow(host.config_path)),
      ),
    ),
  );
  if (host.shadowed_by)
    main.push(
      h(
        "div.hc-note.info",
        {},
        "项目级配置 ",
        h("code", {}, shortPath(host.shadowed_by)),
        " 里也有 agentnave 条目，在那个项目里会优先使用它。App 只看不改项目级配置。",
      ),
    );
  const local = S.localErr.get(id);
  const last = host.last_action;
  if (local) main.push(h("div.hc-note.error", { role: "alert" }, local));
  else if (last && !last.ok)
    main.push(
      h(
        "div.hc-note.error",
        { role: "alert" },
        last.message,
        h(
          "small",
          {},
          `${relTime(last.at)}${host.status === "needs_attention" ? " · 修好后点“重试接入”，或在下方“高级”里换个入口" : ""}`,
        ),
      ),
    );
  else if (host.attention) main.push(h("div.hc-note.error", { role: "alert" }, host.attention.message));
  else if (last)
    main.push(
      h(
        "p.hc-last",
        {},
        h("span.tick", { "aria-hidden": "true" }, "✓"),
        `${last.message} · ${relTime(last.at)}`,
      ),
    );
  if (host.reload?.state === "required") {
    main.push(h("div.hc-reload.hc-note.info", {},
      h("div", {}, h("strong", {}, "需要重启或重新连接后确认"),
        h("p", {}, "服务、Skill 或接入配置有变化，或尚未确认生效。请按上方方式重启 / 重连后确认；下次变更会再次提醒。")),
      button("我已重启 / 重连", "", "confirm-reload", () =>
        run(id, "confirm-reload", () => post("/app/hosts/confirm-reload", { id, token: host.reload.token })),
        Boolean(busy) || S.updating || host.status !== "connected"),
    ));
  } else if (host.reload?.state === "confirmed") {
    main.push(h("p.hc-last", {}, "已按你的确认记录当前版本；软件未自动验证宿主接口。"));
  } else if (host.reload?.state === "unknown" && host.status === "connected") {
    main.push(h("p.hc-last", {}, "无法判断此连接是否需要重启，请按上方生效方式手动重连。"));
  }
  main.push(S.confirm === id && !busy ? confirmBlock(host) : actionsRow(host));
  // Keep the disclosure button above its content so it stays under the pointer.
  if (S.adv.has(id) && host.status !== "orphaned") main.push(advPanel(host));
  card.append(h("div.hc-main", {}, ...main));
  return card;
}

function renderCards() {
  const box = $("host-cards");
  if (!S.data.hosts.length) {
    box.replaceChildren(
      h(
        "div.cards-empty",
        {},
        h("strong", {}, "没有检测到受支持的宿主"),
        "支持 Codex、Claude Code、CodeBuddy Code、WorkBuddy 桌面、Grok CLI、Antigravity、Pi、Gemini CLI、OpenCode、Hermes。安装其中任意一个后点“刷新检测”。",
      ),
    );
    return;
  }
  box.replaceChildren(...S.data.hosts.map(buildCard));
}

function renderAll() {
  if (!S.data) return;
  window.dispatchEvent(new CustomEvent("agentnave:hosts", { detail: S.data }));
  renderBanner();
  renderCallout();
  renderSummary();
  renderBackups();
  renderCards();
  if (S.focus) {
    const { id, act } = S.focus;
    S.focus = null;
    document
      .querySelector(`.hc[data-host="${id}"] [data-act="${act}"]`)
      ?.focus({ preventScroll: true });
  }
}

$("refresh-hosts").addEventListener("click", refreshAll);
window.addEventListener("agentnave:status", ({ detail }) => {
  if (!S.data) return;
  const state = detail.service;
  S.updating = ["checking", "installing"].includes(detail.update?.state) || ["upgrading", "installing"].includes(state?.state);
  const key = JSON.stringify([detail.version, state]);
  if (S.updateKey !== key) {
    S.updateKey = key;
    reload();
  }
  const installing = state?.state === "installing";
  if (!installing && (!S.install.busy || !["done", "failed"].includes(state?.state))) return;
  S.install.busy = installing;
  S.install.error = state?.state === "failed" ? state.error : null;
  if (installing) renderAll();
  else reload();
});
$("onboarding-reopen").addEventListener("click", async () => {
  try { await post("/app/onboarding/reopen"); say("已重新显示开始使用，返回群聊或面板即可查看。"); }
  catch { say("重新显示失败，请稍后重试。", true); }
});

// ---------------------------------------------------------------- boot
(async function boot(attempt = 0) {
  if (!$("pane-hosts").hidden) say("正在读取宿主状态…");
  S.seen = loadSeen();
  S.firstRun = S.seen.size === 0;
  try {
    const [hosts, status] = await Promise.all([
      load("/app/hosts"),
      load("/app/status"),
    ]);
    S.appVersion = status.version || "";
    S.install.busy = status.service?.state === "installing";
    S.install.error = status.service?.state === "failed" ? status.service.error : null;
    setData(hosts);
    if (!$("pane-hosts").hidden) say("宿主状态已读取。");
  } catch {
    // The tray answers 503 until its first host detection finishes; retry briefly before giving up.
    if (attempt < 10) { setTimeout(() => boot(attempt + 1), 1000); return; }
    say("无法读取宿主状态，请稍后点“刷新检测”重试。", true);
    $("hosts-summary").replaceChildren(h("span.dim", {}, "读取失败"));
  }
})();

})();
