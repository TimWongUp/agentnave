"use strict";
// AgentNave workbench. Read-only: renders the `state` JSON of the workbench, director and
// public views; every action happens in the host Agent's conversation.

const ROUND = {
  idle: ["等待开场", "idle"],
  running: ["进行中", "run"],
  review: ["待审核", "review"],
  finished: ["本轮已结束", "done"],
  failed: ["本轮失败", "bad"],
  cancelled: ["已取消", "idle"],
  interrupted: ["上次中断", "bad"],
};
const TASK = {
  idle: ["尚无任务", "idle"],
  running: ["进行中", "run"],
  succeeded: ["已回复 · 待验收", "done"],
  failed: ["失败", "bad"],
  blocked: ["受阻", "bad"],
  cancelled: ["已取消", "idle"],
  interrupted: ["已中断", "bad"],
};
const SEAT = {
  pending: ["排队中", "idle"],
  running: ["作答中", "run"],
  succeeded: ["已作答", "done"],
  failed: ["失败", "bad"],
  blocked: ["受阻", "bad"],
  cancelled: ["已取消", "idle"],
  not_started: ["未开始", "idle"],
  interrupted: ["已中断", "bad"],
};
const REVIEW = {
  pending: ["待审核", "review"],
  publish: ["已发布", "done"],
  discard: ["已丢弃", "idle"],
};
const HOST = { id: "@host", label: "主持人", model: "", provider: "host" };
const EXAMPLES = [
  [
    "节目",
    "开一档节目，让 Codex、Grok 和 Pi 先并联回答：……，回答先给我审核再公开。",
  ],
  ["做任务", "让 CLI 完成任务：……，将结果放进同一个任务会话。"],
];
const TICK = 1500;

const app = {
  layout: "loading",
  home: null,
  homeUrl: null, // workbench state and its page URL
  detail: null,
  detailUrl: null, // selected conversation and its page URL
  mode: "all",
  query: "",
  tabs: {},
  speaker: null,
  open: new Set(),
  offline: false,
  keys: {},
  seen: null, // public messages already shown; null until the conversation first loads
};

// DOM ------------------------------------------------------------------------------------
const $ = (id) => document.getElementById(id);
function h(tag, props, ...kids) {
  const [base, id] = tag.split("#");
  const [name, ...classes] = base.split(".");
  const node = document.createElement(name || "div");
  if (classes.length) node.className = classes.join(" ");
  if (id) node.id = id;
  for (const [key, value] of Object.entries(props || {})) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (key === "class")
      node.className = (node.className + " " + value).trim();
    else if (key === "style") node.style.cssText = value;
    else node.setAttribute(key, value === true ? "" : value);
  }
  node.append(
    ...kids.flat(Infinity).filter((kid) => kid != null && kid !== false),
  );
  return node;
}
const tone = ([label, kind]) => ({ label, kind });
const pill = ({ label, kind }) =>
  h("span.pill.tone-" + kind, {}, h("i.dot.tone-" + kind), label);
const dot = (kind) => h("i.dot.tone-" + kind);
const statusOf = (mode, status) =>
  tone((mode === "task" ? TASK : ROUND)[status] || [status || "—", "idle"]);
// A failed or interrupted result is history the host already received, not a to-do.
const urgent = (kind) => kind === "run" || kind === "review";

function when(iso) {
  if (!iso) return "";
  const date = new Date(iso),
    now = new Date();
  const time = date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  });
  if (date.toDateString() === now.toDateString()) return time;
  return `${date.getMonth() + 1}月${date.getDate()}日 ${time}`;
}
function duration(ms) {
  const s = Math.max(0, Math.round((ms || 0) / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  return m < 60
    ? `${m} 分 ${s % 60} 秒`
    : `${Math.floor(m / 60)} 小时 ${m % 60} 分`;
}
function modelName(seat) {
  return seat.model || "未记录模型";
}
function identityNote(seat) {
  const host = seat.provider === "host";
  return [host ? "boss" : "组员", host ? seat.host : seat.provider].filter(Boolean).join(" · ");
}
function castOf(detail) {
  const lastManager = [...(detail.messages || [])].reverse().find((message) => message.speaker === "@host");
  return [{ ...HOST, host: detail.host, model: lastManager?.manager_model || "" }, ...detail.seats];
}
function avatar(seat, size = "") {
  if (seat.provider === "host" && !seat.model) return null;
  return Avatars.create("model", seat.provider === "host" ? seat.host : seat.provider,
    seat.model, size.replaceAll(".", " "));
}
function copyButton(text, label = "复制") {
  const button = h("button.icon-btn", { type: "button" }, label);
  button.onclick = async () => {
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "已复制";
    } catch {
      button.textContent = "请手动选中复制";
    }
    setTimeout(() => {
      button.textContent = label;
    }, 1600);
  };
  return button;
}

// Minimal Markdown: fences, headings, lists, quotes, inline code and bold. Builds DOM nodes
// only, so model output can never inject markup.
function inline(text) {
  const out = [],
    pattern = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\[[^\]\n]+\]\(https?:\/\/[^)\s]+\))/g;
  let last = 0,
    match;
  while ((match = pattern.exec(text))) {
    if (match.index > last) out.push(text.slice(last, match.index));
    const token = match[0];
    const link = token.match(/^\[([^\]]+)\]\((.+)\)$/);
    out.push(
      link
        ? h("a", { href: link[2], target: "_blank", rel: "noreferrer", title: link[2] }, link[1])
        : token[0] === "`"
          ? h("code", {}, token.slice(1, -1))
          : h("strong", {}, token.slice(2, -2)),
    );
    last = pattern.lastIndex;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
function md(text) {
  const root = h("div.md"),
    lines = String(text || "")
      .replace(/\r\n/g, "\n")
      .split("\n");
  let para = [],
    list = null;
  const flush = () => {
    if (para.length) root.append(h("p", {}, inline(para.join("\n"))));
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      flush();
      list = null;
      const code = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i]))
        code.push(lines[i]);
      root.append(h("pre", {}, h("code", {}, code.join("\n"))));
      continue;
    }
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    const item = line.match(/^\s*([-*+]|\d+[.)])\s+(.*)$/);
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      flush();
      list = null;
      root.append(h("hr"));
    } else if (heading) {
      flush();
      list = null;
      root.append(h("h4", {}, inline(heading[1])));
    } else if (item) {
      flush();
      const tag = /\d/.test(item[1]) ? "ol" : "ul";
      if (!list || list.tagName.toLowerCase() !== tag)
        root.append((list = h(tag)));
      list.append(h("li", {}, inline(item[2])));
    } else if (/^\s*>/.test(line)) {
      flush();
      list = null;
      root.append(h("blockquote", {}, inline(line.replace(/^\s*>\s?/, ""))));
    } else if (!line.trim()) {
      flush();
      list = null;
    } else {
      list = null;
      para.push(line);
    }
  }
  flush();
  return root;
}
// Long text folds behind "展开全文"; the choice survives re-renders by key.
function foldable(key, text) {
  const long =
    (text || "").length > 900 || (text || "").split("\n").length > 14;
  const body = h(
    "div.clamp" + (long && !app.open.has(key) ? ".folded" : ""),
    {},
    md(text),
  );
  if (!long) return body;
  const more = h(
    "button.more",
    { type: "button" },
    app.open.has(key) ? "收起" : "展开全文",
  );
  more.onclick = () => {
    app.open.has(key) ? app.open.delete(key) : app.open.add(key);
    paint(true);
  };
  return [body, more];
}

// Data -----------------------------------------------------------------------------------
async function load(url) {
  const response = await fetch(new URL("state", url), { cache: "no-store" });
  if (!response.ok) throw new Error(String(response.status));
  return response.json();
}
const dir = (href, base) => new URL(href, base || location.href);
const same = (a, b) => !!a && !!b && a.href === b.href;

// One poller. Every reply carries the generation it was asked in, so a slow reply for a
// conversation left meanwhile, or an older workbench list, is dropped.
let timer = null,
  busy = false,
  again = false,
  ticks = 0,
  generation = 0,
  homeAsked = 0,
  homeShown = 0;
async function tick() {
  clearTimeout(timer);
  if (document.hidden) return;
  if (busy) {
    again = true;
    return;
  }
  busy = true;
  try {
    const jobs = [];
    const asked = generation,
      url = app.detailUrl;
    if (url)
      jobs.push(
        load(url).then((data) => {
          if (asked === generation) app.detail = data;
        }),
      );
    if (app.homeUrl && (ticks % 2 === 0 || !app.home)) {
      const order = ++homeAsked;
      jobs.push(
        load(app.homeUrl).then((data) => {
          if (order > homeShown) {
            homeShown = order;
            app.home = data;
          }
        }),
      );
    }
    const results = await Promise.allSettled(jobs);
    app.offline = results.some((result) => result.status === "rejected");
    ticks++;
    paint();
  } finally {
    busy = false;
    if (!document.hidden) timer = setTimeout(tick, again ? 0 : TICK);
    again = false;
  }
}
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) tick();
});

function select(url, push = true) {
  if (same(url, app.detailUrl) && app.detail) return;
  generation++;
  app.detailUrl = url;
  app.detail = null;
  app.speaker = null;
  app.seen = null;
  if (push) history.pushState(null, "", url || app.homeUrl);
  scroller().scrollTop = 0;
  paint(true);
  tick();
}
window.addEventListener("popstate", () => {
  const here = dir("./");
  select(same(here, app.homeUrl) ? null : here, false);
});
function navLink(href, className, kids, label) {
  const url = dir(href, app.homeUrl);
  return h(
    "a." + className,
    {
      href: url.href,
      "aria-label": label,
      "aria-current": same(url, app.detailUrl) ? "page" : null,
      onclick: (event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey || event.button)
          return;
        event.preventDefault();
        select(same(url, app.homeUrl) ? null : url);
      },
    },
    kids,
  );
}

// Rendering ------------------------------------------------------------------------------
function paint(force = false) {
  document.body.dataset.layout = app.layout;
  document.body.classList.toggle("picked", !!app.detailUrl);
  const detail = app.detail,
    problem = app.offline
      ? "连接中断，显示的是上次读取的画面"
      : detail?.stale || detail?.refresh_error || app.home?.refresh_error
        ? "部分历史读取失败，显示的是上次读取的画面"
        : "";
  $("banner").hidden = !problem;
  $("banner").textContent = problem;
  if (app.layout === "workbench")
    update(
      "side",
      [
        app.home,
        app.mode,
        app.query,
        app.detailUrl?.href,
        app.offline,
      ],
      renderSidebar,
      $("sidebar"),
      force,
    );
  const view =
    app.layout === "public"
      ? renderPublic
      : !app.detailUrl
        ? renderOverview
        : !detail
          ? renderLoading
          : detail.mode === "task"
            ? renderTask
            : renderShow;
  const key = [
    view.name,
    detail,
    app.home && !app.detailUrl ? app.home : null,
    app.tabs,
    app.speaker,
    [...app.open],
  ];
  const main = scroller(),
    nearBottom = main.scrollHeight - main.scrollTop - main.clientHeight < 140;
  const before = main.scrollTop;
  if (update("view", key, view, $("view"), force)) {
    main.scrollTop = before;
    followTranscript(nearBottom);
  }
  document.title = app.detail?.title
    ? `${app.detail.title} · AgentNave`
    : "AgentNave · 工作台";
}
function update(slot, key, render, target, force) {
  const next = JSON.stringify(key);
  if (!force && app.keys[slot] === next) return false;
  app.keys[slot] = next;
  const nodes = [render()].flat().filter(Boolean);
  if (nodes.length) target.replaceChildren(...nodes);
  return true;
}
function followTranscript(nearBottom) {
  const detail = app.detail,
    count = detail?.messages?.length || 0;
  const reading =
    detail && detail.mode !== "task" && currentTab(detail) === "transcript";
  if (!reading) {
    $("jump").hidden = true;
    if (detail) app.seen = count;
    return;
  }
  // Only growth after the first load counts as new, including a first message in an empty record.
  if (app.seen !== null && count > app.seen) {
    if (nearBottom && !app.speaker) scrollToEnd();
    else $("jump").hidden = false;
  }
  app.seen = count;
}
const scroller = () =>
  app.layout === "public" ? document.scrollingElement : $("main");
function scrollToEnd() {
  const main = scroller();
  main.scrollTo({ top: main.scrollHeight });
  $("jump").hidden = true;
}
$("jump").onclick = scrollToEnd;

function renderLoading() {
  return h(
    "div.page",
    {},
    h("div.empty", {}, h("span.mark", {}, "N"), h("p", {}, "正在读取会话…")),
  );
}

function renderSidebar() {
  const shows = app.home?.shows || [];
  const query = app.query.trim().toLocaleLowerCase();
  const inMode = (show) => app.mode === "all" || show.mode === app.mode;
  const matches = shows.filter(
    (show) => inMode(show) && show.title.toLocaleLowerCase().includes(query),
  );
  const hot = matches.filter((show) =>
    urgent(statusOf(show.mode, show.status).kind),
  );
  const calm = matches.filter((show) => !hot.includes(show));
  const count = (mode) =>
    shows.filter((show) => mode === "all" || show.mode === mode).length;

  const segment = h(
    "div.segment",
    { role: "group", "aria-label": "按用途筛选" },
    [
      ["all", "全部"],
      ["show", "节目"],
      ["task", "任务"],
    ].map(([mode, label]) =>
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(app.mode === mode),
          onclick: () => {
            app.mode = mode;
            paint();
          },
        },
        label,
        h("b", {}, count(mode)),
      ),
    ),
  );
  const item = (show) => {
    const state = statusOf(show.mode, show.status);
    return navLink(
      show.director_url,
      "conv",
      [
        h("span.glyph." + show.mode, {}, show.mode === "task" ? "任" : "节"),
        h(
          "span.conv-body",
          {},
          h("span.conv-title", {}, show.title || "未命名会话"),
          h(
            "span.conv-meta",
            {},
            dot(state.kind),
            state.label,
          ),
        ),
      ],
      show.title,
    );
  };
  const group = (title, items, extra) =>
    items.length
      ? [h("div.group-title", {}, title, extra), items.map(item)]
      : [];
  const list = h(
    "nav",
    { "aria-label": "会话" },
    group("需要关注", hot),
    group(hot.length ? "其他会话" : "会话", calm),
    !matches.length
      ? h(
          "p.side-empty",
          {},
          shows.length
            ? "没有匹配的会话"
            : "还没有会话。在 boss 的对话中开始一档节目或一个任务。",
        )
      : null,
  );
  const unavailable = app.home?.unavailable || [];
  const foot = [
    unavailable.length
      ? h(
          "details",
          {},
          h("summary", {}, `${unavailable.length} 份历史暂不可用`),
          unavailable.map((entry) =>
            h("p", {}, `${entry.reason}（${entry.path}）`),
          ),
        )
      : null,
    app.home?.storage
      ? h("span.path", { title: "数据位置" }, app.home.storage)
      : null,
  ];
  // The head with the search box is built once, so typing never loses focus.
  $("side-state").replaceChildren(
    dot(app.offline ? "bad" : "done"),
    app.offline ? "已断开" : "已同步",
  );
  $("side-segment").replaceChildren(segment);
  $("side-list").replaceChildren(list);
  $("side-foot").replaceChildren(...foot.filter(Boolean));
  return null;
}
function sidebarShell() {
  const search = h("input", {
    type: "search",
    placeholder: "搜索会话",
    "aria-label": "搜索会话",
    autocomplete: "off",
  });
  search.oninput = () => {
    app.query = search.value;
    paint();
  };
  $("sidebar").replaceChildren(
    h(
      "div.side-head",
      {},
      h(
        "div.side-top",
        {},
        navLink(
          app.homeUrl.href,
          "brand",
          [h("span.mark", {}, "N"), h("span", {}, "AgentNave")],
          "工作台",
        ),
        h("span.link-state#side-state"),
      ),
      h("label.search", {}, search),
      h("div#side-segment"),
    ),
    h("div.side-list#side-list"),
    h("div.side-foot#side-foot"),
  );
}

function renderOverview() {
  const shows = app.home?.shows || [];
  const states = shows.map((show) => ({
    show,
    state: statusOf(show.mode, show.status),
  }));
  const running = states.filter(({ state }) => state.kind === "run");
  const review = states.filter(({ state }) => state.kind === "review");
  const attention = states.filter(({ state }) => urgent(state.kind));
  const hour = new Date().getHours();
  const greeting =
    hour < 6
      ? "夜深了"
      : hour < 12
        ? "早上好"
        : hour < 18
          ? "下午好"
          : "晚上好";
  const summary =
    running.length || review.length
      ? [
          running.length && `${running.length} 个会话正在进行`,
          review.length && `${review.length} 个等待你审核`,
        ]
          .filter(Boolean)
          .join("，") + "。"
      : "眼下一切安静，没有进行中的工作。";
  const tile = (label, value, kind, mode) =>
    h(
      "button.tile" + (kind ? ".tone-" + kind : ""),
      {
        type: "button",
        onclick: () => {
          app.mode = mode;
          paint();
        },
      },
      h("span", {}, kind ? dot(value ? kind : "idle") : null, label),
      h("strong", {}, value),
    );
  return h(
    "div.page",
    {},
    h(
      "header.hero",
      {},
      h("p.eyebrow", {}, "工作台"),
      h("h1", {}, greeting),
      h("p", {}, summary),
    ),
    h(
      "div.tiles",
      {},
      tile("进行中", running.length, "run", "all"),
      tile("待审核", review.length, "review", "all"),
      tile(
        "节目",
        shows.filter((show) => show.mode === "show").length,
        "",
        "show",
      ),
      tile(
        "任务",
        shows.filter((show) => show.mode === "task").length,
        "",
        "task",
      ),
    ),
    h(
      "section.section",
      {},
      h("h2", {}, "需要关注"),
      attention.length
        ? h(
            "div.cards",
            {},
            attention.map(({ show, state }) =>
              navLink(
                show.director_url,
                "card",
                [
                  h(
                    "span.glyph." + show.mode,
                    {},
                    show.mode === "task" ? "任" : "节",
                  ),
                  h(
                    "span.conv-body",
                    {},
                    h("span.conv-title", {}, show.title),
                    h(
                      "span.conv-meta",
                      {},
                      show.mode === "task" ? "任务" : "节目",
                    ),
                  ),
                  pill(state),
                  h("span.arrow", {}, "→"),
                ],
                show.title,
              ),
            ),
          )
        : h(
            "div.calm",
            {},
            h("strong", {}, "没有需要处理的事"),
            "运行中、待审核或出错的会话会出现在这里。",
          ),
    ),
    h(
      "details.guide",
      {},
      h("summary", {}, "如何开始新会话"),
      h(
        "p",
        {},
        "在 boss 的对话中说明用途和内容，boss 会创建会话，这里自动同步。",
      ),
      EXAMPLES.map(([label, text]) =>
        h(
          "div.example",
          {},
          h(
            "p",
            {},
            h("span.badge." + (label === "节目" ? "show" : "task"), {}, label),
            " ",
            text,
          ),
          copyButton(text, "复制示例"),
        ),
      ),
    ),
  );
}

function header(detail, eyebrow, pillState, meta, actions) {
  return h(
    "header.detail-head",
    {},
    app.layout === "workbench"
      ? navLink(app.homeUrl.href, "back", "← 所有会话", "返回所有会话")
      : null,
    h("p.eyebrow", {}, eyebrow),
    h("h1", {}, detail.title || "未命名会话"),
    h(
      "div.head-row",
      {},
      pillState ? pill(pillState) : null,
      meta ? h("span.meta", {}, meta) : null,
      h("span.spacer"),
      actions,
    ),
  );
}
function currentTab(detail) {
  if (detail.view === "public") return "transcript";
  if (app.tabs[detail.id]) return app.tabs[detail.id];
  const live =
    detail.round && ["running", "review"].includes(detail.round.status);
  return live || !detail.messages.length ? "live" : "transcript";
}

function renderShow() {
  const detail = app.detail,
    round = detail.round;
  const cast = castOf(detail);
  const runs = Object.fromEntries(
    (round?.seats || []).map((run) => [run.seat_id, run]),
  );
  const state = statusOf("show", detail.status);
  const speaking = (detail.speaking || []).map(
    (id) => cast.find((seat) => seat.id === id)?.label || id,
  );
  const tab = currentTab(detail);
  const choose = (next) => {
    app.tabs[detail.id] = next;
    paint();
  };
  const flags = [
    detail.review ? "导演审核后公开" : "回答自动公开",
    detail.isolated ? "隔离席位" : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return h(
    "div.page",
    {},
    header(
      detail,
      [h("span.badge.show", {}, "节目"), flags],
      state,
      [
        speaking.length ? `${speaking.join("、")} 作答中 · ` : "",
        `${detail.seats.length} 位选手 · ${detail.messages.length} 条公开发言`,
      ].join(""),
      [
        detail.public_url
          ? h(
              "a.btn",
              { href: detail.public_url, target: "_blank", rel: "noreferrer" },
              "选手视角 ↗",
            )
          : null,
        h(
          "a.btn",
          {
            href: new URL("transcript.jsonl", app.detailUrl).href,
            download: "transcript.jsonl",
          },
          "导出 JSONL",
        ),
      ],
    ),
    h(
      "div.cast",
      { "aria-label": "席位" },
      cast.map((seat) => {
        const run = runs[seat.id];
        const kind = run
          ? run.review === "pending"
            ? "review"
            : (SEAT[run.status] || ["", "idle"])[1]
          : "";
        const ring = h(
          "span.ring" + (kind ? ".tone-" + kind : ""),
          { style: "position:relative;display:grid;border-radius:9px" },
          avatar(seat),
        );
        return h(
          "div.seat",
          {
            title: run
              ? run.review
                ? REVIEW[run.review]?.[0]
                : SEAT[run.status]?.[0]
              : "",
          },
          ring,
          h(
            "span.seat-text",
            {},
            h("strong", { title: seat.label }, modelName(seat)),
            h("small", {}, identityNote(seat)),
          ),
        );
      }),
    ),
    h(
      "nav.tabs",
      { "aria-label": "视图" },
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(tab === "live"),
          onclick: () => choose("live"),
        },
        "本轮现场",
        h("span.private", {}, "私有"),
      ),
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(tab === "transcript"),
          onclick: () => choose("transcript"),
        },
        "公开记录",
        h("b", {}, detail.messages.length),
      ),
    ),
    tab === "live" ? renderDesk(detail, cast) : renderTranscript(detail, cast),
  );
}

function renderDesk(detail, cast) {
  const round = detail.round;
  if (!round)
    return h(
      "div.empty",
      {},
      h("strong", {}, "还没有开始第一轮"),
      "boss 发起轮次后，各席位的进展会实时出现在这里。",
    );
  const state = statusOf("show", round.status);
  return [
    h(
      "div.round-head",
      {},
      h("h3", {}, `第 ${round.number} 轮`),
      h("span.badge.show", {}, round.order === "parallel" ? "并联" : "串联"),
      pill(state),
    ),
    round.error ? h("p.error", {}, "本轮提示：" + round.error) : null,
    h(
      "div.desk",
      {},
      round.seats.map((run) => {
        const seat = cast.find((item) => item.id === run.seat_id) || {
          id: run.seat_id,
          label: run.seat_id,
          model: "",
          provider: "",
        };
        const runState = run.review
          ? tone(REVIEW[run.review] || [run.review, "idle"])
          : tone(SEAT[run.status] || [run.status, "idle"]);
        const key = `desk:${detail.id}:${round.number}:${run.seat_id}`;
        return h(
          "article.run.tone-" + runState.kind,
          {},
          h(
            "div.run-head",
            {},
            avatar(seat),
            h(
              "span.who",
              {},
              h("strong", { title: seat.label }, modelName(seat)),
              h("small", {}, identityNote(seat)),
            ),
            pill(runState),
          ),
          run.instruction
            ? h("div.instruction", {}, h("b", {}, "私密指令 "), run.instruction)
            : null,
          run.output
            ? foldable(key, run.output)
            : run.status === "running"
              ? h(
                  "div.waiting",
                  {},
                  h("span.typing", {}, h("i"), h("i"), h("i")),
                  "正在等待回复…",
                )
              : null,
          run.error ? h("p.error", {}, run.error) : null,
        );
      }),
    ),
    detail.review
      ? h(
          "p.meta",
          { style: "color:var(--faint);font-size:12px;margin-top:16px" },
          "发布或丢弃回答，请在 boss 的对话中操作。",
        )
      : null,
  ];
}

function renderTranscript(detail, cast) {
  const messages = detail.messages || [];
  const speakers = cast.filter((seat) =>
    messages.some((message) => message.speaker === seat.id),
  );
  const filters =
    speakers.length > 1
      ? h(
          "div.filters",
          { role: "group", "aria-label": "按角色查看" },
          h(
            "button",
            {
              type: "button",
              "aria-pressed": String(!app.speaker),
              onclick: () => {
                app.speaker = null;
                paint();
              },
            },
            "所有人",
          ),
          speakers.map((seat) =>
            h(
              "button",
              {
                type: "button",
                "aria-pressed": String(app.speaker === seat.id),
                onclick: () => {
                  app.speaker = app.speaker === seat.id ? null : seat.id;
                  paint();
                },
              },
              `${modelName(seat)} · ${identityNote(seat)}`,
            ),
          ),
        )
      : null;
  const shown = messages
    .map((message, index) => ({ message, index }))
    .filter(({ message }) => !app.speaker || message.speaker === app.speaker);
  if (!shown.length)
    return [
      filters,
      h(
        "div.empty",
        {},
        h("span.mark", {}, "N"),
        h(
          "strong",
          {},
          app.speaker ? "这位还没有公开发言" : "好戏，从一句话开始",
        ),
        app.speaker
          ? "换一个角色看看。"
          : "主持人开场、选手回应，公开发言会依次留在这里。",
      ),
    ];
  let round = null;
  const items = [];
  for (const { message, index } of shown) {
    if (message.round !== round) {
      round = message.round;
      items.push(h("div.divider", {}, `第 ${round} 轮`));
    }
    const identity = cast.find((item) => item.id === message.speaker) || {
      id: message.speaker,
      label: message.speaker,
      model: "",
      provider: "",
    };
    const seat = identity.provider === "host" ? { ...identity, model: message.manager_model || "" } : identity;
    items.push(
      h(
        "article.msg" +
          (seat.provider === "host" ? ".host" : "") +
          (message.kind === "absent" ? ".absent" : ""),
        {},
        avatar(seat, ".lg"),
        h(
          "div.msg-body",
          {},
          h(
            "div.msg-head",
            {},
            h("strong", { title: seat.label }, modelName(seat)),
            h("span.meta", {}, identityNote(seat)),
            message.kind === "task"
              ? h("span.badge.show", {}, "本轮任务")
              : message.kind === "forward"
                ? h("span.badge.forward", {}, "转发")
                : null,
            h("time", { datetime: message.time }, when(message.time)),
            h("span.num", {}, "#" + String(index + 1).padStart(3, "0")),
            h("span.tools", {}, copyButton(message.text || "")),
          ),
          message.kind === "forward" && message.forward_source
            ? h("p.forward-source", {}, "转发自 " + message.forward_source)
            : null,
          foldable(`msg:${detail.id}:${index}`, message.text),
        ),
      ),
    );
  }
  return [filters, h("div.timeline", {}, items)];
}

function renderTask() {
  const detail = app.detail,
    tasks = detail.tasks || [];
  const running = tasks.filter((task) => task.status === "running").length;
  const latest = tasks[tasks.length - 1];
  const state = running
    ? tone(["进行中", "run"])
    : statusOf("task", latest?.status || "idle");
  return h(
    "div.page",
    {},
    header(
      detail,
      [h("span.badge.task", {}, "任务"), "仅 boss 可见 · 回复不等于验收通过"],
      state,
      `${tasks.length} 次调用${running ? ` · ${running} 个运行中` : ""}`,
      null,
    ),
    tasks.length
      ? h(
          "div.runs",
          {},
          [...tasks].reverse().map((task) => {
            const taskState = statusOf("task", task.status);
            const seat = {
              label: task.provider,
              provider: task.provider,
              model: task.model,
            };
            const manager = {
              label: detail.host || "boss",
              provider: "host",
              host: detail.host || "",
              model: task.manager_model || "",
            };
            return h(
              "article.task-run.tone-" + taskState.kind,
              {},
              h(
                "header",
                {},
                avatar(seat, ".lg"),
                h(
                  "span.who",
                  {},
                  h("strong", {}, modelName(seat)),
                  h(
                    "small",
                    {},
                    `${identityNote(seat)} · ${when(task.time)} · ${task.status === "running" ? "已运行 " : "用时 "}${duration(task.elapsed_ms)}`,
                  ),
                ),
                pill(taskState),
              ),
              h(
                "div.chips",
                {},
                h("span.chip", { title: "模型" }, task.model || "原生设置"),
                h("span.chip", { title: task.cwd }, task.cwd),
                task.forward && task.forward.length
                  ? h(
                      "span.chip",
                      { title: task.forward.join("\n") },
                      `转发 ${task.forward.length} 条`,
                    )
                  : null,
              ),
              task.prompt
                ? h(
                    "div.body.prompt",
                    {},
                    h(
                      "p.meta.prompt-who",
                      {},
                      avatar(manager, ".sm"),
                      h(
                        "span.who",
                        {},
                        h("strong", {}, modelName(manager)),
                        h("small", {}, identityNote(manager)),
                      ),
                    ),
                    foldable(`prompt:${detail.id}:${task.id}`, task.prompt),
                  )
                : null,
              h(
                "div.body",
                {},
                task.output
                  ? foldable(`task:${detail.id}:${task.id}`, task.output)
                  : task.status === "running"
                    ? h(
                        "div.waiting",
                        {},
                        h("span.typing", {}, h("i"), h("i"), h("i")),
                        "正在等待回复…",
                      )
                    : h(
                        "p.meta",
                        { style: "color:var(--faint);margin:0" },
                        "没有回复内容",
                      ),
                task.error ? h("p.error", {}, task.error) : null,
                task.output
                  ? h(
                      "div",
                      { style: "display:flex;justify-content:flex-end" },
                      copyButton(task.output, "复制回复"),
                    )
                  : null,
              ),
            );
          }),
        )
      : h(
          "div.empty",
          {},
          h("strong", {}, "还没有任务"),
          "boss 派发的任务会出现在这里。",
        ),
  );
}

function renderPublic() {
  const detail = app.detail;
  if (!detail) return renderLoading();
  const cast = castOf(detail);
  return [
    h(
      "div.public-bar",
      {},
      h(
        "span.brand",
        {},
        h("span.mark", {}, "N"),
        "AgentNave",
        h("small", {}, "公开对话"),
      ),
      h(
        "a.btn",
        {
          href: new URL("transcript.jsonl", app.detailUrl).href,
          download: "transcript.jsonl",
        },
        "导出 JSONL",
      ),
    ),
    h(
      "div.page",
      {},
      h(
        "header.detail-head",
        {},
        h(
          "p.eyebrow",
          {},
          h("span.badge.show", {}, "节目"),
          "全员可见的公开记录",
        ),
        h("h1", {}, detail.title),
        h(
          "div.head-row",
          {},
          h(
            "span.meta",
            {},
            `${detail.seats.length} 位选手 · ${detail.messages.length} 条公开发言`,
          ),
        ),
      ),
      h(
        "div.cast",
        {},
        cast.map((seat) =>
          h(
            "div.seat",
            {},
            avatar(seat),
            h(
              "span.seat-text",
              {},
              h("strong", { title: seat.label }, modelName(seat)),
              h("small", {}, identityNote(seat)),
            ),
          ),
        ),
      ),
      renderTranscript(detail, cast),
    ),
  ];
}

// Boot -----------------------------------------------------------------------------------
async function boot() {
  const here = dir("./");
  let data;
  try {
    data = await load(here);
  } catch {
    $("view").replaceChildren(
      h(
        "div.boot",
        {},
        h("span.mark", {}, "N"),
        h("p", {}, "无法连接到 AgentNave，请稍后刷新。"),
      ),
    );
    setTimeout(boot, 3000);
    return;
  }
  if (data.view === "public") {
    app.layout = "public";
    app.detail = data;
    app.detailUrl = here;
  } else if (data.view === "workbench") {
    app.layout = "workbench";
    app.home = data;
    app.homeUrl = here;
  } else {
    app.layout = "workbench";
    app.detail = data;
    app.detailUrl = here;
    app.homeUrl = dir(data.workbench_url || "./", here);
  }
  if (app.layout === "workbench") sidebarShell();
  paint(true);
  tick();
}
boot();
