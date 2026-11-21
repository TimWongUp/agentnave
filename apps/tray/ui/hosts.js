"use strict";
{
"use strict";
// Settings -> 宿主接入. Pure DOM construction (h() = createElement/textContent), no innerHTML, no inline style.
// Data comes from GET /app/hosts; every write is a POST with X-AgentNave-Action (AN.post). The page never
// writes on its own: auto-takeover is the server's job at startup and on 刷新检测.
const { h, $, load, post, installService, relTime, L } = AN;
const { copyAct, sameCopy, cliCopyAction, cliCopyCwd, copyConfirmState, stopCopy } = AN.hostCopies;


const STATUS = {
  available: [L("可接入", "Available"), "info"],
  connected: [L("已接入", "Connected"), "ok"],
  needs_attention: [L("需要处理", "Needs attention"), "warn"],
  orphaned: [L("已卸载但仍有条目", "Uninstalled (entry remains)"), "idle"],
};
const ENTRY = { agentnave: "AgentNave", other: L("自定义命令", "Custom command") };

const S = {
  data: null,
  appVersion: "",
  busy: new Map(), // host id -> "connect" | "remove"
  confirm: null, // host id whose removal is being confirmed
  copyConfirm: null, // {id, kind, pid?, since?, terminal?, cwd?} of the copy whose stop/restart is being confirmed
  adv: new Set(), // host ids whose 更多信息 rows are open
  sel: null, // host id shown in the detail pane (the first row when unset or gone)
  othersOpen: false, // the 未安装的 boss fold
  localErr: new Map(), // host id -> message when a request itself failed
  newIds: new Set(),
  seen: new Set(),
  firstRun: false, // nothing was ever seen: no per-host "新检测到" badges, the callout covers it
  refreshing: false,
  batch: false,
  install: { busy: false, error: null },
  focus: null, // {id, act} restored after a re-render
  updateKey: null,
  hostCli: null, // { choices, bosses } from GET /app/orchestration: what each boss does with its own CLI
  updating: false,
  refreshResult: "",
};

// ---------------------------------------------------------------- small helpers
const say = (text, bad = false) => {
  $("notice").textContent = text;
  $("notice").classList.toggle("error", bad);
};
const shortPath = (p) =>
  String(p || "").replace(/^(\/Users|\/home)\/[^/]+/, "~");
const reloadPending = (host) => ["required", "unverified"].includes(host.reload?.state);
// Connected bosses first; the rest keep their order below them.
const chosen = (host) => (host.layers || []).some((l) => l.has_entry && l.scope !== "project");
const orderedHosts = () => [...S.data.hosts].sort((a, b) => chosen(b) - chosen(a));
const selectedHost = () => {
  const hosts = orderedHosts();
  return hosts.find((x) => x.id === S.sel) || hosts[0];
};
// Picking another row drops a confirmation that belonged to the one left behind.
function selectHost(id) {
  if (selectedHost()?.id !== id) {
    S.confirm = null;
    S.copyConfirm = null;
  }
  S.sel = id;
  S.focus = { id, act: "select" };
  renderAll();
}
function showHost(match) {
  const host = orderedHosts().find(match);
  if (!host) return;
  selectHost(host.id);
  $("host-cards").scrollIntoView({ block: "nearest", behavior: "smooth" });
}
function copyButton(text, label = L("复制", "Copy")) {
  const b = h(
    "button.link-btn",
    { type: "button", "aria-label": L(`${label}：${shortPath(text)}`, `${label}: ${shortPath(text)}`) },
    label,
  );
  b.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      b.textContent = L("复制失败", "Copy failed");
      setTimeout(() => (b.textContent = label), 1600);
      return;
    }
    b.textContent = L("已复制", "Copied");
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
function sourceText(s) {
  if (s === "path") return L("在 PATH 中找到可执行文件", "Executable found in PATH");
  if (s === "config") return L("只在配置里发现条目", "Entry found in config only");
  if (s === "store") return L("应用商店", "App store");
  if (s === "package") return L("系统软件包", "System package");
  if (s === "app") return L("应用包", "App bundle");
  return s?.startsWith("/") ? L(`应用：${s}`, `App: ${s}`) : s || "—";
}
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
  // A confirmation for a copy that has since gone never renders, and would hold every refresh.
  const pending = S.copyConfirm;
  const host = pending && data.hosts.find((x) => x.id === pending.id);
  if (pending && !(host && ["required", "unverified"].includes(host.reload?.state)
    && (host.reload.copies || []).some((copy) => sameCopy(copy, pending)) && copyAction(host, pending)))
    S.copyConfirm = null;
  S.newIds = new Set(
    data.hosts
      .filter((x) => x.status === "available" && !S.seen.has(`${x.id}@${x.first_seen || ""}`))
      .map((x) => x.id),
  );
  renderAll();
}
async function reload() {
  try {
    const [next, roles] = await Promise.all([load("/app/hosts"), loadHostCli()]);
    const idle = !S.busy.size && !S.confirm && !S.copyConfirm && !document.activeElement?.closest(".hc-adv, .hc-hostcli");
    const rolesChanged = JSON.stringify(roles) !== JSON.stringify(S.hostCli);
    if (idle) S.hostCli = roles;
    if (idle && JSON.stringify(next) !== JSON.stringify(S.data)) setData(next);
    else if (idle && rolesChanged) renderAll();
  } catch {
    /* keep what is shown */
  }
}
// A file that cannot be read shows no choice rather than a wrong one.
async function loadHostCli() {
  try {
    const roles = await load("/app/orchestration");
    return roles.error ? null : { choices: roles.document.host_cli || {}, bosses: roles.bosses || [] };
  } catch {
    return null;
  }
}
async function run(id, kind, request) {
  S.busy.set(id, kind);
  S.confirm = null;
  S.copyConfirm = null;
  S.localErr.delete(id);
  renderAll();
  let result = null;
  try {
    result = await request();
  } catch (e) {
    S.localErr.set(
      id,
      L(`请求没有完成（${e.message || "未知错误"}），请刷新核对实际状态。`, `Request did not complete (${e.message || "Unknown error"}); refresh to check actual status.`),
    );
  }
  S.busy.delete(id);
  if (result) say(result.message, !result.ok);
  else renderAll();
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
const reloadCopy = (host, kind) =>
  run(host.id, `copy-${kind}`, () => post("/app/hosts/reload-copy", { id: host.id, kind })).then((r) => {
    if (r && !r.ok) {
      S.localErr.set(host.id, r.message);
      renderAll();
    }
    return r;
  });
async function refreshAll() {
  S.refreshing = true;
  renderAll();
  const before = new Map((S.data?.hosts || []).map((x) => [x.id, x.last_action?.at]));
  try {
    const next = await post("/app/hosts/refresh");
    S.data = {
      service: next.service,
      hosts: next.hosts,
      absent: next.absent,
      backups: next.backups,
    };
    const added = next.hosts
      .filter((x) => !before.has(x.id))
      .map((x) => x.label);
    const touched = next.hosts.filter(
      (x) => before.has(x.id) && x.last_action?.at !== before.get(x.id),
    ).length;
    const parts = [
      added.length && L(`发现 ${added.join("、")}`, `found ${added.join(", ")}`),
      touched && L(`更新了 ${touched} 个应用的条目或 Skill`, `updated entries or Skills for ${touched} ${touched === 1 ? "app" : "apps"}`),
    ].filter(Boolean);
    S.refreshResult = parts.length ? L(`检测完成：${parts.join("，")}。`, `Detection complete: ${parts.join(", ")}.`) : L("检测完成，没有变化。", "Detection complete, no changes.");
    say(S.refreshResult);
  } catch (e) {
    S.refreshResult = L(`刷新检测失败：${e.message || "未知错误"}`, `Refresh detection failed: ${e.message || "Unknown error"}`);
    say(S.refreshResult, true);
  }
  S.refreshing = false;
  setData(S.data);
}
async function installSvc() {
  S.install = { busy: true, error: null };
  renderAll();
  try {
    await installService();
    say(L("本机服务已安装。", "Local service installed."));
  } catch (e) {
    S.install.error = e.message || L("安装失败，请重试。", "Installation failed. Please try again.");
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
        h("strong", {}, L("正在安装本机服务…", "Installing local service…")),
        h("p", {}, L("从 GitHub 安装，约需一分钟。可以先看看下面检测到的应用。", "Installing from GitHub (~1 min). You can check the detected apps below in the meantime.")),
      ),
      h("div.banner-actions", {}, spinBtn(L("安装中…", "Installing…"), ".primary")),
    );
  } else if (S.install.error) {
    el.className = "banner error";
    el.setAttribute("role", "alert");
    el.replaceChildren(
      h(
        "div.banner-content",
        {},
        h("strong", {}, L("安装本机服务失败", "Failed to install local service")),
        h("p", {}, S.install.error),
      ),
      h(
        "div.banner-actions",
        {},
        copyButton(S.data.service.command, L("复制命令", "Copy command")),
        button(L("重试", "Retry"), "", "install", installSvc),
      ),
    );
  } else if (!svc.installed) {
    const title = L("请安装或更新本机服务", "Install or update local service");
    const desc = L("boss 统一通过 agentnave-mcp 启动 AgentNave，保存任务与协作历史。旧版服务需要先升级。", "Bosses start AgentNave via agentnave-mcp to save task and collaboration history. Older services must be upgraded first.");
    el.replaceChildren(
      h("div.banner-content", {}, h("strong", {}, title), h("p", {}, desc)),
      h(
        "div.banner-actions",
        {},
        copyButton(S.data.service.command, L("复制命令", "Copy command")),
        button(
          svc.installed ? L("重新安装本机服务", "Reinstall local service") : L("安装本机服务", "Install local service"),
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
  const names = fresh.map((x) => x.label).join(L("、", ", "));
  const title =
    fresh.length === 1
      ? L(`新检测到 ${names}`, `Newly detected: ${names}`)
      : L(`检测到 ${fresh.length} 个可接入的应用`, `Detected ${fresh.length} apps available to connect`);
  const sub =
    fresh.length === 1
      ? untested
        ? L("它还没有经过真实接入验证，仍可接入。", "It has not been verified with a live connection yet, but can still be connected.")
        : L("统一接入 AgentNave，已有条目的其他字段保留。", "Connect to AgentNave; other fields in existing entries are preserved.")
      : L(`${names}${untested ? `；其中 ${untested} 个未实测，需在各自详情里单独接入` : ""}`, `${names}${untested ? `; ${untested} untested (connect individually in each detail)` : ""}`);
  const targets = fresh.length === 1 ? fresh : tested;
  const go = targets.length
    ? button(fresh.length === 1 ? L(`接入 ${fresh[0].label}`, `Connect ${fresh[0].label}`) : L(`接入其中 ${tested.length} 个已实测的`, `Connect ${tested.length} tested`), ".primary", "new-connect", () => connectAll(targets.map((x) => x.id)), S.batch || S.refreshing)
    : null;
  const ignore = button(
    L("忽略", "Ignore"),
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
    h("span", {}, h("strong", {}, String(hosts.length)), L(" 个应用", ` ${hosts.length === 1 ? "app" : "apps"}`)),
  ];
  if (n("connected")) kids.push(chip("var(--run)", L(`${n("connected")} 已接入`, `${n("connected")} connected`)));
  if (n("available"))
    kids.push(chip("var(--task)", L(`${n("available")} 可接入`, `${n("available")} available`)));
  if (attention) {
    const b = h(
      "button.sum-chip",
      { type: "button" },
      L(`${attention} 需要处理`, `${attention} needs attention`),
    );
    b.addEventListener("click", () => showHost((x) => x.status === "needs_attention"));
    kids.push(b);
  }
  const reloads = hosts.filter((x) => ["required", "unverified"].includes(x.reload?.state)).length;
  if (reloads) {
    const b = h("button.sum-chip", { type: "button" }, L(`${reloads} 待生效`, `${reloads} pending reload`));
    b.addEventListener("click", () => showHost(reloadPending));
    kids.push(b);
  }
  const faults = hosts.filter((x) => x.fault).length;
  if (faults) {
    const b = h("button.sum-chip", { type: "button" }, L(`${faults} 连接中断`, `${faults} connection lost`));
    b.addEventListener("click", () => showHost((x) => x.fault));
    kids.push(b);
  }
  if (n("orphaned"))
    kids.push(chip("var(--muted)", L(`${n("orphaned")} 个已卸载但仍有条目`, `${n("orphaned")} uninstalled`)));
  kids.push(
    svc.installed
      ? chip(
          "var(--run)",
          L("本机服务：已安装 ✓", "Local service: Installed ✓"),
        )
      : chip("var(--accent)", L("本机服务：未安装", "Local service: Not installed")),
  );
  $("hosts-summary").replaceChildren(...kids);
  const refresh = $("refresh-hosts");
  refresh.disabled = S.refreshing || S.batch;
  refresh.replaceChildren(
    ...(S.refreshing
      ? [h("i.spin", { "aria-hidden": "true" }), L("检测中…", "Detecting…")]
      : [L("刷新检测", "Refresh detection")]),
  );
  refresh.classList.toggle("pending", S.refreshing);
  $("refresh-result").textContent = S.refreshing ? "" : S.refreshResult;
  $("hosts-badge").hidden = !attention && !reloads && !faults;
  // The general reload note only matters while some host still needs a reload.
  $("reload-notice").hidden = !reloads;
}

function renderBackups() {
  const b = S.data.backups;
  $("backups-line").replaceChildren(
    h(
      "span",
      {},
      b.config || b.skill
        ? L(`备份：配置文件 ${b.config} 份、Skill 文件 ${b.skill} 份`, `Backups: ${b.config} config, ${b.skill} Skill`)
        : L("备份：还没有", "Backups: None yet"),
    ),
    h("span.sep", { "aria-hidden": "true" }, "·"),
    pathRow(b.dir),
    button(L("打开备份目录", "Open backup folder"), "", "reveal", async () => { try { const r = await post("/app/hosts/reveal-backups"); say(r.message, !r.ok); } catch { say(L("无法打开备份目录", "Failed to open backup folder"), true); } }),
  );
}

// ---------------------------------------------------------------- host list and detail
// The App installs and updates the Skill with every connection, start, update and refresh, so a
// connected app only shows the version, or why it could not be installed.
function skillNode(host) {
  const s = host.skill;
  if (s.state === "blocked")
    return h("span", {}, h("span.warn-text", {}, L("无法安装", "Cannot install")), h("span.sub", {}, L("· Skill 目录经过符号链接", "· Skill directory goes through a symlink")));
  if (s.state === "manual")
    return h("span", {}, h("span.warn-text", {}, L("无法安装", "Cannot install")), h("span.sub", {}, L("· 这个应用的 Skill 目录尚未确定", "· Skill directory for this app is not determined yet")));
  if (host.status !== "connected") return h("span.dim", {}, L("接入时安装", "Installed on connect"));
  if (s.state === "app" && (!S.appVersion || s.version === S.appVersion)) return h("span", {}, `v${s.version}`);
  return h("span.warn-text", {}, L("未能安装，点「刷新检测」重试", "Failed to install; click “Refresh detection” to retry"));
}

function entryNode(host) {
  if (host.status === "available")
    return h(
      "span",
      {},
      h("span.chip", {}, "AgentNave"),
      h("span.sub", {}, L("接入后使用", "Used after connecting")),
    );
  const kids = [h("span.chip", {}, ENTRY[host.entry] || "—")];
  if (host.entry === "other")
    kids.push(h("span.sub.block", {}, L("命令不是 App 提供的入口", "Command is not the entry provided by the App")));
  if (host.status !== "orphaned" && !host.has_host_env)
    kids.push(h("span.sub.block.warn-text", {}, L("缺少 AGENTNAVE_HOST", "Missing AGENTNAVE_HOST")));
  return h("span", {}, ...kids);
}

// One aligned row: the label in the left column, what it holds on the right.
function detailRow(label, ...content) {
  return h("div.member-row.host-row", {},
    h("span.member-scene", {}, label),
    h("div.member-line", {}, ...content.filter(Boolean)));
}

function advRows(host) {
  const id = host.id;
  const busy = S.busy.has(id);
  // The root is detected; a path typed in an earlier version can only be dropped again.
  const saveRoot = (path) => run(id, "root", () => post("/app/hosts/root", { id, path })).then((r) => { if (r?.ok) renderAll(); });
  const sources = { user: L("之前手动指定", "Manual (legacy)"), env: L("App 环境变量", "App environment"), shell: L("登录 shell", "Login shell"), default: L("默认目录", "Default directory") };
  const layers = (host.layers || []).filter((l) => l.has_entry);
  return h("div.hc-adv.detail-rows-group", { id: `adv-${id}` },
    detailRow(L("配置根目录", "Config root"), pathRow(host.root.path)),
    detailRow(L("根目录来源", "Root source"), h("span.host-text", {}, sources[host.root.source] || host.root.source),
      host.root.source === "user" && button(L("恢复自动", "Reset to auto"), "", "root-auto", () => saveRoot(null), busy)),
    layers.length > 0 && detailRow(L("已写入的配置", "Written configs"), h("div.host-stack", {},
      layers.map((l) => h("span.host-text", {}, `${l.scope === "project" ? L("项目（只读）", "Project (read-only)") : L("用户层", "User")}：${shortPath(l.path)}`)))),
    detailRow(L("命令", "Command"), host.command ? pathRow(host.command) : h("span.host-text.dim", {}, L("尚未写入", "Not written yet"))),
    detailRow("AGENTNAVE_HOST", host.has_host_env
      ? h("span.host-text", {}, L(`已设置（${host.boss}）`, `Set (${host.boss})`))
      : h("span.host-text.dim", {}, host.status === "available" ? L("接入时写入", "Written on connect") : L("未设置", "Not set"))),
    detailRow(L("Skill 目录", "Skill directory"), host.skill.path ? pathRow(host.skill.path) : h("span.host-text.dim", {}, L("未确定，无法自动安装", "Undetermined; cannot auto-install"))),
    detailRow(L("检测来源", "Detected via"), h("span.host-text", {}, host.detect_source === "app" ? L(`应用：${host.app_path || "已安装"}`, `App: ${host.app_path || "Installed"}`) : sourceText(host.detect_source))),
  );
}

function confirmBlock(host) {
  const orphan = host.status === "orphaned";
  const skill = ["app", "external"].includes(host.skill.state) ? L("Skill 会一并删除", "Skill will also be removed") : "";
  const paths = (host.layers || []).filter((l) => l.scope !== "project" && l.has_entry).map((l) => shortPath(l.path));
  const where = L(`${(paths.length ? paths : [shortPath(host.config_path)]).join("、")} 里名为 agentnave 的条目`, `the agentnave entry in ${(paths.length ? paths : [shortPath(host.config_path)]).join(", ")}`);
  const msg = orphan
    ? L(`将删除 ${where}（写入前会备份），并摘掉不再使用的 AgentNave 规则块${host.id === "antigravity" ? "和交互 hook" : ""}${skill ? `；${skill}` : ""}。${host.label} 本身不会受影响。`, `Removes ${where} (backed up before writing), unused AgentNave rules blocks${host.id === "antigravity" ? " and the interactive hook" : ""}${skill ? `; ${skill}` : ""}. ${host.label} itself is unaffected.`)
    : L(`将删除 ${where}（写入前会备份），并摘掉不再使用的 AgentNave 规则块${host.id === "antigravity" ? "和交互 hook" : ""}${skill ? `；${skill}` : ""}。之后可以随时重新接入。`, `Removes ${where} (backed up before writing), unused AgentNave rules blocks${host.id === "antigravity" ? " and the interactive hook" : ""}${skill ? `; ${skill}` : ""}. You can reconnect anytime.`);
  return h(
    "div.disable-confirm",
    { role: "group", "aria-label": L(`确认移除 ${host.label}`, `Confirm removing ${host.label}`) },
    h(
      "p",
      {},
      h(
        "strong",
        {},
        orphan
          ? L(`移除 ${host.label} 的残留条目？`, `Remove leftover entry for ${host.label}?`)
          : L(`移除 ${host.label} 的接入？`, `Disconnect ${host.label}?`),
      ),
      msg,
    ),
    h(
      "div.btn-group",
      {},
      button(L("取消", "Cancel"), "", "cancel", () => {
        S.confirm = null;
        S.focus = { id: host.id, act: "remove" };
        renderAll();
      }),
      button(
        orphan ? L("确认移除条目", "Remove entry") : L("确认移除", "Disconnect"),
        ".danger-solid",
        "confirm",
        () => removeEntry(host),
      ),
    ),
  );
}

// The detail head's one forward action: what can be done to this host now. Removal lives in the foot.
function primaryAction(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const svcOk = S.data.service.installed;
  const dis = !!busy;
  const out = [];
  if (host.status === "available") {
    if (!svcOk) out.push(h("span.fine", {}, S.install.busy ? L("正在安装本机服务…", "Installing local service…") : L("需先安装本机服务", "Install local service first")));
    out.push(busy === "connect"
      ? spinBtn(L("接入中…", "Connecting…"), ".primary")
      : button(L("接入", "Connect"), ".primary", "connect", () => connect(host), dis || !svcOk || S.batch));
  } else if (host.status === "needs_attention") {
    out.push(busy === "connect"
      ? spinBtn(L("重试中…", "Retrying…"), ".primary")
      : button(L("重试接入", "Retry"), ".primary", "connect", () => connect(host), dis || !svcOk));
  } else if (host.status === "connected" && host.launcher?.stale && svcOk) {
    // A connected host whose entry names another launcher is still writable: the same connect
    // path rewrites just the command and keeps the rest of the entry.
    out.push(busy === "connect"
      ? spinBtn(L("切换中…", "Switching…"), ".primary")
      : button(L("改用新启动器", "Use new launcher"), ".primary", "connect", () => connect(host), dis));
  }
  return out;
}

function removeButton(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  if (busy === "remove") return spinBtn(L("移除中…", "Removing…"));
  const label = host.status === "connected" || host.status === "needs_attention" ? L("移除", "Remove") : L("移除条目", "Remove entry");
  return button(label, ".danger", "remove", () => {
    S.confirm = id;
    S.copyConfirm = null;
    S.focus = { id, act: "cancel" };
    renderAll();
  }, !!busy || S.confirm === id);
}

// What the list row and the detail's status row say: the state, then anything still pending.
function hostState(host) {
  const busy = S.busy.get(host.id);
  const [word, tone] = STATUS[host.status] || [host.status, "idle"];
  const pending = [];
  if (host.fault) pending.push(L("连接中断", "Connection lost"));
  if (host.reload?.state === "required") pending.push(L("待重开", "Reload pending"));
  else if (host.reload?.state === "unverified") pending.push(L("待确认生效", "Pending verification"));
  const failed = host.status === "needs_attention" || Boolean(S.localErr.get(host.id) || (host.last_action && !host.last_action.ok) || host.attention);
  if (busy) return { text: { connect: L("接入中…", "Connecting…"), remove: L("移除中…", "Removing…") }[busy] || L("处理中…", "Working…"), tone: "run", busy: true, flag: null };
  return {
    text: [word, ...pending].join(" · "),
    tone: failed ? "bad" : pending.length ? "warn" : { ok: "run", info: "muted", warn: "warn", idle: "muted" }[tone] || "muted",
    busy: false,
    flag: failed ? "error" : pending.length ? "pending" : null,
  };
}

function readersText(host) {
  const readers = [...new Set((host.clients || []).map((client) => client.kind))];
  return readers.map((kind) => (kind === "desktop" ? L("桌面端", "Desktop") : "CLI")).join(" · ");
}

function listRow(host, selected) {
  const state = hostState(host);
  const flagTitle = state.flag === "error" ? L("需要处理", "Needs attention") : L("有待处理事项", "Pending actions");
  const readers = readersText(host);
  const row = h(
    "button.member-list-item" + (selected ? ".is-selected" : "") + (host.status === "orphaned" && !selected ? ".is-off" : ""),
    { type: "button", "data-host": host.id, "data-act": "select", "aria-pressed": String(selected) },
    h("span.cli-icon", { "aria-hidden": "true" }, AN.hostTile(host.mark || host.id, host.label)),
    h("span.member-item-body", {},
      h("span.member-item-top", {},
        h("span.member-item-name", { title: host.label }, host.label),
        readers && h("span.member-item-version.host-readers", {}, readers),
        state.flag && h("span.member-item-flag." + (state.flag === "error" ? "is-error" : "is-dirty"), { title: flagTitle, "aria-label": flagTitle }, state.flag === "error" ? "!" : null)),
      h("span.member-item-status" + (state.busy ? ".is-busy" : ""), {},
        h("span.member-status-dot.tone-" + state.tone, { "aria-hidden": "true" }),
        h("span.member-item-status-text", { title: state.text }, state.text))));
  row.addEventListener("click", () => {
    if (!selected) selectHost(host.id);
  });
  return row;
}

// A note above the rows: something this host needs from the user.
function notes(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const orphan = host.status === "orphaned";
  const out = [];
  const local = S.localErr.get(id);
  const last = host.last_action;
  if (local) out.push(h("div.hc-note.error", { role: "alert" }, local));
  else if (last && !last.ok)
    out.push(h("div.hc-note.error", { role: "alert" }, last.message,
      h("small", {}, `${relTime(last.at)}${host.status === "needs_attention" ? L(" · 修好后点“重试接入”", " · Click “Retry” after fixing") : ""}`)));
  else if (host.attention) out.push(h("div.hc-note.error", { role: "alert" }, host.attention.message));
  if (host.fault) out.push(faultNote(host));
  // Configuration changes remain visible until the host stops or a new connection is observed.
  if (reloadPending(host)) {
    out.push(reloadNote(host), copiesGroup(host));
    const pending = S.copyConfirm?.id === id && !busy && copyAction(host, S.copyConfirm);
    if (pending) out.push(copyConfirmBlock(host, S.copyConfirm, pending));
  }
  if (orphan)
    out.push(h("div.hc-note.info", {}, L(`本机已检测不到 ${host.label}，但它的配置里还留着 agentnave 条目。留着不影响使用；不再需要时可以移除。`, `${host.label} is no longer detected locally, but its config still has the agentnave entry. Leaving it is harmless; remove it if no longer needed.`)));
  if (host.shadowed_by)
    out.push(h("div.hc-note.info", {}, L("项目级配置 ", "Project config "), h("code", {}, shortPath(host.shadowed_by)), L(" 里也有 agentnave 条目，在那个项目里会优先使用它。App 只看不改项目级配置。", " also has an agentnave entry, which takes precedence in that project. The App never modifies project configs.")));
  if (host.launcher?.stale)
    out.push(h("div.hc-note.info", {},
      L("配置里的启动器仍是旧的 ", "Configured launcher is still the old "), h("code", {}, shortPath(host.launcher.current || "")),
      L("；重新接入会把 command 改为 ", "; reconnecting updates command to "), h("code", {}, shortPath(host.launcher.expected || ""))));
  return out.filter(Boolean);
}

function detailPanel(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const orphan = host.status === "orphaned";
  const state = hostState(host);
  const isNew = !S.firstRun && S.newIds.has(id);
  const open = S.adv.has(id);
  const last = host.last_action;
  const panel = h("div.detail-panel.host-detail", { "data-host": id, "aria-labelledby": `name-${id}`, "aria-busy": busy ? "true" : null });
  panel.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && S.confirm === id) {
      S.confirm = null;
      S.focus = { id, act: "remove" };
      renderAll();
    } else if (e.key === "Escape" && S.copyConfirm?.id === id) {
      S.focus = { id, act: copyAct(S.copyConfirm) };
      S.copyConfirm = null;
      renderAll();
    }
  });
  const head = h("div.detail-head", {},
    h("div.member-title", {},
      h("span.cli-icon", { "aria-hidden": "true" }, AN.hostTile(host.mark || id, host.label)),
      h("h3.cli-name", { id: `name-${id}`, title: host.label }, host.label),
      // Only a host not yet connected can be untested: once its entry works it has been tried.
      !host.tested && host.status === "available" &&
        h("span.badge.badge-muted", { title: L("还没有经过真实接入验证：写入逻辑按官方文档实现，遇到问题请反馈。", "Not verified with a live connection yet: implemented per official docs; please report any issues.") }, L("未实测", "Untested")),
      isNew && h("span.badge.badge-accent", {}, L("新检测到", "New"))),
    h("div.host-head-act", {}, ...primaryAction(host)));

  const more = h("button.cli-missing-toggle", { type: "button", "data-act": "adv", "aria-expanded": String(open), "aria-controls": `adv-${id}` },
    h("span.cli-chevron", { "aria-hidden": "true" }), L("更多信息", "More info"));
  more.addEventListener("click", () => {
    if (open) S.adv.delete(id);
    else S.adv.add(id);
    S.focus = { id, act: "adv" };
    renderAll();
  });
  const statusExtra = host.status === "available" ? L("本机已安装，还没有接入 AgentNave", "Installed locally, not connected to AgentNave yet") : "";
  const readers = readersText(host);
  const rows = h("div.detail-rows", {},
    ...notes(host),
    detailRow(L("状态", "Status"),
      h("span.hc-status.tone-" + state.tone + (busy ? ".is-busy" : ""), {}, h("i.dot", { "aria-hidden": "true" }), state.text),
      statusExtra && h("span.host-text.dim", {}, statusExtra)),
    readers && detailRow(L("发现", "Found"), h("span.host-text", {}, readers)),
    hostCliRow(host),
    detailRow(L("入口", "Entry"), entryNode(host)),
    detailRow("Skill", skillNode(host)),
    detailRow(L("配置文件", "Config file"), pathRow(host.config_path), host.status === "available" && h("span.host-text.dim", {}, L("接入时写入", "Written on connect"))),
    open && last && last.ok && detailRow(L("最近操作", "Last action"), h("span.host-text.hc-last", {}, h("span.tick", { "aria-hidden": "true" }, "✓"), `${last.message} · ${relTime(last.at)}`)),
    !orphan && h("div.host-more", {}, more),
    open && !orphan && advRows(host));

  panel.append(head, rows);
  const removable = host.status !== "available";
  if (removable)
    panel.append(h("div.detail-foot", {},
      S.confirm === id && !busy && confirmBlock(host),
      h("div.detail-foot-actions", {}, removeButton(host))));
  return panel;
}

// What a connected boss does with a task whose CLI is its own; a choice equal to the host's
// default stays out of the file. Rows that share rules files share the choice.
const HOST_CLI_CHOICES = [
  ["subagent", L("用 boss 自己的子代理", "Boss's own subagent")],
  ["launch", L("由 AgentNave 另开", "Started by AgentNave")],
  ["skip", L("改用其他 CLI", "Use another CLI")],
];
function hostCliRow(host) {
  const boss = host.status === "connected" && S.hostCli?.bosses.find((row) => row.keys.includes(host.id));
  if (!boss) return null;
  const choice = S.hostCli.choices[host.id]
    || boss.keys.map((key) => S.hostCli.choices[key]).find(Boolean)
    || host.default_host_cli
    || "subagent";
  const label = L("同款 CLI 的任务", "Tasks for its own CLI");
  const tip = L(
    "工位或员工用的 CLI 和这个 boss 相同时（比如在 Claude Code 里派给 Claude 的任务）怎么执行。用 boss 自己的子代理：boss 用自带的子代理完成，不经过 AgentNave。由 AgentNave 另开：另启动一个同款 CLI，和 boss 共用账号额度。改用其他 CLI：跳过它，换用备选或其他员工。开会不受影响。",
    "How a task runs when its seat or employee uses the same CLI as this boss (say, a Claude task sent from Claude Code). Boss's own subagent: the boss finishes it with its built-in subagent, without AgentNave. Started by AgentNave: another copy of the CLI starts, sharing the boss's account quota. Use another CLI: it is skipped for a fallback or another employee. Meetings are not affected.",
  );
  const select = h("select.member-model", { "aria-label": label, "data-act": "host-cli", disabled: Boolean(S.busy.get(host.id)) },
    HOST_CLI_CHOICES.map(([value, text]) => h("option", { value, selected: value === choice }, text)));
  select.addEventListener("change", async () => {
    S.focus = { id: host.id, act: "host-cli" };
    S.localErr.delete(host.id);
    try {
      const saved = await post("/app/orchestration/host-cli", { keys: boss.keys, choice: select.value });
      S.hostCli.choices = saved.host_cli;
      window.dispatchEvent(new CustomEvent("agentnave:host-cli", { detail: saved.host_cli }));
    } catch (e) {
      S.localErr.set(host.id, L(`没有保存（${e.message || "未知错误"}）`, `Not saved (${e.message || "Unknown error"})`));
    }
    renderAll();
  });
  return h("div.member-row.host-row.hc-hostcli", {},
    h("span.member-scene", {}, label, h("span.setting-tip", { tabindex: "0", role: "note", "aria-label": tip }, "?")),
    h("div.member-line", {}, select));
}

// Why a reload is required; the per-copy instructions below the title say how to reopen.
// All but a config change clear themselves.
const RELOAD_REASONS = {
  config: [L("接入配置已更新", "Connection config updated"), L("检测到新连接后自动消除提示。", "This notice clears automatically once a new connection is detected.")],
  host_running: [L("接入配置已更新", "Connection config updated"), L("应用仍在运行旧配置。", "The app is still running with the old config.")],
  tools_upgrade: [L("服务工具已更新", "Service tools updated"), L("重新连接后自动消除提示。", "This notice clears after reconnecting.")],
  unverified: [L("接入配置已更新", "Connection config updated"), L("尚未确认应用已加载新配置。", "Not yet verified that the app loaded the new config.")],
  tools: [L("有连接仍在用旧工具接口", "A connection is still using the old tool interface"), L("在宿主里重新连接 AgentNave MCP 也可以；完成后提示自动消除。", "Reconnecting AgentNave MCP in the host also works; this notice clears automatically once done.")],
  stale: [L("需要重连", "Reconnect required"), L("在宿主里重新连接 AgentNave MCP 也可以让新版服务生效。", "Reconnecting AgentNave MCP in the host also loads the new service.")],
};
const COPY_KIND = {
  daemon: L("后台服务", "Background service"),
  desktop: L("桌面端", "Desktop app"),
  app: L("桌面端", "Desktop app"),
  cli: L("终端会话", "Terminal session"),
  ide: L("编辑器插件", "Editor extension"),
  gateway: L("网关", "Gateway"),
};
// Labels for the copies the host's own CLI can stop or restart: [button, busy label, confirm
// title, consequence]. The snapshot's copy_actions decides which ones the host offers.
const COPY_ACTIONS = {
  "claude/daemon": [L("停止", "Stop"), L("停止中…", "Stopping…"), L("停止 Claude 后台服务？", "Stop the Claude background service?"), L("将结束 Claude 的全部后台会话。", "All Claude background sessions will end.")],
  "codex/daemon": [L("重启", "Restart"), L("重启中…", "Restarting…"), L("重启 Codex 后台服务？", "Restart the Codex background service?"), L("正在运行的后台任务和远程任务会中断。", "Running background and remote tasks will be interrupted.")],
  "hermes/gateway": [L("重启", "Restart"), L("重启中…", "Restarting…"), L("重启 Hermes 网关？", "Restart the Hermes gateway?"), L("消息平台上的对话会短暂中断，随后自动重连。", "Conversations on messaging platforms will briefly drop and then reconnect.")],
};
const copyAction = (host, copy) =>
  copy.kind === "cli"
    ? cliCopyAction(host, copy)
    : (host.copy_actions || []).includes(copy.kind) && COPY_ACTIONS[`${host.boss}/${copy.kind}`];
const bundleName = (path) => String(path || "").split("/").pop().replace(/\.app$/i, "");

// One row per old copy still holding the previous config: what it is, since when, how to reopen it.
function copyRow(host, copy) {
  const id = host.id;
  const busy = S.busy.get(id);
  const action = copyAction(host, copy);
  const act = copyAct(copy);
  const which = [copy.bundle && bundleName(copy.bundle), copy.kind === "cli" && copy.terminal].filter(Boolean).join(" · ");
  const since = copy.since ? relTime(new Date(copy.since).toISOString()) : "";
  let control = null;
  if (action)
    control = busy === act
      ? spinBtn(action[1])
      : button(action[0], "", act, () => {
          S.copyConfirm = copyConfirmState(id, copy);
          S.confirm = null;
          S.focus = { id, act: "copy-cancel" };
          renderAll();
        }, Boolean(busy) || S.updating || S.copyConfirm?.id === id);
  const label = h("span.member-scene", {}, COPY_KIND[copy.kind] || copy.kind,
    ["daemon", "gateway"].includes(copy.kind) &&
      AN.settings.settingTip(L("这是应用在后台常驻的进程，关掉窗口也不会退出；它重启后才会用上新配置。", "This is the app’s resident background process; closing the window doesn’t stop it, and it only picks up the new config after a restart.")));
  return h("div.member-row.host-row.host-copy-row", {},
    label,
    h("div.member-line", {},
      h("div.host-copy-body", {},
        (which || since) && h("p.host-text", {}, which && h("strong", {}, which),
          since && h("span.hc-since", {}, which ? L(` · ${since}${/\d$/.test(since) ? " " : ""}启动`, ` · started ${since}`) : L(`${since}${/\d$/.test(since) ? " " : ""}启动`, `started ${since}`))),
        cliCopyCwd(copy),
        host.reopen?.[copy.kind] && h("p.host-text.dim", {}, host.reopen[copy.kind])),
      h("div.hc-copy-act", {}, control)));
}

// The service died while the host was still connected; the user's confirmation or a newer healthy
// connection of the row clears it (ADR-0096).
function faultNote(host) {
  const id = host.id;
  return h("div.hc-reload.hc-note.info", { "data-fault": "" },
    h("div.hc-reload-head", {},
      h("div", {},
        h("strong", {}, L(`与 ${host.label} 的连接已中断`, `Connection to ${host.label} lost`)),
        h("p", {}, L(`AgentNave 服务在 ${host.label} 仍打开时退出了。请检查 ${host.label}，重新连接 AgentNave MCP 或重启它。`, `The AgentNave service exited while ${host.label} was still open. Check ${host.label}, then reconnect AgentNave MCP or restart it.`))),
      button(L("知道了", "Dismiss"), "", "dismiss-fault", () =>
        run(id, "dismiss-fault", () => post("/app/hosts/dismiss-fault", { id })),
        Boolean(S.busy.get(id)) || S.updating),
    ),
  );
}

function reloadNote(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const [title, detail] = RELOAD_REASONS[host.reload.reason] || RELOAD_REASONS.config;
  // A host that never connected may be waiting for its own in-app trust prompt, which this App
  // cannot click; a host with a known trust step names it on its own line instead.
  const trust = !host.trust && host.reload.reason === "unverified" && !host.reload.connections
    ? L("若该宿主从未连接过 AgentNave，可能需要在它的应用内确认信任「agentnave」后才会启动服务。", "If this host has never connected to AgentNave, you may need to trust “agentnave” inside its app before the service starts.")
    : "";
  const auto = host.reload.reason in RELOAD_REASONS && host.reload.reason !== "config";
  const copies = host.reload.copies || [];
  const fallback = host.reopen?.[host.id === "claude-desktop" ? "desktop" : host.reopen?.app ? "app" : "cli"] || host.reload_hint;
  return h("div.hc-reload.hc-note.info", {},
    h("div.hc-reload-head", {},
      h("div", {}, h("strong", {}, title), h("p", {}, detail + (trust ? ` ${trust}` : ""))),
      !auto && button(L("我已重新加载", "I’ve reloaded"), "", "confirm-reload", () =>
        run(id, "confirm-reload", () => post("/app/hosts/confirm-reload", { id, token: host.reload.token })),
        Boolean(busy) || S.updating || host.status !== "connected"),
    ),
    !copies.length && fallback && h("p.hc-reopen", {}, fallback),
    host.trust && h("p.hc-trust", {}, host.trust),
  );
}

// The old copies of a host whose reload is pending, one aligned row each.
function copiesGroup(host) {
  const copies = host.reload.copies || [];
  return copies.length ? h("div.host-copies", {}, ...copies.map((copy) => copyRow(host, copy))) : null;
}

function copyConfirmBlock(host, confirm, [label, , title, consequence]) {
  const cancel = () => {
    S.copyConfirm = null;
    S.focus = { id: host.id, act: copyAct(confirm) };
    renderAll();
  };
  return h(
    "div.hc-confirm",
    { role: "group", "aria-label": title },
    h("div.msg", {}, h("strong", {}, title), consequence),
    h("div.acts", {},
      button(L("取消", "Cancel"), "", "copy-cancel", cancel),
      button(L(`确认${label}`, `Confirm ${label.toLowerCase()}`), ".danger-solid", "copy-confirm", () =>
        confirm.kind === "cli"
          ? stopCopy(host, confirm, run, (hid, msg) => S.localErr.set(hid, msg), renderAll)
          : reloadCopy(host, confirm.kind)),
    ),
  );
}

function renderCards() {
  const box = $("host-cards");
  if (!S.data.hosts.length) {
    box.replaceChildren(
      h(
        "div.cards-empty",
        {},
        h("strong", {}, L("没有检测到可以当 boss 的应用", "No boss apps detected")),
        L("安装下面任意一个后点“刷新检测”。", "Install any of the apps below and click “Refresh detection”."),
      ),
    );
    return;
  }
  const hosts = orderedHosts();
  const current = selectedHost();
  box.replaceChildren(
    h("div.cli-card.members-split", {},
      h("div.members-list-pane", {},
        h("div.members-list-items", { role: "group", "aria-label": L("boss 列表", "Bosses") },
          ...hosts.map((host) => listRow(host, host.id === current.id)))),
      h("div.members-detail-pane", {}, detailPanel(current))));
}

// Below the list: supported bosses not installed here, and a prompt any other MCP agent can follow.
function renderOthers() {
  const absent = S.data.absent || [];
  const { launcher, skills } = S.data.service;
  // A service too old to carry the Skills leaves that step out.
  const skillStep = skills ? L(
    `然后把 ${skills} 里的 agentnave-manager 文件夹（有 agentnave-handover 的话也带上）整个复制到你的用户级 Skill 目录；你不支持 Skill 的话，改为每次用 AgentNave 前先读 ${skills}/agentnave-manager/SKILL.md。`,
    ` Then copy the agentnave-manager folder (and agentnave-handover, if present) from ${skills} into your user-level Skill directory; if you do not support Skills, read ${skills}/agentnave-manager/SKILL.md before each use of AgentNave instead.`,
  ) : "";
  const prompt = L(
    `请把 AgentNave 接入你自己：在你的用户级（全局）MCP 配置里加一个名为 agentnave 的本机 STDIO 服务，command 为 ${launcher}，不带参数和环境变量。${skillStep}写入前备份原文件，其他内容原样保留；完成后告诉我改了哪些文件，以及要不要重开会话才能生效。`,
    `Connect AgentNave to yourself: add a local STDIO server named agentnave to your user-level (global) MCP configuration, with command ${launcher} and no arguments or environment variables.${skillStep} Back up each file before writing and leave everything else as it is; then tell me which files you changed and whether I need to restart the session for it to take effect.`,
  );
  const copy = copyButton(prompt, L("复制接入提示词", "Copy setup prompt"));
  copy.setAttribute("aria-label", L("复制接入提示词", "Copy setup prompt")); // not the whole prompt
  copy.disabled = !launcher;
  copy.className = "";
  const toggle = h("button.cli-missing-toggle", { type: "button", "data-act": "others", "aria-expanded": String(S.othersOpen) },
    h("span.cli-chevron", { "aria-hidden": "true" }),
    absent.length ? L(`未安装的 boss（${absent.length}）`, `Uninstalled bosses (${absent.length})`) : L("其他 boss", "Other bosses"));
  toggle.addEventListener("click", () => {
    S.othersOpen = !S.othersOpen;
    S.focus = { act: "others" };
    renderAll();
  });
  $("host-others").replaceChildren(
    h("div.cli-missing", {}, toggle,
      S.othersOpen && h("div.host-others-list", {},
        absent.length > 0 && h("div.member-row.host-row", {},
          h("span.member-scene", {}, L("未安装", "Not installed")),
          h("div.member-line.others-list", {}, ...absent.map((x) => h("span.others-item", {}, h("span.others-icon", {}, AN.hostTile(x.mark, x.label)), x.label)))),
        h("div.member-row.host-row", {},
          h("span.member-scene", {}, L("其他支持 MCP 的 Agent", "Other MCP agents")),
          h("div.member-line", {},
            !launcher && h("span.fine", {}, S.install.busy ? L("正在安装本机服务…", "Installing local service…") : L("需先安装本机服务", "Install local service first")),
            copy)))));
}

function renderAll() {
  if (!S.data) return;
  window.dispatchEvent(new CustomEvent("agentnave:hosts", { detail: S.data }));
  renderBanner();
  renderCallout();
  renderSummary();
  renderBackups();
  renderCards();
  renderOthers();
  if (S.focus) {
    const { id, act } = S.focus;
    S.focus = null;
    const within = id ? `[data-host="${id}"]` : "#host-others";
    document
      .querySelector(`${within}[data-act="${act}"], ${within} [data-act="${act}"]`)
      ?.focus({ preventScroll: true });
  }
}

$("refresh-hosts").addEventListener("click", refreshAll);
$("reload-notice").querySelector?.("a")?.addEventListener("click", (e) => {
  e.preventDefault();
  showHost(reloadPending);
});
setInterval(() => {
  const hidden = AN.isHidden ? AN.isHidden() : document.hidden;
  if (!hidden && !$("pane-hosts").hidden && !S.refreshing) reload();
}, 10000);
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
// ---------------------------------------------------------------- boot
(async function boot(attempt = 0) {
  if (!$("pane-hosts").hidden) say(L("正在读取接入状态…", "Loading connection status…"));
  S.seen = loadSeen();
  S.firstRun = S.seen.size === 0;
  try {
    const [hosts, status, roles] = await Promise.all([
      load("/app/hosts"),
      load("/app/status"),
      loadHostCli(),
    ]);
    S.hostCli = roles;
    S.appVersion = status.version || "";
    S.install.busy = status.service?.state === "installing";
    S.install.error = status.service?.state === "failed" ? status.service.error : null;
    setData(hosts);
    if (!$("pane-hosts").hidden) say("");
  } catch {
    // The tray answers 503 until its first host detection finishes; retry briefly before giving up.
    if (attempt < 10) { setTimeout(() => boot(attempt + 1), 1000); return; }
    say(L("无法读取接入状态，请稍后点“刷新检测”重试。", "Failed to load connection status; click “Refresh detection” to retry."), true);
    $("hosts-summary").replaceChildren(h("span.dim", {}, L("读取失败", "Load failed")));
  }
})();

}
