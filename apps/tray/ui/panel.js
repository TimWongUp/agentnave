"use strict";
// Menu bar panel: what runs now, what waits for review, what changed lately. Links go to
// workbench routes; the tray app opens them in the main window instead of navigating here.

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
  blocked: ["需处理", "bad"],
  cancelled: ["已取消", "idle"],
  interrupted: ["已中断", "bad"],
};
const RECENT = 6;
const LIVE = 4;

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
// The newest line of a reply, stripped of Markdown punctuation, for a one-line preview.
function tail(text) {
  const lines = String(text || "")
    .split("\n")
    .map((line) => line.replace(/[#>*`_|-]+/g, " ").trim())
    .filter(Boolean);
  return lines.length ? lines[lines.length - 1] : "";
}

async function load(path) {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) throw new Error(String(response.status));
  return response.json();
}

// Speakers of one running or reviewed conversation, from its director view.
function voices(detail) {
  if (!detail) return [];
  if (detail.mode === "task") {
    return (detail.tasks || [])
      .filter((task) => task.status === "running")
      .map((task) => ({
        who: task.provider,
        provider: task.provider,
        model: task.model,
        text: tail(task.output),
        running: true,
      }));
  }
  const seats = Object.fromEntries(
    (detail.seats || []).map((seat) => [seat.id, seat]),
  );
  return (detail.round?.seats || [])
    .filter((run) => run.status === "running" || run.review === "pending")
    .map((run) => {
      const seat = seats[run.seat_id] || {};
      return {
        who: seat.label || run.seat_id,
        provider: seat.provider,
        model: seat.model,
        text: run.status === "running" ? tail(run.output) : "回答待审核",
        running: run.status === "running",
      };
    });
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
  const shows = (home.shows || []).filter((show) => !show.archived);
  const running = shows.filter((show) => statusOf(show)[1] === "run");
  const review = shows.filter((show) => statusOf(show)[1] === "review");
  const trouble = shows.filter((show) => statusOf(show)[1] === "bad");
  const hot = [...running, ...review].slice(0, LIVE);
  const details = await Promise.all(
    hot.map((show) => load(link(show) + "state").catch(() => null)),
  );
  const key = JSON.stringify([home.shows, details]);
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

  const blocks = [];
  if (hot.length) {
    blocks.push(h("p.label", {}, "正在发生"));
    hot.forEach((show, index) => {
      const [text, tone] = statusOf(show);
      const detail = details[index];
      const round = detail?.round ? `第 ${detail.round.number} 轮 · ` : "";
      blocks.push(
        h(
          "a.live",
          { href: link(show) },
          h(
            "div.live-head",
            {},
            glyph(show.mode),
            h("span.title", {}, show.title),
            h("small", {}, round + text),
            dot(tone),
          ),
          voices(detail)
            .slice(0, 4)
            .map((voice) =>
              h(
                "div.voice",
                {},
                avatar(voice.provider, voice.model),
                h("span.who", {}, voice.who),
                voice.running && !voice.text
                  ? typing()
                  : h("span.tail", {}, voice.text || "…"),
              ),
            ),
        ),
      );
    });
  }
  if (trouble.length) {
    blocks.push(h("p.label", {}, "需要处理"));
    trouble.forEach((show) => blocks.push(row(show)));
  }
  const recent = shows
    .filter((show) => !hot.includes(show) && !trouble.includes(show))
    .slice(0, RECENT);
  if (recent.length) {
    blocks.push(h("p.label", {}, "最近"));
    recent.forEach((show) => blocks.push(row(show)));
  }
  if (!blocks.length)
    blocks.push(
      h(
        "div.calm",
        {},
        h("strong", {}, "还没有会话"),
        "在主控 Agent 的对话中开始一档节目或一个任务。",
      ),
    );
  $("list").replaceChildren(...blocks);
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
