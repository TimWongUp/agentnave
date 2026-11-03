"use strict";

{
  const { h, L, load, href, setText } = AN;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  async function post(path, body) {
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
    if (!started.ok) throw new Error(started.message || L("已有安装正在进行", "An installation is already in progress"));
    for (let i = 0; i < 300; i++) {
      await sleep(800);
      // A closed window stays loaded; check again once it is shown, the install keeps going.
      if (document.hidden) await new Promise((resolve) => document.addEventListener("visibilitychange", resolve, { once: true }));
      const state = (await load("/app/status")).service;
      if (state?.state === "failed")
        throw new Error(state.error || L("安装失败，请重试。", "Installation failed, please try again."));
      if (state?.state === "done") return;
    }
    throw new Error(L("安装超时，请稍后到“设置 → 版本与更新”查看。", "Installation timed out. Check Settings → About later."));
  }

  // ---- 开始使用 -----------------------------------------------------------------------------
  const TITLES = {
    service: L("安装本机服务", "Install local service"),
    host: L("接入一个 boss 应用", "Connect a boss app"),
    members: L("设置员工", "Set up employees"),
    try: L("在 boss 里派一次任务", "Dispatch a task from boss"),
  };
  const FALLBACK = {
    service: L("boss 要通过它才能启动 AgentNave", "Required for boss to launch AgentNave"),
    host: L("在设置里一键接入已检测到的应用", "Connect detected apps with one click in Settings"),
    members: L("选择哪些 CLI 与模型可以当员工", "Choose which CLIs and models work as employees"),
    try: L("在 boss 里说：用 agentnave 让几位员工各答一遍", "In boss, say: use agentnave to have a few employees each answer"),
  };
  const SAMPLE = L("用 agentnave 让两位员工各自回答一遍：这个仓库是做什么的？", "Use agentnave to have two employees each answer: what does this repo do?");
  // First pending step -> its one primary action. `to` is a real route; `run` is an in-place action.
  const ACTION = {
    service: {
      label: L("安装本机服务", "Install local service"),
      busy: L("安装中，约需一分钟…", "Installing, about a minute…"),
      run: installService,
    },
    host: { label: L("去接入 boss", "Connect boss"), to: "/settings/#hosts" },
    members: { label: L("去设置员工", "Set up employees"), to: "/settings/#members" },
    try: { label: L("复制示例指令", "Copy sample prompt"), copy: SAMPLE },
  };

  function onboarding({ variant = "compact", onChange } = {}) {
    const root = h("section.ob.ob-" + variant, {
      "aria-labelledby": "ob-title",
    });
    root.hidden = true;
    const ctrl = { el: root, data: null, visible: false };
    const ctx = { root, ctrl, variant, onChange, busy: null, error: null, sawIncomplete: false, celebrating: false, lastKey: "" };

    const allDone = (d) => d.steps.every((s) => s.done);
    const shown = () =>
      !!ctrl.data &&
      !ctrl.data.dismissed &&
      (ctx.celebrating || !allDone(ctrl.data));

    const render = () => onboardingRender(ctx);
    ctx.shown = shown;
    ctx.render = render;
    ctx.stepNode = (step, isCurrent) => onboardingStepNode(ctx, step, isCurrent);
    ctx.actionNode = (id, act) => onboardingActionNode(ctx, id, act);
    ctx.onDismiss = onDismiss;
    ctx.refresh = refresh;

    async function onDismiss() {
      try {
        await post("/app/onboarding/dismiss", {});
        ctrl.data = { ...ctrl.data, dismissed: true };
      } catch {
        ctx.error = {
          step: (ctrl.data.steps.find((s) => !s.done) || {}).id,
          message: L("没能关闭，请稍后再试。", "Could not close. Please try again later."),
        };
      }
      render();
    }

    async function refresh() {
      if (ctx.busy) return;
      try {
        ctrl.data = await load("/app/onboarding");
      } catch {
        return; // keep what is shown; the next poll retries
      }
      if (!allDone(ctrl.data)) ctx.sawIncomplete = true;
      else if (ctx.sawIncomplete && !ctx.celebrating && !ctrl.data.dismissed) {
        ctx.celebrating = true;
        setTimeout(() => {
          ctx.celebrating = false;
          ctx.sawIncomplete = false;
          render();
        }, 4000);
      }
      render();
    }
    ctrl.refresh = refresh;
    ctrl.setVariant = (next) => {
      if (next === ctx.variant) return;
      ctx.variant = next;
      root.className = "ob ob-" + ctx.variant;
      ctx.lastKey = "";
      render();
    };
    ctrl.start = () => AN.poll(refresh, 15000); // not the 1.5 s loop: the server reads host configs
    return ctrl;
  }

  function onboardingRender(ctx) {
    const was = ctx.ctrl.visible;
    ctx.ctrl.visible = ctx.shown();
    if (was !== ctx.ctrl.visible) ctx.onChange && (0, ctx.onChange)(ctx.ctrl.visible);
    ctx.root.hidden = !ctx.ctrl.visible;
    if (!ctx.ctrl.visible) {
      ctx.lastKey = "";
      return;
    }
    const key = JSON.stringify([
      ctx.ctrl.data,
      ctx.busy,
      ctx.error,
      ctx.celebrating,
      ctx.variant,
    ]);
    if (key === ctx.lastKey) return;
    ctx.lastKey = key;
    const { steps } = ctx.ctrl.data;
    const done = steps.filter((s) => s.done).length;
    const current = steps.find((s) => !s.done);
    const dismiss = h(
      "button.ob-x",
      {
        type: "button",
        "aria-label": L("关闭“开始使用”", "Close Getting started"),
        title: L("关闭后不再显示", "Hide Getting started"),
      },
      "×",
    );
    dismiss.addEventListener("click", ctx.onDismiss);
    const head = h(
      "header.ob-head",
      {},
      h(
        "h2.ob-title",
        { id: "ob-title" },
        ctx.celebrating ? L("都准备好了", "All set") : L("开始使用", "Getting started"),
      ),
      h(
        "span.ob-count",
        { "aria-label": L(`已完成 ${done} 步，共 ${steps.length} 步`, `${done} of ${steps.length} steps completed`) },
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
    if (ctx.celebrating) {
      kids.push(
        h("p.ob-lead", {}, L("接入和设置都完成了，这张卡片几秒后自动收起。", "Connection and setup are complete. This card will close in a few seconds.")),
      );
    } else {
      if (ctx.variant === "full")
        kids.push(
          h(
            "p.ob-lead",
            {},
            L("完成这几步，办公室里就会出现 boss、工位和会议室。", "Complete these steps to see boss, desks, and meeting rooms in the office."),
          ),
        );
      kids.push(
        h(
          "ol.ob-steps",
          {},
          steps.map((s) => ctx.stepNode(s, s === current)),
        ),
      );
    }
    ctx.root.replaceChildren(...kids);
  }

  function onboardingStepNode(ctx, step, isCurrent) {
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
              ? L("（已完成）", " (completed)")
              : isCurrent
                ? L("（当前步骤）", " (current step)")
                : L("（未完成）", " (incomplete)"),
          ),
        ),
        detail && h("span.ob-detail", {}, detail),
      ),
    ];
    const li = h("li.ob-step" + cls, {}, ...kids);
    if (isCurrent) {
      const body = li.querySelector(".ob-body");
      const act = ACTION[step.id];
      if (act) body.append(h("div.ob-act", {}, ctx.actionNode(step.id, act)));
      if (ctx.error && ctx.error.step === step.id)
        body.append(h("p.ob-err", { role: "alert" }, ctx.error.message));
    }
    return li;
  }

  function onboardingActionNode(ctx, id, act) {
    if (act.to)
      return h(
        "a.ob-btn",
        { href: href(act.to) },
        act.label,
        h("span", { "aria-hidden": "true" }, " →"),
      );
    const pending = ctx.busy === id;
    const btn = h(
      "button.ob-btn" + (pending ? ".pending" : ""),
      { type: "button", disabled: pending },
      pending ? [h("i.ob-spin"), act.busy] : ctx.error && ctx.error.step === id && act.run ? L("重试", "Retry") : act.label,
    );
    btn.addEventListener("click", async () => {
      if (act.copy) {
        try {
          await navigator.clipboard.writeText(act.copy);
        } catch {
          setText(btn, L("复制失败", "Copy failed"));
          setTimeout(() => setText(btn, act.label), 2000);
          return;
        }
        setText(btn, L("已复制", "Copied"));
        setTimeout(() => setText(btn, act.label), 2000);
        return;
      }
      ctx.busy = id;
      ctx.error = null;
      ctx.render();
      try {
        await act.run();
      } catch (e) {
        ctx.error = { step: id, message: `${e.message || L("操作失败", "Action failed")}` };
      }
      ctx.busy = null;
      await ctx.refresh();
    });
    return btn;
  }

  Object.assign(AN, { post, installService, onboarding });
}
