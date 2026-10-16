"use strict";
// Menu bar panel: a live group chat of recent work — what the host Agent sent, what each
// model is writing or answered — plus update notices. Links go to workbench routes; the tray
// app opens them in the main window instead of navigating here.

const ROUND = {
  idle: ["等待开场", "idle"],
  running: ["进行中", "run"],
  review: ["待审核", "review"],
  failed: ["失败", "bad"],
  finished: ["已结束", "done"],
  cancelled: ["已取消", "idle"],
  interrupted: ["中断", "bad"],
};
const TASK = {
  idle: ["无任务", "idle"],
  running: ["进行中", "run"],
  succeeded: ["已回复", "done"],
  failed: ["失败", "bad"],
  blocked: ["受阻", "bad"],
  cancelled: ["已取消", "idle"],
  interrupted: ["已中断", "bad"],
};
const RECENT = 6;
// The chat follows conversations that run, wait for review or changed within WINDOW.
const WINDOW = 2 * 60 * 60 * 1000;
const THREADS = 8;
const MESSAGES = 40;

if (new URLSearchParams(location.search).has("glass"))
  document.body.classList.add("glass");

const $ = (id) => document.getElementById(id);
function h(tag, props, ...kids) {
  const [name, ...classes] = tag.split(".");
  const node = document.createElement(name || "div");
  if (classes.length) node.className = classes.join(" ");
  for (const [key, value] of Object.entries(props || {})) {
    if (value == null) continue;
    if (key === "style") node.style.cssText = value; // CSSOM, which the CSP allows
    else node.setAttribute(key, value);
  }
  node.append(
    ...kids.flat(Infinity).filter((kid) => kid != null && kid !== false),
  );
  return node;
}
const statusOf = (show) =>
  (show.mode === "task" ? TASK : ROUND)[show.status] || [
    show.status || "—",
    "idle",
  ];
const glyph = (mode) =>
  h("span.glyph." + mode, {}, mode === "task" ? "任" : "节");
const dot = (kind) => h("i.dot.tone-" + kind);
const typing = () => h("span.typing", {}, h("i"), h("i"), h("i"));
const link = (show) => show.director_url; // "/c/<id>/" in the tray workbench

function avatar(provider, model) {
  const key = `${model || ""} ${provider || ""}`.toLowerCase();
  const known =
    ["claude", "gpt", "gemini", "grok", "deepseek"].find((name) =>
      key.includes(name),
    ) || { codex: "gpt", antigravity: "gemini" }[provider];
  if (known) return h("span.avatar.av-" + known);
  let hue = 0;
  for (const char of provider || "?")
    hue = (hue * 31 + char.charCodeAt(0)) % 360;
  return h(
    "span.avatar.av-plain",
    { style: `--hue:${hue}` },
    (provider || "?").slice(0, 1).toUpperCase(),
  );
}
// Reply text for a chat bubble: Markdown punctuation dropped, blank runs collapsed.
function plain(text) {
  return String(text || "")
    .split("\n")
    .filter((line) => !/^\s*```/.test(line))
    .map((line) =>
      line
        .replace(/^\s{0,3}(#{1,6}|>)\s*/, "")
        .replace(/(\*\*|__|`)/g, "")
        .trimEnd(),
    )
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}
// The newest lines of a reply still being written.
const latest = (text, lines = 3) =>
  plain(text).split("\n").filter(Boolean).slice(-lines).join("\n");

async function load(path) {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) throw new Error(String(response.status));
  return response.json();
}

const HOST = { who: "主控", host: true };
const at = (iso) => {
  const time = Date.parse(iso || "");
  return Number.isNaN(time) ? 0 : time;
};

// One conversation's messages, from its director view, in the order they happened.
function chat(show, detail) {
  if (!detail) return [];
  const thread = { href: link(show), title: show.title, mode: show.mode };
  const now = Date.now();
  const out = [];
  const say = (time, voice, text, extra) =>
    out.push({ ...thread, time, ...voice, text, ...extra });
  if (detail.mode === "task") {
    for (const task of detail.tasks || []) {
      const start = at(task.time);
      const end = start + (task.elapsed_ms || 0);
      const voice = {
        who: task.provider,
        provider: task.provider,
        model: task.model,
      };
      if (task.prompt) say(start, HOST, task.prompt, { to: task.provider });
      if (task.status === "running")
        say(now, voice, latest(task.output), { typing: true });
      else if (task.output) say(end, voice, task.output);
      if (task.status !== "running" && task.status !== "succeeded") {
        const [label, tone] = TASK[task.status] || [task.status, "idle"];
        say(end, { system: true, tone }, `${task.provider} ${label}`, {
          detail: task.error,
        });
      }
    }
    return out;
  }
  const seats = Object.fromEntries(
    (detail.seats || []).map((seat) => [
      seat.id,
      {
        who: seat.label || seat.id,
        provider: seat.provider,
        model: seat.model,
      },
    ]),
  );
  for (const message of detail.messages || []) {
    const time = at(message.time);
    if (message.kind === "absent")
      say(
        time,
        { system: true, tone: "bad" },
        `${message.speaker} ${message.text}`,
      );
    else if (message.speaker === "@host")
      say(time, HOST, message.text, {
        to: message.kind === "task" ? `第 ${message.round} 轮` : null,
      });
    else
      say(
        time,
        seats[message.speaker] || { who: message.speaker },
        message.text,
      );
  }
  for (const run of detail.round?.seats || []) {
    const voice = seats[run.seat_id] || { who: run.seat_id };
    if (run.status === "running")
      say(now, voice, latest(run.output), { typing: true });
    else if (run.review === "pending")
      say(now, voice, run.output, { badge: "待审核" });
  }
  return out;
}

// Notices about this app's own update and an outdated local service, above the list.
function notices(status) {
  const blocks = [];
  const update = status?.update;
  if (update?.state === "available")
    blocks.push(
      notice(
        `AgentNave v${update.version} 可用`,
        `当前 v${status.version}，更新后自动重启`,
        "更新并重启",
        "/app/update",
      ),
    );
  else if (update?.state === "installing")
    blocks.push(notice("正在下载更新", "完成后自动重启"));
  else if (update?.state === "failed")
    blocks.push(notice("更新失败", "检查网络后重试", "重试", "/app/update"));
  const service = status?.service;
  if (service)
    blocks.push(
      notice(
        `本机服务 v${service.version} 比托盘 v${status.version} 旧`,
        "在终端运行升级命令，再让各宿主重连 MCP",
        "复制命令",
        "/app/copy-service-command",
      ),
    );
  return blocks;
}
function notice(title, detail, action, href) {
  const button = action && h("a.notice-action", { href }, action);
  if (button && href === "/app/copy-service-command")
    button.addEventListener("click", () => {
      button.textContent = "已复制";
      setTimeout(() => (button.textContent = action), 2000);
    });
  return h(
    "div.notice",
    {},
    h("div.notice-text", {}, h("strong", {}, title), h("small", {}, detail)),
    button,
  );
}

let last = "";
async function refresh() {
  let home;
  try {
    home = await load("/state");
  } catch {
    $("summary").textContent = "无法读取工作室数据";
    return;
  }
  const status = await load("/app/status").catch(() => null);
  const shows = (home.shows || []).filter((show) => !show.archived);
  const running = shows.filter((show) => statusOf(show)[1] === "run");
  const review = shows.filter((show) => statusOf(show)[1] === "review");
  // Shows arrive most recently changed first.
  const threads = shows
    .filter(
      (show) =>
        ["run", "review"].includes(statusOf(show)[1]) ||
        Date.now() - at(show.updated) < WINDOW,
    )
    .slice(0, THREADS);
  const details = await Promise.all(
    threads.map((show) => load(link(show) + "state").catch(() => null)),
  );
  const key = JSON.stringify([home.shows, details, status]);
  if (key === last) return;
  last = key;

  const state = $("state");
  const [label, kind] = running.length
    ? [`${running.length} 运行中`, "run"]
    : review.length
      ? [`${review.length} 待审核`, "review"]
      : ["空闲", "idle"];
  state.className = "state tone-" + kind;
  state.replaceChildren(dot(kind), label);
  const hour = new Date().getHours();
  const greeting =
    hour < 6
      ? "夜深了"
      : hour < 12
        ? "早上好"
        : hour < 18
          ? "下午好"
          : "晚上好";
  $("summary").textContent =
    running.length || review.length
      ? `${greeting} · ${[running.length && `${running.length} 个会话正在进行`, review.length && `${review.length} 个等你审核`].filter(Boolean).join("，")}`
      : `${greeting} · 眼下没有进行中的工作`;

  const messages = threads
    .flatMap((show, index) => chat(show, details[index]))
    .sort((a, b) => a.time - b.time)
    .slice(-MESSAGES);
  const blocks = notices(status);
  let thread = null;
  for (const message of messages) {
    if (message.href !== thread) {
      thread = message.href;
      blocks.push(
        h(
          "a.thread",
          { href: message.href },
          glyph(message.mode),
          h("span", {}, message.title),
        ),
      );
    }
    blocks.push(bubble(message));
  }
  if (!messages.length) {
    blocks.push(
      h(
        "div.calm",
        {},
        h("strong", {}, shows.length ? "最近两小时没有动静" : "还没有会话"),
        shows.length
          ? "主控派出任务或开始节目后，对话会实时出现在这里。"
          : "在主控 Agent 的对话中开始一档节目或一个任务。",
      ),
    );
    if (shows.length) {
      blocks.push(h("p.label", {}, "最近"));
      shows.slice(0, RECENT).forEach((show) => blocks.push(row(show)));
    }
  }
  const list = $("list");
  const following =
    !list.childElementCount ||
    list.scrollHeight - list.scrollTop - list.clientHeight < 48;
  list.replaceChildren(...blocks);
  if (following) list.scrollTop = list.scrollHeight;
}

function clock(time) {
  if (!time) return "";
  const date = new Date(time);
  const hm = date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  });
  return date.toDateString() === new Date().toDateString()
    ? hm
    : `${date.getMonth() + 1}/${date.getDate()} ${hm}`;
}
function bubble(message) {
  if (message.system)
    return h(
      "a.sys.tone-" + message.tone,
      { href: message.href, title: message.detail },
      dot(message.tone),
      h("span", {}, message.text),
      message.detail && h("small", {}, message.detail),
    );
  const text = message.typing ? message.text : plain(message.text);
  return h(
    "a.msg" + (message.host ? ".host" : ""),
    { href: message.href },
    message.host
      ? h("span.avatar.av-host")
      : avatar(message.provider, message.model),
    h(
      "div.msg-body",
      {},
      h(
        "div.msg-meta",
        {},
        h("strong", {}, message.who),
        message.to && h("span", {}, "→ " + message.to),
        message.badge && h("span.badge", {}, message.badge),
        message.typing ? h("span.writing", {}, "正在输入") : null,
        h("time", {}, message.typing ? "" : clock(message.time)),
      ),
      text
        ? h("div.msg-text" + (message.typing ? ".typing-text" : ""), {}, text)
        : message.typing
          ? typing()
          : null,
    ),
  );
}
function row(show) {
  const [text, tone] = statusOf(show);
  return h(
    "a.row",
    { href: link(show) },
    glyph(show.mode),
    h("span.title", {}, show.title),
    h("small", {}, dot(tone), text),
  );
}

let timer = null,
  busy = false;
async function loop() {
  clearTimeout(timer);
  if (document.hidden || busy) return;
  busy = true;
  try {
    await refresh();
  } finally {
    busy = false;
    if (!document.hidden) timer = setTimeout(loop, 1500);
  }
}
document.addEventListener("visibilitychange", loop);
window.addEventListener("focus", loop);
loop();
