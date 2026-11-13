"use strict";
{
"use strict";
// Settings -> 宿主接入. Pure DOM construction (h() = createElement/textContent), no innerHTML, no inline style.
// Data comes from GET /app/hosts; every write is a POST with X-AgentNave-Action (AN.post). The page never
// writes on its own: auto-takeover is the server's job at startup and on 刷新检测.
const { h, $, load, post, installService, relTime, L } = AN;


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
  copyConfirm: null, // {id, kind} of the background copy whose stop/restart is being confirmed
  adv: new Set(), // host ids whose 高级 panel is open
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
  refreshResult: "",
};

// ---------------------------------------------------------------- small helpers
const say = (text, bad = false) => {
  $("notice").textContent = text;
  $("notice").classList.toggle("error", bad);
};
const shortPath = (p) =>
  String(p || "").replace(/^(\/Users|\/home)\/[^/]+/, "~");
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
    && (host.reload.copies || []).some((copy) => copy.kind === pending.kind) && copyAction(host, pending.kind)))
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
    const next = await load("/app/hosts");
    if (!S.busy.size && !S.confirm && !S.copyConfirm && !document.activeElement?.closest(".hc-adv") && JSON.stringify(next) !== JSON.stringify(S.data)) setData(next);
  } catch {
    /* keep what is shown */
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
      : L(`${names}${untested ? `；其中 ${untested} 个未实测，需在各自卡片上单独接入` : ""}`, `${names}${untested ? `; ${untested} untested (connect individually on each card)` : ""}`);
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
      L(`${attention} 需要处理 ↓`, `${attention} needs attention ↓`),
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
  const reloads = hosts.filter((x) => ["required", "unverified"].includes(x.reload?.state)).length;
  if (reloads) {
    const b = h("button.sum-chip", { type: "button" }, L(`${reloads} 待生效 ↓`, `${reloads} pending reload ↓`));
    b.addEventListener("click", () => {
      const card = [...document.querySelectorAll(".hc")].find((c) => c.querySelector(".hc-reload:not([data-fault])"));
      card?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
    kids.push(b);
  }
  const faults = hosts.filter((x) => x.fault).length;
  if (faults) {
    const b = h("button.sum-chip", { type: "button" }, L(`${faults} 连接中断 ↓`, `${faults} connection lost ↓`));
    b.addEventListener("click", () => {
      const card = [...document.querySelectorAll(".hc")].find((c) => c.querySelector("[data-fault]"));
      card?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
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

// ---------------------------------------------------------------- host card
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

function advPanel(host) {
  const id = host.id;
  const busy = S.busy.has(id);
  // The root is detected; a path typed in an earlier version can only be dropped again.
  const saveRoot = (path) => run(id, "root", () => post("/app/hosts/root", { id, path })).then((r) => { if (r?.ok) renderAll(); });
  const rootEditor = h("div.root-editor", {},
    h("span", {}, L("配置根目录", "Config root")), pathRow(host.root.path),
    h("span.fine", {}, L(`来源：${({ user: "之前手动指定", env: "App 环境变量", shell: "登录 shell", default: "默认目录" })[host.root.source] || host.root.source}`, `Source: ${({ user: "Manual (legacy)", env: "App environment", shell: "Login shell", default: "Default directory" })[host.root.source] || host.root.source}`)),
    host.root.source === "user" && h("div.adv-actions", {}, button(L("恢复自动", "Reset to auto"), "", "root-auto", () => saveRoot(null), busy)));
  const kv = [
    [
      L("命令", "Command"),
      host.command ? pathRow(host.command) : h("span.dim", {}, L("尚未写入", "Not written yet")),
    ],
    [
      "AGENTNAVE_HOST",
      host.has_host_env
        ? h("span", {}, L(`已设置（${host.boss}）`, `Set (${host.boss})`))
        : h(
            "span.dim",
            {},
            host.status === "available" ? L("接入时写入", "Written on connect") : L("未设置", "Not set"),
          ),
    ],
    [
      L("Skill 目录", "Skill directory"),
      host.skill.path
        ? pathRow(host.skill.path)
        : h("span.dim", {}, L("未确定，无法自动安装", "Undetermined; cannot auto-install")),
    ],
    [L("检测来源", "Detected via"), h("span", {}, host.detect_source === "app" ? L(`应用：${host.app_path || "已安装"}`, `App: ${host.app_path || "Installed"}`) : sourceText(host.detect_source))],
  ];
  return h(
    "div.hc-adv",
    { id: `adv-${id}` },
    rootEditor,
    h("div.fine", {}, (host.layers || []).filter((l) => l.has_entry).map((l) => L(`${l.scope === "project" ? "项目（只读）" : "用户层"}：${shortPath(l.path)}`, `${l.scope === "project" ? "Project (read-only)" : "User"}: ${shortPath(l.path)}`)).join(L("；", "; "))),
    h(
      "dl.adv-kv",
      {},
      kv.map(([k, v]) => [h("dt", {}, k), h("dd", {}, v)]),
    ),
  );
}

function confirmBlock(host) {
  const orphan = host.status === "orphaned";
  const skill = ["app", "external"].includes(host.skill.state) ? L("Skill 会一并删除", "Skill will also be removed") : "";
  const paths = (host.layers || []).filter((l) => l.scope !== "project" && l.has_entry).map((l) => shortPath(l.path));
  const where = L(`${(paths.length ? paths : [shortPath(host.config_path)]).join("、")} 里名为 agentnave 的条目`, `the agentnave entry in ${(paths.length ? paths : [shortPath(host.config_path)]).join(", ")}`);
  const msg = orphan
    ? L(`将删除 ${where}（写入前会备份）。${host.label} 本身不会受影响${skill ? `；${skill}` : ""}。`, `Will remove ${where} (backed up before writing). ${host.label} itself is unaffected${skill ? `; ${skill}` : ""}.`)
    : L(`只删除 ${where}（写入前会备份），其他配置不动${skill ? `；${skill}` : ""}。之后可以随时重新接入。`, `Only removes ${where} (backed up before writing); other settings remain untouched${skill ? `; ${skill}` : ""}. You can reconnect anytime.`);
  return h(
    "div.hc-confirm",
    { role: "group", "aria-label": L(`确认移除 ${host.label}`, `Confirm removing ${host.label}`) },
    h(
      "div.msg",
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
      "div.acts",
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

// The row's own buttons: what can be done to this host now.
function actionButtons(host) {
  const id = host.id;
  const busy = S.busy.get(id);
  const svcOk = S.data.service.installed;
  const dis = !!busy;
  const askRemove = (label) =>
    busy === "remove"
      ? spinBtn(L("移除中…", "Removing…"))
      : button(label, ".danger", "remove", () => {
          S.confirm = id;
          S.copyConfirm = null;
          S.focus = { id, act: "cancel" };
          renderAll();
        }, dis);
  const out = [];
  if (host.status === "available") {
    if (!svcOk) out.push(h("span.fine", {}, S.install.busy ? L("正在安装本机服务…", "Installing local service…") : L("需先安装本机服务", "Install local service first")));
    out.push(busy === "connect"
      ? spinBtn(L("接入中…", "Connecting…"), ".primary")
      : button(L("接入", "Connect"), ".primary", "connect", () => connect(host), dis || !svcOk || S.batch));
  } else if (host.status === "needs_attention") {
    out.push(askRemove(L("移除", "Remove")));
    out.push(busy === "connect"
      ? spinBtn(L("重试中…", "Retrying…"), ".primary")
      : button(L("重试接入", "Retry"), ".primary", "connect", () => connect(host), dis || !svcOk));
  } else {
    // A connected host whose entry names another launcher is still writable: the same connect
    // path rewrites just the command and keeps the rest of the entry.
    if (host.status === "connected" && host.launcher?.stale && svcOk)
      out.push(busy === "connect"
        ? spinBtn(L("切换中…", "Switching…"), ".primary")
        : button(L("改用新启动器", "Use new launcher"), ".primary", "connect", () => connect(host), dis));
    out.push(askRemove(host.status === "connected" ? L("移除", "Remove") : L("移除条目", "Remove entry")));
  }
  return out;
}

// One line under the name: the state, then what matters most about it.
function subLine(host, word) {
  const bits = [h("span.hc-status", {}, h("i.dot", { "aria-hidden": "true" }), word)];
  const readers = [...new Set((host.clients || []).map((client) => client.kind))];
  if (readers.length) bits.push(h("span.hc-fact", {}, L("发现 ", "Found ") + readers.map((kind) => kind === "desktop" ? L("桌面端", "Desktop") : "CLI").join(" · ")));
  if (host.status === "connected") {
    // A Skill is only mentioned when it could not be installed.
    const s = host.skill;
    const current = s.state === "app" && (!S.appVersion || s.version === S.appVersion);
    if (!current) bits.push(h("span.hc-fact.warn-text", {}, ["blocked", "manual"].includes(s.state) ? L("Skill 无法安装", "Skill cannot be installed") : L("Skill 未能安装", "Skill not installed")));
  } else if (host.status === "available") {
    bits.push(h("span.hc-fact", {}, L("本机已安装，还没有接入 AgentNave", "Installed locally, not connected to AgentNave yet")));
  }
  return h("p.hc-sub", {}, ...bits);
}

function buildCard(host) {
  const id = host.id;
  const [word, tone] = STATUS[host.status] || [host.status, "idle"];
  const busy = S.busy.get(id);
  const orphan = host.status === "orphaned";
  const isNew = !S.firstRun && S.newIds.has(id);
  const open = S.adv.has(id);
  const card = h(
    "article.hc.tone-" + tone + (busy ? ".is-busy" : "") + (orphan ? ".is-orphan" : ""),
    { "data-host": id, "aria-labelledby": `name-${id}`, "aria-busy": busy ? "true" : null },
  );
  card.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && S.confirm === id) {
      S.confirm = null;
      S.focus = { id, act: "remove" };
      renderAll();
    } else if (e.key === "Escape" && S.copyConfirm?.id === id) {
      S.focus = { id, act: `copy-${S.copyConfirm.kind}` };
      S.copyConfirm = null;
      renderAll();
    }
  });
  // Only a host not yet connected can be untested: once its entry works it has been tried.
  const badges = [
    host.reload?.state === "required" && h("span.badge.badge-warning", {}, L("需要重新加载", "Reload required")),
    host.reload?.state === "unverified" && h("span.badge", {}, L("待确认生效", "Pending verification")),
    host.fault && h("span.badge.badge-warning", {}, L("连接已中断", "Connection lost")),
    !host.tested && host.status === "available" &&
      h("span.badge.badge-muted", { title: L("还没有经过真实接入验证：写入逻辑按官方文档实现，遇到问题请反馈。", "Not verified with a live connection yet: implemented per official docs; please report any issues.") }, L("未实测", "Untested")),
    isNew && h("span.badge.badge-accent", {}, L("新检测到", "New")),
  ];
  const details = h(
    "button.adv-btn",
    { type: "button", "data-act": "adv", "aria-expanded": String(open), "aria-controls": `details-${id}` },
    h("i.chev", { "aria-hidden": "true" }),
    L("详情", "Details"),
  );
  details.addEventListener("click", () => {
    if (open) S.adv.delete(id);
    else S.adv.add(id);
    S.focus = { id, act: "adv" };
    renderAll();
  });
  const main = [
    h(
      "div.hc-line",
      {},
      h("span.hc-icon", { "aria-hidden": "true" }, AN.hostTile(host.mark || id, host.label)),
      h("div.hc-id", {},
        h("div.hc-title", {}, h("h3.hc-name", { id: `name-${id}` }, host.label), ...badges),
        subLine(host, word)),
      h("div.hc-acts", {}, ...(S.confirm === id && !busy ? [] : actionButtons(host)), details),
    ),
  ];
  if (orphan)
    main.push(h("p.hc-orphan", {}, L(`本机已检测不到 ${host.label}，但它的配置里还留着 agentnave 条目。留着不影响使用；不再需要时可以移除。`, `${host.label} is no longer detected locally, but its config still has the agentnave entry. Leaving it is harmless; remove it if no longer needed.`)));
  if (host.shadowed_by)
    main.push(h("div.hc-note.info", {}, L("项目级配置 ", "Project config "), h("code", {}, shortPath(host.shadowed_by)), L(" 里也有 agentnave 条目，在那个项目里会优先使用它。App 只看不改项目级配置。", " also has an agentnave entry, which takes precedence in that project. The App never modifies project configs.")));
  if (host.launcher?.stale)
    main.push(h("div.hc-note.info", {},
      L("配置里的启动器仍是旧的 ", "Configured launcher is still the old "), h("code", {}, shortPath(host.launcher.current || "")),
      L("；重新接入会把 command 改为 ", "; reconnecting updates command to "), h("code", {}, shortPath(host.launcher.expected || ""))));
  const local = S.localErr.get(id);
  const last = host.last_action;
  if (local) main.push(h("div.hc-note.error", { role: "alert" }, local));
  else if (last && !last.ok)
    main.push(h("div.hc-note.error", { role: "alert" }, last.message,
      h("small", {}, `${relTime(last.at)}${host.status === "needs_attention" ? L(" · 修好后点“重试接入”", " · Click “Retry” after fixing") : ""}`)));
  else if (host.attention) main.push(h("div.hc-note.error", { role: "alert" }, host.attention.message));
  else if (last && open)
    main.push(h("p.hc-last", {}, h("span.tick", { "aria-hidden": "true" }, "✓"), `${last.message} · ${relTime(last.at)}`));
  if (host.fault) main.push(faultNote(host));
  // Configuration changes remain visible until the host stops or a new connection is observed.
  if (["required", "unverified"].includes(host.reload?.state)) {
    main.push(reloadNote(host));
    const pending = S.copyConfirm?.id === id && !busy && copyAction(host, S.copyConfirm.kind);
    if (pending) main.push(copyConfirmBlock(host, S.copyConfirm.kind, pending));
  }
  if (S.confirm === id && !busy) main.push(confirmBlock(host));
  // The 详情 button controls this wrapper, which every status renders (an orphan has no advanced part).
  if (open)
    main.push(h("div.hc-details", { id: `details-${id}` },
      h("dl.hc-facts", {},
        h("div", {}, h("dt", {}, L("入口", "Entry")), h("dd", {}, entryNode(host))),
        h("div", {}, h("dt", {}, "Skill"), h("dd", {}, skillNode(host))),
        h("div.wide", {}, h("dt", {}, host.status === "available" ? L("配置文件（接入时写入）", "Config file (written on connect)") : L("配置文件", "Config file")), h("dd", {}, pathRow(host.config_path))),
      ),
      !orphan && advPanel(host)));
  card.append(h("div.hc-main", {}, ...main));
  return card;
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
const copyAction = (host, kind) =>
  (host.copy_actions || []).includes(kind) && COPY_ACTIONS[`${host.boss}/${kind}`];
const bundleName = (path) => String(path || "").split("/").pop().replace(/\.app$/i, "");

// One row per old copy still holding the previous config: what it is, since when, how to reopen it.
function copyRow(host, copy) {
  const id = host.id;
  const busy = S.busy.get(id);
  const action = copyAction(host, copy.kind);
  const act = `copy-${copy.kind}`;
  const name = [COPY_KIND[copy.kind] || copy.kind, copy.bundle && bundleName(copy.bundle)].filter(Boolean).join(" · ");
  const since = copy.since ? relTime(new Date(copy.since).toISOString()) : "";
  let control = null;
  if (action)
    control = busy === act
      ? spinBtn(action[1])
      : button(action[0], "", act, () => {
          S.copyConfirm = { id, kind: copy.kind };
          S.confirm = null;
          S.focus = { id, act: "copy-cancel" };
          renderAll();
        }, Boolean(busy) || S.updating || S.copyConfirm?.id === id);
  return h("li.hc-copy", {},
    h("div", {},
      h("strong", {}, name, ["daemon", "gateway"].includes(copy.kind) &&
        AN.settings.settingTip(L("这是应用在后台常驻的进程，关掉窗口也不会退出；它重启后才会用上新配置。", "This is the app’s resident background process; closing the window doesn’t stop it, and it only picks up the new config after a restart."))),
      since && h("span.hc-since", {}, L(` · ${since}${/\d$/.test(since) ? " " : ""}启动`, ` · started ${since}`)),
      host.reopen?.[copy.kind] && h("p", {}, host.reopen[copy.kind])),
    h("div.hc-copy-act", {}, control));
}

// The service died while the host was still connected; only the user's confirmation clears it.
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
    copies.length
      ? h("ul.hc-copies", {}, copies.map((copy) => copyRow(host, copy)))
      : fallback && h("p.hc-reopen", {}, fallback),
    host.trust && h("p.hc-trust", {}, host.trust),
  );
}

function copyConfirmBlock(host, kind, [label, , title, consequence]) {
  const cancel = () => {
    S.copyConfirm = null;
    S.focus = { id: host.id, act: `copy-${kind}` };
    renderAll();
  };
  return h(
    "div.hc-confirm",
    { role: "group", "aria-label": title },
    h("div.msg", {}, h("strong", {}, title), consequence),
    h("div.acts", {},
      button(L("取消", "Cancel"), "", "copy-cancel", cancel),
      button(L(`确认${label}`, `Confirm ${label.toLowerCase()}`), ".danger-solid", "copy-confirm", () => reloadCopy(host, kind)),
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
  // Connected bosses first; the rest keep their order below them.
  const chosen = (host) => (host.layers || []).some((l) => l.has_entry && l.scope !== "project");
  box.replaceChildren(...[...S.data.hosts].sort((a, b) => chosen(b) - chosen(a)).map(buildCard));
}

// Below the cards: supported bosses not installed here, and a prompt any other MCP agent can follow.
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
  $("host-others").replaceChildren(
    absent.length && h("div.others-row", {},
      h("span.others-name", {}, L(`未安装（${absent.length}）`, `Not installed (${absent.length})`)),
      h("span.others-list", {}, ...absent.map((x) => h("span.others-item", {}, h("span.others-icon", {}, AN.hostTile(x.mark, x.label)), x.label)))),
    h("div.others-row", {},
      h("span.others-name", {}, L("其他支持 MCP 的 Agent", "Other MCP agents")),
      h("span.others-acts", {},
        !launcher && h("span.fine", {}, S.install.busy ? L("正在安装本机服务…", "Installing local service…") : L("需先安装本机服务", "Install local service first")),
        copy)),
  );
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
    document
      .querySelector(`.hc[data-host="${id}"] [data-act="${act}"]`)
      ?.focus({ preventScroll: true });
  }
}

$("refresh-hosts").addEventListener("click", refreshAll);
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
    const [hosts, status] = await Promise.all([
      load("/app/hosts"),
      load("/app/status"),
    ]);
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
