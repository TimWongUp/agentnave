"use strict";
// First-run checklist in the office's empty state. It appears for a user with no boss connected or no
// employee confirmed, stays until every step is done or the user skips it, and writes nothing but its
// own state to the App's preferences. Each step's status is read live from the App.
(() => {
  const { h, L, load, post, installService, href } = AN;
  const { evaluate, isNewUser, isFinished } = window.ANOnboarding;
  const box = document.getElementById("onboarding");
  if (!box) return;

  const POLL_MS = 5000;
  const data = {
    status: null,
    hosts: null,
    members: null,
    orchestration: null,
    prefs: null,
  };
  const busy = new Set(); // step ids whose button was pressed and has not answered yet
  const problem = {}; // step id -> why the last press failed
  let placed = false; // the blank state is showing the "no workspaces" message
  let timer = 0;
  let started = false; // this window decided the checklist is on, saved or not
  let over = false; // finished or skipped in this window
  let writing = null; // the state being saved, so one decision is written once
  let saving = Promise.resolve(); // writes go out one after another, so a later state lands last
  let reading = null; // the read in flight
  let again = false; // something changed while it was in flight: read once more after it
  let drawn = "";

  // null while preferences could not be read, so nothing is decided or written.
  const savedState = () =>
    data.prefs && !data.prefs.error
      ? data.prefs.preferences?.onboarding?.state || ""
      : null;

  function saveState(state) {
    if (writing === state) return;
    writing = state;
    saving = saving.then(async () => {
      try {
        const saved = await post("/app/preferences", { onboarding: { state } });
        data.prefs = { ...data.prefs, preferences: saved.preferences };
      } catch {
        if (writing === state) writing = null; // tried again by the next decision
      }
      render();
    });
  }

  // Whether the checklist shows; the first showing and the finish are saved.
  function decide(steps) {
    const saved = savedState();
    if (over || !steps || saved === null || ["done", "skipped"].includes(saved))
      return false;
    if (!started && saved !== "active" && !isNewUser(steps)) return false;
    started = true;
    if (isFinished(steps)) {
      over = true;
      void saveState("done");
      return false;
    }
    if (saved !== "active") void saveState("active");
    return true;
  }

  const names = (list) => list.join(L("、", ", "));
  const working = () =>
    h(
      "button.ob-btn.pending",
      { type: "button", disabled: true },
      h("i.ob-spin", { "aria-hidden": "true" }),
      L("处理中", "Working"),
    );
  const link = (path, label, locked) =>
    locked
      ? h("button.ob-btn", { type: "button", disabled: true }, label)
      : h("a.ob-btn", { href: href(path), draggable: "false" }, label);
  function pressed(id, label, run, locked) {
    if (busy.has(id)) return working();
    const button = h(
      "button.ob-btn",
      { type: "button", disabled: locked },
      label,
    );
    button.addEventListener("click", run);
    return button;
  }

  // Pressing a button changes the App's state; read it again at once rather than at the next tick.
  async function act(id, run) {
    busy.add(id);
    delete problem[id];
    render();
    try {
      await run();
    } catch (error) {
      problem[id] = error.message || L("没有完成", "Did not finish");
    }
    busy.delete(id);
    await read();
  }
  const installLocalService = () => act("service", installService);
  const connectBosses = (ids) =>
    act("boss", async () => {
      const failures = [];
      for (const id of ids) {
        const result = await post("/app/hosts/connect", { id });
        if (!result.ok) failures.push(result.message);
      }
      if (failures.length) throw new Error(failures[0]);
    });

  // One row per step: its name, where it stands, and the one button that moves it on.
  const ROWS = {
    service(step) {
      const title = L("本机服务", "Local service");
      if (step.state === "busy")
        return { title, note: L("安装中…", "Installing…"), action: working() };
      if (step.state === "done")
        return { title, note: L("已安装", "Installed") };
      const failed =
        problem.service ||
        (step.state === "failed" &&
          (step.error || L("安装失败", "Installation failed")));
      const label = failed ? L("重试", "Retry") : L("安装", "Install");
      return {
        title,
        note: failed || L("未安装", "Not installed"),
        bad: Boolean(failed),
        action: pressed("service", label, installLocalService),
      };
    },
    boss(step) {
      const title = L("接入 boss", "Connect a boss");
      if (step.state === "done") return { title, note: names(step.connected) };
      const seen = step.available.length;
      const note =
        problem.boss ||
        (seen
          ? L(
              `检测到 ${names(step.available)}`,
              `Detected ${names(step.available)}`,
            )
          : L("没有检测到 boss", "No boss detected"));
      const action = step.targets.length
        ? pressed(
            "boss",
            L("接入", "Connect"),
            () => connectBosses(step.targets),
            step.locked,
          )
        : link(
            "/settings/#hosts",
            seen ? L("去接入", "Connect") : L("去看看", "Look"),
            step.locked,
          );
      return { title, note, bad: Boolean(problem.boss), action };
    },
    employees(step) {
      const title = L("确认员工", "Confirm employees");
      if (step.state === "done")
        return {
          title,
          note: L(`已确认 ${step.count} 个`, `${step.count} confirmed`),
        };
      return {
        title,
        note: L("还没有确认员工", "No employee confirmed yet"),
        action: link("/settings/#members", L("去确认", "Confirm")),
      };
    },
    models(step) {
      const title = L("选模型", "Pick models");
      if (step.state === "done")
        return { title, note: L("已指定模型", "Model picked") };
      return {
        title,
        note: L("还没有指定员工用的模型", "No model picked for employees yet"),
        action: link("/settings/#members", L("去选择", "Pick")),
      };
    },
    orchestration(step) {
      const title = L("编排", "Orchestration");
      if (step.state === "done")
        return { title, note: L("已选编排档位", "Preset chosen") };
      return {
        title,
        note: L("还没有选编排档位", "No preset chosen yet"),
        action: link(
          "/settings/#orchestration",
          L("去设置", "Set up"),
          step.locked,
        ),
      };
    },
  };

  const MARKS = { done: "✓", failed: "!" };
  function stepRow(step, index) {
    const row = ROWS[step.id](step);
    const mark =
      step.state === "busy"
        ? h("i.ob-spin", { "aria-hidden": "true" })
        : MARKS[step.state] || String(index + 1);
    return h(
      "li.ob-step",
      { "data-state": row.bad ? "failed" : step.state },
      h("span.ob-mark", {}, mark),
      h(
        "span.ob-text",
        {},
        h("span.ob-title", {}, row.title),
        h("span.ob-note", { title: row.note }, row.note),
      ),
      h("span.ob-act", {}, row.action),
    );
  }

  function render() {
    const steps = evaluate(data);
    const shown = placed && decide(steps);
    box.hidden = !shown;
    box.parentElement.classList.toggle("ob-open", shown);
    if (!shown) {
      drawn = "";
      return;
    }
    // Polls that change nothing leave the buttons alone, so a focused one keeps its focus.
    const key = JSON.stringify([
      steps,
      [...busy],
      problem,
      document.documentElement.lang,
    ]);
    if (key === drawn) return;
    drawn = key;
    const skip = h("button.ob-skip", { type: "button" }, L("跳过", "Skip"));
    skip.addEventListener("click", () => {
      over = true;
      void saveState("skipped");
      render();
    });
    box.replaceChildren(
      h(
        "div.ob-head",
        {},
        h("h2", { id: "ob-title" }, L("上手清单", "Getting started")),
        skip,
      ),
      h(
        "p.ob-about",
        {},
        L(
          "AgentNave 本身不是 Agent，也不在这里聊天。你照常在 Claude Code、Codex 等应用里和 boss 对话，boss 通过 AgentNave 的 MCP 服务派活、开会；这个 App 负责设置，并让你看到员工在做什么。",
          "AgentNave is not an agent, and you don't chat here. Keep talking to your boss in Claude Code, Codex, or another app; the boss dispatches work and runs meetings through the AgentNave MCP service, and this App sets it up and shows what the employees are doing.",
        ),
      ),
      h("ol.ob-steps", {}, ...steps.map(stepRow)),
    );
  }

  // One read at a time, so a slow answer never lands over a newer one.
  function read() {
    if (reading) {
      again = true;
      return reading;
    }
    reading = readAll().finally(() => {
      reading = null;
      if (again) {
        again = false;
        void read();
      }
    });
    return reading;
  }

  async function readAll() {
    const keys = ["status", "hosts", "members", "orchestration", "prefs"];
    const paths = [
      "/app/status",
      "/app/hosts",
      "/settings/members",
      "/app/orchestration",
      "/app/preferences",
    ];
    const answers = await Promise.allSettled(paths.map((path) => load(path)));
    // A read that failed keeps what was last known.
    answers.forEach((answer, i) => {
      if (answer.status === "fulfilled") data[keys[i]] = answer.value;
    });
    render();
  }

  addEventListener("agentnave:status", ({ detail }) => {
    const installed =
      detail?.service?.state === "done" &&
      data.status?.service?.state !== "done";
    data.status = detail;
    if (!placed) return;
    // The install finished: the hosts answer says so too, so read it instead of drawing a gap.
    if (installed) void read();
    else render();
  });

  window.AN.onboarding = {
    // The office shows the checklist only under its "no workspaces" message.
    place(on) {
      if (placed === on) return;
      placed = on;
      clearInterval(timer);
      if (on) {
        void read();
        timer = setInterval(() => {
          if (!AN.isHidden() && !busy.size) void read();
        }, POLL_MS);
      } else render();
    },
  };
})();
