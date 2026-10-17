"use strict";
// Helpers shared by the panel and the chat page. Classic script, one global: AN.
// Every node is built with createElement/textContent.
(function () {
  const mock = () => (window.AgentNaveMock && window.AgentNaveMock.enabled() ? window.AgentNaveMock : null);

  // h("a.row.tone-run", {href}, child, [children]) -> element. "style" goes through CSSOM.
  function h(tag, props, ...kids) {
    const [name, ...classes] = tag.split(".");
    const node = document.createElement(name || "div");
    if (classes.length) node.className = classes.join(" ");
    for (const [key, value] of Object.entries(props || {})) {
      if (value == null || value === false) continue;
      if (key === "style") node.style.cssText = value;
      else node.setAttribute(key, value === true ? "" : value);
    }
    node.append(...kids.flat(Infinity).filter((kid) => kid != null && kid !== false));
    return node;
  }
  const $ = (id) => document.getElementById(id);

  async function load(path) {
    const m = mock();
    if (m) return m.fetch(path);
    const response = await fetch(path, { cache: "no-store" });
    if (!response.ok) throw Object.assign(new Error(String(response.status)), { status: response.status });
    return response.json();
  }
  // Real routes in production; mapped to the prototype's files under the mock.
  const href = (path) => (mock() ? mock().href(path) : path);

  // ---- vocabulary ---------------------------------------------------------------------
  // [label, tone]; tone selects one of --run --review --bad --done --idle.
  const MEMBER = {
    running: ["进行中", "run"], review: ["待审核", "review"], succeeded: ["已回复", "done"],
    failed: ["失败", "bad"], blocked: ["受阻", "bad"], cancelled: ["已取消", "idle"],
    interrupted: ["已中断", "bad"], pending: ["等待中", "idle"],
  };
  const GROUP = { running: ["运行中", "run"], review: ["待审核", "review"], failed: ["有失败", "bad"], idle: ["空闲", "idle"] };
  const CONV = {
    idle: ["未开始", "idle"], running: ["进行中", "run"], review: ["待审核", "review"],
    succeeded: ["已回复", "done"], finished: ["已结束", "done"], failed: ["失败", "bad"],
    blocked: ["受阻", "bad"], cancelled: ["已取消", "idle"], interrupted: ["已中断", "bad"],
  };
  const pick = (table, status) => table[status] || [status || "—", "idle"];
  const urgent = (status) => status === "running" || status === "review";

  function avatar(provider, model, cls) {
    return Avatars.create("model", provider, model, cls);
  }
  const colorClass = (n) => "c" + (Math.abs(Number(n) || 0) % 8);

  // ---- time ---------------------------------------------------------------------------
  const at = (iso) => {
    const t = Date.parse(iso || "");
    return Number.isNaN(t) ? 0 : t;
  };
  function relTime(iso) {
    const s = Math.max(0, (Date.now() - at(iso)) / 1000);
    if (s < 45) return "刚刚";
    if (s < 3600) return `${Math.round(s / 60)} 分钟前`;
    if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
    const d = new Date(at(iso));
    return `${d.getMonth() + 1}/${d.getDate()}`;
  }
  const clock = (iso) => new Date(at(iso)).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  const fullTime = (iso) => new Date(at(iso)).toLocaleString("zh-CN", { hour12: false });
  function dayLabel(iso) {
    const d = new Date(at(iso));
    const today = new Date();
    const diff = Math.round((new Date(today.getFullYear(), today.getMonth(), today.getDate()) - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 864e5);
    return diff === 0 ? "今天" : diff === 1 ? "昨天" : `${d.getMonth() + 1} 月 ${d.getDate()} 日`;
  }

  // ---- in-place DOM updates -----------------------------------------------------------
  const setText = (node, text) => { if (node.textContent !== text) node.textContent = text; };
  const setAttr = (node, name, value) => {
    if (value == null) { if (node.hasAttribute(name)) node.removeAttribute(name); }
    else if (node.getAttribute(name) !== value) node.setAttribute(name, value);
  };
  const setClass = (node, className) => { if (node.className !== className) node.className = className; };
  // Make parent's children equal `nodes`, touching the DOM only where the order differs.
  function reconcile(parent, nodes) {
    nodes.forEach((node, i) => {
      if (parent.children[i] !== node) parent.insertBefore(node, parent.children[i] || null);
    });
    while (parent.children.length > nodes.length) parent.lastElementChild.remove();
  }
  // Stable order: ids keep their slot; new ids go first; rows only move when `rank` changes
  // (running/review before the rest). Server order is used only to seed new ids.
  function stickyOrder(previous, items) {
    const slot = new Map(previous.map((id, i) => [id, i]));
    const fresh = items.filter((x) => !slot.has(x.id));
    const kept = items.filter((x) => slot.has(x.id)).sort((a, b) => slot.get(a.id) - slot.get(b.id));
    return [...(previous.length ? fresh : []), ...kept, ...(previous.length ? [] : fresh)]
      .sort((a, b) => a.rank - b.rank) // Array.prototype.sort is stable
      .map((x) => x.id);
  }
  // 1.5 s polling that pauses while hidden and never overlaps itself.
  function poll(tick, every = 1500) {
    let timer = null, busy = false;
    async function run() {
      clearTimeout(timer);
      if (document.hidden || busy) return;
      busy = true;
      try { await tick(); } finally {
        busy = false;
        if (!document.hidden) timer = setTimeout(run, every);
      }
    }
    document.addEventListener("visibilitychange", run);
    window.addEventListener("focus", run);
    run();
    return run;
  }
  // Markdown punctuation dropped; blank runs collapsed (used for one-line previews).
  const plain = (text) =>
    String(text || "").split("\n").filter((l) => !/^\s*```/.test(l))
      .map((l) => l.replace(/^\s{0,3}(#{1,6}|>)\s*/, "").replace(/(\*\*|__|`)/g, "").trimEnd())
      .join("\n").replace(/\n{3,}/g, "\n\n").trim();

  document.addEventListener("click", (event) => {
    const link = event.target.closest("a[href]");
    if (mock() && link?.getAttribute("href").startsWith("/app/")) event.preventDefault();
  });

  window.AN = { h, $, load, href, MEMBER, GROUP, CONV, pick, urgent, avatar, colorClass, at, relTime, clock, fullTime, dayLabel, setText, setAttr, setClass, reconcile, stickyOrder, poll, plain };
})();

(function () {
  const { h, load, href, setText } = AN;
  const mock = () =>
    window.AgentNaveMock && window.AgentNaveMock.enabled()
      ? window.AgentNaveMock
      : null;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  async function post(path, body) {
    const m = mock();
    if (m && m.post) return m.post(path, body || {});
    const response = await fetch(path, {
      method: "POST",
      headers: {
        "X-AgentNave-Action": "1",
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body || {}),
    });
    if (!response.ok)
      throw Object.assign(new Error((await response.json().catch(() => ({}))).error || String(response.status)), {
        status: response.status,
      });
    return response.json();
  }

  // The install runs for about a minute: POST returns at once, progress is /app/status.service.state.
  async function installService() {
    const started = await post("/app/service/install", {});
    if (!started.ok) throw new Error(started.message || "已有安装正在进行");
    for (let i = 0; i < 300; i++) {
      await sleep(800);
      const state = (await load("/app/status")).service;
      if (state?.state === "failed")
        throw new Error(state.error || "安装失败，请重试。");
      if (state?.state === "done") return;
    }
    throw new Error("安装超时，请稍后到“设置 → 版本与更新”查看。");
  }

  // ---- 开始使用 -----------------------------------------------------------------------------
  const TITLES = {
    service: "安装本机服务",
    host: "接入一个宿主",
    members: "设置组员",
    try: "在宿主里派一次任务",
  };
  const FALLBACK = {
    service: "宿主要通过它才能启动 AgentNave",
    host: "在设置里一键接入已检测到的宿主",
    members: "选择哪些 CLI 与模型可以当组员",
    try: "在宿主里说：用 agentnave 让几位组员各答一遍",
  };
  const SAMPLE = "用 agentnave 让两位组员各自回答一遍：这个仓库是做什么的？";
  // First pending step -> its one primary action. `to` is a real route; `run` is an in-place action.
  const ACTION = {
    service: {
      label: "安装本机服务",
      busy: "安装中，约需一分钟…",
      run: installService,
    },
    host: { label: "去接入宿主", to: "/settings/#hosts" },
    members: { label: "去设置组员", to: "/settings/#members" },
    try: { label: "复制示例指令", copy: SAMPLE },
  };

  function onboarding({ variant = "compact", onChange } = {}) {
    const root = h("section.ob.ob-" + variant, {
      "aria-labelledby": "ob-title",
    });
    root.hidden = true;
    const ctrl = { el: root, data: null, visible: false };
    let busy = null,
      error = null,
      sawIncomplete = false,
      celebrating = false,
      lastKey = "";

    const allDone = (d) => d.steps.every((s) => s.done);
    const shown = () =>
      !!ctrl.data &&
      !ctrl.data.dismissed &&
      (celebrating || !allDone(ctrl.data));

    function render() {
      const was = ctrl.visible;
      ctrl.visible = shown();
      if (was !== ctrl.visible) onChange && onChange(ctrl.visible);
      root.hidden = !ctrl.visible;
      if (!ctrl.visible) {
        lastKey = "";
        return;
      }
      const key = JSON.stringify([
        ctrl.data,
        busy,
        error,
        celebrating,
        variant,
      ]);
      if (key === lastKey) return;
      lastKey = key;
      const { steps } = ctrl.data;
      const done = steps.filter((s) => s.done).length;
      const current = steps.find((s) => !s.done);
      const dismiss = h(
        "button.ob-x",
        {
          type: "button",
          "aria-label": "关闭“开始使用”",
          title: "关闭后不再显示",
        },
        "×",
      );
      dismiss.addEventListener("click", onDismiss);
      const head = h(
        "header.ob-head",
        {},
        h(
          "h2.ob-title",
          { id: "ob-title" },
          celebrating ? "都准备好了" : "开始使用",
        ),
        h(
          "span.ob-count",
          { "aria-label": `已完成 ${done} 步，共 ${steps.length} 步` },
          `${done}/${steps.length}`,
        ),
        dismiss,
      );
      const bar = h(
        "div.ob-bar",
        { "aria-hidden": "true" },
        steps.map((s) => h("i" + (s.done ? ".on" : ""))),
      );
      const kids = [head, bar];
      if (celebrating) {
        kids.push(
          h("p.ob-lead", {}, "接入和设置都完成了，这张卡片几秒后自动收起。"),
        );
      } else {
        if (variant === "full")
          kids.push(
            h(
              "p.ob-lead",
              {},
              "完成这几步，群聊就会开始出现主控与组员的对话。",
            ),
          );
        kids.push(
          h(
            "ol.ob-steps",
            {},
            steps.map((s) => stepNode(s, s === current)),
          ),
        );
      }
      root.replaceChildren(...kids);
    }

    function stepNode(step, isCurrent) {
      const cls = step.done ? ".done" : isCurrent ? ".current" : "";
      const name = TITLES[step.id] || step.id;
      const detail = step.detail || (step.done ? "" : FALLBACK[step.id]);
      const kids = [
        h("span.ob-ind", { "aria-hidden": "true" }),
        h(
          "div.ob-body",
          {},
          h(
            "b.ob-name",
            {},
            name,
            h(
              "span.sr-only",
              {},
              step.done
                ? "（已完成）"
                : isCurrent
                  ? "（当前步骤）"
                  : "（未完成）",
            ),
          ),
          detail && h("span.ob-detail", {}, detail),
        ),
      ];
      const li = h("li.ob-step" + cls, {}, ...kids);
      if (isCurrent) {
        const body = li.querySelector(".ob-body");
        const act = ACTION[step.id];
        if (act) body.append(h("div.ob-act", {}, actionNode(step.id, act)));
        if (error && error.step === step.id)
          body.append(h("p.ob-err", { role: "alert" }, error.message));
      }
      return li;
    }

    function actionNode(id, act) {
      if (act.to)
        return h(
          "a.ob-btn",
          { href: href(act.to) },
          act.label,
          h("span", { "aria-hidden": "true" }, " →"),
        );
      const pending = busy === id;
      const btn = h(
        "button.ob-btn" + (pending ? ".pending" : ""),
        { type: "button", disabled: pending },
        pending ? [h("i.ob-spin"), act.busy] : error && error.step === id && act.run ? "重试" : act.label,
      );
      btn.addEventListener("click", async () => {
        if (act.copy) {
          try {
            await navigator.clipboard.writeText(act.copy);
          } catch {
            setText(btn, "复制失败");
            setTimeout(() => setText(btn, act.label), 2000);
            return;
          }
          setText(btn, "已复制");
          setTimeout(() => setText(btn, act.label), 2000);
          return;
        }
        busy = id;
        error = null;
        render();
        try {
          await act.run();
        } catch (e) {
          error = { step: id, message: `${e.message || "操作失败"}` };
        }
        busy = null;
        await refresh();
      });
      return btn;
    }

    async function onDismiss() {
      try {
        await post("/app/onboarding/dismiss", {});
        ctrl.data = { ...ctrl.data, dismissed: true };
      } catch {
        error = {
          step: (ctrl.data.steps.find((s) => !s.done) || {}).id,
          message: "没能关闭，请稍后再试。",
        };
      }
      render();
    }

    async function refresh() {
      if (busy) return;
      try {
        ctrl.data = await load("/app/onboarding");
      } catch {
        return; // keep what is shown; the next poll retries
      }
      if (!allDone(ctrl.data)) sawIncomplete = true;
      else if (sawIncomplete && !celebrating && !ctrl.data.dismissed) {
        celebrating = true;
        setTimeout(() => {
          celebrating = false;
          sawIncomplete = false;
          render();
        }, 4000);
      }
      render();
    }
    ctrl.refresh = refresh;
    ctrl.setVariant = (next) => {
      if (next === variant) return;
      variant = next;
      root.className = "ob ob-" + variant;
      lastKey = "";
      render();
    };
    ctrl.start = () => AN.poll(refresh, 15000); // not the 1.5 s loop: the server reads host configs
    return ctrl;
  }

  Object.assign(AN, { post, installService, onboarding });
})();
