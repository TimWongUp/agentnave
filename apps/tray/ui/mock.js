"use strict";
// Prototype mock backend. It serves the exact shapes of contract sections 2.1-2.3
// (/app/stage, /app/chats, /app/chats/<group>) plus the existing /app/status, driven by a
// small time-based scenario so that tails change, members finish and a manager appears.
//
// Production: this file ships only as the fallback of load(): enable it with
//   new URLSearchParams(location.search).has("mock") || location.protocol === "file:"
// The prototype keeps it always on. Query: ?scene=live|calm|review|failed|six|quiet|empty|error
// &update=available|installing|failed &service=older|upgrading|done|failed &at=<seconds into live>
// &restart (reset the scenario clock; it is shared between pages through localStorage).
(function () {
  const q = new URLSearchParams(location.search);
  const SCENE = q.get("scene") || "live";
  const enabled = () => q.has("mock") || location.protocol === "file:";
  if (!enabled()) return;

  // ---- scenario clock -------------------------------------------------------------------
  const CLOCK_KEY = "agentnave.mock.t0";
  function startTime() {
    if (q.has("at")) return Date.now() - Number(q.get("at")) * 1000;
    try {
      const saved = Number(localStorage.getItem(CLOCK_KEY));
      if (saved && !q.has("restart") && Date.now() - saved < 20 * 60e3) return saved;
      localStorage.setItem(CLOCK_KEY, String(Date.now()));
    } catch {
      /* storage may be unavailable under file: */
    }
    return Date.now();
  }
  const T0 = startTime();
  const elapsed = () => (Date.now() - T0) / 1000;

  // ---- vocabulary -----------------------------------------------------------------------
  const HOSTS = {
    claude: "Claude Code",
    codex: "Codex",
    codebuddy: "CodeBuddy",
    grok: "Grok",
    antigravity: "Antigravity",
    pi: "Pi",
  };
  const NATIVE = "原生设置";
  const keyOf = (provider, model) => `${provider}/${model || NATIVE}`;
  const FAILED = ["failed", "blocked", "interrupted"];
  const STATE_WORD = {
    failed: "失败",
    blocked: "受阻",
    cancelled: "已取消",
    interrupted: "已中断",
  };
  const plain = (text) =>
    String(text || "")
      .split("\n")
      .filter((line) => !/^\s*```/.test(line))
      .map((line) => line.replace(/^\s{0,3}(#{1,6}|>|[-*]|\d+\.)\s*/, "").replace(/(\*\*|__|`)/g, "").trim())
      .filter(Boolean);
  const clip = (text, n = 120) => (text.length > n ? text.slice(0, n - 1) + "…" : text);

  // ---- long texts (Markdown, to exercise folding and the tiny renderer) -------------------
  const OUT_CODEX = `已修复：空闲换进程现在会先确认没有活跃的 Provider 进程树。

## 改动
- \`src/agentnave/relay.py\`：\`should_swap()\` 在存在活跃 Invocation 时直接返回 \`False\`，并把等待原因写进 debug 日志。
- \`src/agentnave/relay.py\`：空闲计时从“最后一次请求结束”改为“最后一个子进程退出”。
- \`tests/test_relay_idle.py\`：新增 **test_keeps_busy_process** 与 **test_swaps_after_children_exit**。

## 验证
\`\`\`
$ uv run pytest tests/test_relay_idle.py -q
..                                                       [100%]
2 passed in 1.84s
$ uv run ruff check . && uv run pyright
All checks passed!
0 errors, 0 warnings
\`\`\`

## 风险
- 子进程被外部强杀时，空闲计时最多延后一个轮询周期（5 秒），不影响正确性。
- 没有改动 Windows 的 Job Object 路径，需要在 Windows 上另行验证。`;
  const OUT_GROK = `审查结论：共发现 3 条可能丢失活跃 Invocation 的路径，均与换进程时机有关。

1. 空闲判定只看“最后一次请求结束”，Provider 子进程仍在写文件时会被判为空闲。
2. 换进程前没有等待 supervisor 回收进程组，旧进程树可能残留一个 tick。
3. relay 重启后 \`live/\` 快照未清理，看板会短暂显示已退出的会话。`;
  const OUT_GEMINI = `发布前评审通过。三点建议：安装文档补上升级后需重启宿主一次的说明；Release 说明把托盘与服务端改动分开写；更新包签名密钥说明放进内部文档而不是公开 README。`;
  const OUT_BUDDY = `文档已更新：docs/context/architecture.md 新增“空闲换进程”一节，说明判定条件、等待上限与已知限制。`;
  const OUT_NAME = `我倾向叫“舞台”：它直观表达“主控在左、组员在右、中间是 AgentNave”，也与群聊页的“群”形成一对。\n\n风险是“舞台”暗示表演，和“节目”概念重叠；建议菜单面板叫“舞台”，App 里的会话叫“群”，节目保持原名。`;

  // ---- scenario specs ---------------------------------------------------------------------
  // One spec = one member's work inside one conversation (a task, or a seat of a show).
  // start/end are scenario seconds (negative = before the demo began); end:null runs forever.
  const spec = (provider, model, o) => ({
    provider,
    model: model || NATIVE,
    key: keyOf(provider, model),
    pace: 3,
    base: [],
    filler: (k) => `继续处理第 ${k + 1} 步…`,
    final: "succeeded",
    ...o,
  });
  const keysOf = (specs) => specs.map((s) => s.key);

  function groupA(s) {
    const codex = spec("codex", "gpt-6.1-sol", {
      start: -8, end: 40, pace: 3.4,
      prompt: "在 src/agentnave/relay.py 里找出“空闲时换新进程”的判断，保证仍有 Provider 子进程树在运行时不会被换掉，并补一个回归测试。",
      base: [
        "读取 src/agentnave/relay.py，定位空闲换进程的判断",
        "grep -n \"idle_since\" -r src → 3 处引用",
        "修改 should_swap()：有活跃 Invocation 时返回 False",
        "把空闲计时改为“最后一个子进程退出”",
        "补测试 tests/test_relay_idle.py::test_keeps_busy_process",
        "补测试 tests/test_relay_idle.py::test_swaps_after_children_exit",
        "uv run pytest tests/test_relay_idle.py -q  (2 passed)",
        "uv run ruff check . 通过，整理 diff",
      ],
      filler: (k) => `核对 diff 第 ${k - 7} 处，确认没有无关改动`,
      out: OUT_CODEX,
    });
    const grok = spec("grok", "grok-5-fast", {
      start: -4, end: 70, pace: 4,
      prompt: "只读审查 relay 的换进程流程，列出可能丢失活跃 Invocation 的路径，不要改代码。",
      base: [
        "阅读 relay.py 的 swap 流程",
        "追踪 supervisor 回收进程组的调用顺序",
        "路径 1：空闲判定只看最后一次请求结束",
        "路径 2：换进程前没有等待进程组回收",
      ],
      filler: (k) => `核对 live/ 快照清理时机（第 ${k - 3} 次读取）`,
      out: OUT_GROK,
    });
    const buddy = spec("codebuddy", "glm-5.1", {
      start: 22, end: 95, pace: 3.2,
      prompt: "把“空闲换进程”的判定条件、等待上限和已知限制补进 docs/context/architecture.md。",
      base: [
        "读取 docs/context/architecture.md 现有章节",
        "起草“空闲换进程”小节",
        "补充等待上限与已知限制",
      ],
      filler: (k) => `润色第 ${k - 2} 段表述，保持与 ADR-0010 一致`,
      out: OUT_BUDDY,
    });
    const gem = spec("antigravity", "gemini-3.5-pro", {
      start: -3700, end: -3480, label: "评审员", final: "succeeded", out: OUT_GEMINI,
    });
    const pi = spec("pi", null, {
      start: -3700, end: -3300, label: "挑刺者", final: "failed",
      err: "Provider 进程 600 秒无输出，已超时",
    });
    const c2 = {
      id: "20261017-0830-f1a2b3", title: "v0.20 发布前评审", mode: "show",
      topics: [{ at: -3700, text: "v0.20 准备发布：请各自从自己的角度给出发布前最后的风险与建议，三句话以内。", to: keysOf([gem, pi]) }],
      specs: [gem, pi],
    };
    const c1 = { id: "20261017-0912-a1b2c3", title: "relay 空闲换进程：保留活跃子进程树", mode: "task", specs: [codex, grok] };
    const c3 = { id: "20261017-0958-c3d4e5", title: "补 relay 文档", mode: "task", specs: [buddy] };
    return {
      group: "3f9c1d7a5b2e4c808f6d9a1b7e2c4d50", host: "claude", color: 0, live: true,
      convs: [c2, c1, c3],
    };
  }

  function groupB(s, decided) {
    const grok = spec("grok", "grok-5", {
      start: -900, end: -420, label: "挑刺者", out: OUT_NAME,
    });
    const claude = spec("claude", "opus-5", {
      start: -900, end: -30, label: "产品经理", final: decided ? "succeeded" : "review",
      out: "我的结论：叫“舞台”。理由三条：与产品形态一致、与“群”配对、用户无需学习新词。",
    });
    const gem = spec("antigravity", "gemini-3.5-pro", {
      start: -20, end: 15, pace: 3, label: "设计师", final: decided ? "succeeded" : "review",
      base: ["先看三个候选名在菜单栏里的字宽", "“舞台”两个字最稳，比“指挥台”短", "整理理由：一是辨识度，二是与图标搭配"],
      filler: (k) => `补充第 ${k - 2} 条理由…`,
      out: "我选“舞台”。它两字、好认，放在菜单栏里不拥挤。",
    });
    const pi = spec("pi", null, { start: -5200, end: -5000, final: "succeeded", out: "安装文档已逐条核对，没有发现与 v0.20 不一致的命令。", prompt: "逐条核对安装文档里的命令是否与 v0.20 一致，只报告差异。" });
    const show = {
      id: "20261017-1021-aabbcc", title: "舞台面板命名与边界", mode: "show",
      topics: [{ at: -900, text: "我们要给新的菜单面板起名，并划清它和群聊页的边界。请各自给出你认为最合适的名字，并说明理由与风险。", to: keysOf([grok, claude, gem]) }],
      specs: [grok, claude, gem],
    };
    const task = { id: "20261017-0702-ddeeff", title: "核对安装文档", mode: "task", specs: [pi] };
    return { group: "b81e46f0c93d4a2f9e51d07c6a3b8e12", host: "codex", color: 3, live: true, convs: [task, show] };
  }

  function groupC() {
    const codex = spec("codex", "gpt-6.1-sol", {
      start: -4300, end: -4200, prompt: "清理 Studio 目录里超过 30 天的 live 快照，只列出将要删除的文件，不要真的删除。",
      out: "将删除 12 个文件，共 38 KB，均为已退出进程遗留的 live 快照。清单见下。",
    });
    return {
      group: "c-20261017-0745-d4e5f6", host: null, color: 7, live: false,
      convs: [{ id: "20261017-0745-d4e5f6", title: "清理 Studio 旧日志", mode: "task", specs: [codex] }],
    };
  }

  function groupD(failed) {
    const codex = spec("codex", "gpt-6.1-sol", {
      start: 30, end: 110, pace: 3.6, final: failed ? "failed" : "succeeded",
      err: "测试未通过：test_settings_toggle 断言失败",
      prompt: "给设置页的 Provider 列表加一个启用开关，改动只限 apps/tray/ui/settings.*。",
      base: [
        "读取 settings.html / settings.js 的 Provider 列表渲染",
        "新增 toggle 控件并绑定 /app/settings 写入",
        "更新 settings.css 的开关样式（深浅色）",
        "cargo test 通过，运行 uv run pytest -q",
      ],
      filler: (k) => `运行第 ${k - 3} 组测试，等待结果…`,
      out: "已完成：Provider 列表每行新增启用开关，状态通过 /app/settings 持久化，深浅色均已核对。",
    });
    const pi = spec("pi", null, {
      start: 34, end: 62, final: "blocked", pace: 4,
      err: "需要确认写入 ~/.config 的权限，已暂停",
      prompt: "核对设置页保存路径是否会写出项目目录之外。",
      base: ["检查保存路径解析", "发现需要写入 ~/.config/agentnave"],
    });
    return {
      group: "9d2a07e5c1f84b3aa6e0f4d812c7b953", host: "grok", color: 5, live: true,
      convs: [{ id: "20261017-1040-9a8b7c", title: "给设置页加 Provider 开关", mode: "task", specs: [codex, pi] }],
    };
  }

  // The "six" scene: six managers, one with eight members, to exercise folding and the cap.
  function sixGroups() {
    const POOL = {
      claude: ["opus-5", "sonnet-5.5", "haiku-5"],
      codex: ["gpt-6.1-sol", "gpt-6-mini"],
      grok: ["grok-5", "grok-5-fast"],
      antigravity: ["gemini-3.5-pro", "gemini-3.5-flash"],
      codebuddy: ["glm-5.1", "deepseek-v4"],
      pi: [null, "qwen-4-max"],
    };
    const plan = [
      ["claude", 0, 0, ["running", "running", "running", "review", "succeeded", "succeeded", "succeeded", "failed"]],
      ["codex", 1, 1, ["running", "succeeded"]],
      ["codebuddy", 2, 2, ["review"]],
      ["grok", 3, 3, ["succeeded", "succeeded", "succeeded"]],
      ["pi", 4, 4, ["failed"]],
      ["antigravity", 6, 6, ["succeeded", "succeeded"]],
    ];
    const providers = ["codex", "grok", "antigravity", "codebuddy", "pi", "claude"];
    return plan.map(([host, color, n, states], gi) => {
      const specs = states.map((state, i) => {
        const provider = providers[(i + gi) % providers.length];
        const models = POOL[provider];
        const model = models[Math.floor(i / providers.length) % models.length];
        const running = state === "running";
        return spec(provider, model, {
          start: -20 - i * 5, end: running ? null : -60 - gi * 400 - i * 30,
          final: state === "running" ? "succeeded" : state,
          err: "Provider 报告：上下文超过上限", pace: 3 + (i % 3),
          prompt: `第 ${i + 1} 项：${["检查迁移脚本", "审阅接口变更", "补充回归测试", "整理发布说明", "复核安全边界", "统计耗时", "核对文档", "清理日志"][i]}。`,
          base: [`读取相关文件（${i + 1}）`, `分析调用关系（${i + 1}）`],
          filler: (k) => `处理第 ${k - 1} 个文件…`,
          out: `完成：${["迁移脚本无需改动", "接口变更向后兼容", "新增 3 个回归测试", "发布说明已整理", "边界检查通过", "耗时统计已附", "文档已核对", "日志已清理"][i]}。`,
        });
      });
      return {
        group: `${String(gi + 1).repeat(32).slice(0, 32)}`, host, color, live: gi % 2 === 0 || states.includes("running"),
        convs: [{ id: `20261017-11${gi}0-${String(gi).repeat(6)}`, title: `${HOSTS[host]} 会话 ${gi + 1}`, mode: "task", specs }],
      };
    });
  }

  // ---- evaluation ----------------------------------------------------------------------
  function world() {
    const live = elapsed();
    switch (SCENE) {
      case "calm": return { s: 900, shift: 0, groups: [groupA(), groupB(0, true), groupC()] };
      case "review": return { s: 900, shift: 0, groups: [groupB(0, false), groupC(), groupA()] };
      case "failed": return { s: 900, shift: 0, groups: [groupA(), groupC(), groupD(true)] };
      case "quiet": return { s: 900, shift: 10800, groups: [groupA(), groupB(0, true), groupC()] };
      case "empty": case "error": return { s: 0, shift: 0, groups: [] };
      case "six": return { s: live, shift: 0, groups: sixGroups() };
      default: {
        const groups = [groupA(), groupB(0, false), groupC()];
        if (live >= 30) groups.push(groupD(false));
        return { s: live, shift: 0, groups };
      }
    }
  }

  const stamp = (w, sec) => new Date(T0 + (sec - w.shift) * 1000).toISOString();
  const nowIso = () => new Date().toISOString();

  function evalSpec(sp, w) {
    if (w.s < sp.start) return null;
    const running = sp.end == null || w.s < sp.end;
    const n = running ? 1 + Math.floor((w.s - sp.start) / sp.pace) : 0;
    const lines = [];
    for (let k = Math.max(0, n - 3); k < n; k++) lines.push(k < sp.base.length ? sp.base[k] : sp.filler(k));
    return { ...sp, status: running ? "running" : sp.final, lines, at: running ? w.s : sp.end };
  }

  const lineOf = (e) => {
    if (e.status === "running") return clip(plain(e.lines[e.lines.length - 1]).join(" "));
    if (e.status === "review") return "待审核：请在主控中决定";
    if (e.status === "succeeded") return clip(plain(e.out)[0] || "");
    return clip(`${STATE_WORD[e.status] || e.status}：${e.err || ""}`);
  };
  const URGENCY = { running: 0, review: 1 };
  const memberItem = (e, w, conv) => ({
    key: e.key, provider: e.provider, model: e.model, status: e.status, line: lineOf(e),
    updated: e.status === "running" ? nowIso() : stamp(w, e.at), conversation: conv.id,
  });
  const byUrgency = (a, b) =>
    (URGENCY[a.status] ?? 2) - (URGENCY[b.status] ?? 2) || Date.parse(b.updated) - Date.parse(a.updated);

  // Merge the group's tasks/seats by provider/model, newest activity wins (contract 2.1).
  function membersOf(g, w) {
    const merged = new Map();
    for (const conv of g.convs)
      for (const sp of conv.specs) {
        const e = evalSpec(sp, w);
        if (!e) continue;
        const old = merged.get(e.key);
        if (!old || e.at >= old.e.at) merged.set(e.key, { e, conv });
      }
    return [...merged.values()].map(({ e, conv }) => memberItem(e, w, conv)).sort(byUrgency);
  }
  function statusOf(g, w, members) {
    if (members.some((m) => m.status === "running")) return "running";
    if (members.some((m) => m.status === "review")) return "review";
    // Only the newest finished member decides "failed": an old failure must not stick.
    const latest = [...members].sort((a, b) => Date.parse(b.updated) - Date.parse(a.updated))[0];
    return latest && FAILED.includes(latest.status) ? "failed" : "idle";
  }
  const label = (g) => (g.host ? HOSTS[g.host] : "未知主控");
  const convStatus = (conv, w) => {
    const es = conv.specs.map((sp) => evalSpec(sp, w)).filter(Boolean);
    if (es.some((e) => e.status === "running")) return "running";
    if (es.some((e) => e.status === "review")) return "review";
    const last = es.sort((a, b) => b.at - a.at)[0];
    if (!last) return "idle";
    if (conv.mode === "show") return FAILED.includes(last.status) ? "failed" : "finished";
    return last.status;
  };
  const summaryOf = (g, w) => {
    const members = membersOf(g, w);
    const updated = members.length
      ? members.reduce((m, x) => (Date.parse(x.updated) > m ? Date.parse(x.updated) : m), 0)
      : T0;
    return { members, status: statusOf(g, w, members), updated: new Date(updated).toISOString() };
  };

  // ---- the three endpoints --------------------------------------------------------------
  function stage() {
    const w = world();
    const WINDOW = 2 * 3600e3;
    const managers = w.groups
      .map((g) => ({ g, ...summaryOf(g, w) }))
      .filter(({ members, status, updated }) => members.length && (["running", "review"].includes(status) || Date.now() - Date.parse(updated) < WINDOW))
      .sort((a, b) => (URGENCY[a.status] ?? 2) - (URGENCY[b.status] ?? 2) || Date.parse(b.updated) - Date.parse(a.updated))
      .slice(0, 6)
      .map(({ g, members, status, updated }) => ({
        group: g.group, host: g.host, label: label(g), color: g.color, live: g.live, status, updated, members,
      }));
    return {
      version: 1,
      counts: {
        running: managers.reduce((n, m) => n + m.members.filter((x) => x.status === "running").length, 0),
        review: managers.reduce((n, m) => n + m.members.filter((x) => x.status === "review").length, 0),
      },
      managers,
      empty: managers.length ? null : SCENE === "empty" ? "no_data" : "quiet",
    };
  }

  function messagesOf(g, w) {
    const manager = { role: "manager", provider: g.host, model: null, label: label(g) };
    const speakerOf = (e) => ({ role: "member", provider: e.provider, model: e.model, label: e.label || e.model });
    const out = [];
    const typing = [];
    for (const conv of g.convs) {
      for (const [i, t] of (conv.topics || []).entries())
        if (w.s >= t.at)
          out.push({ id: `t:${conv.id}:${i}`, time: stamp(w, t.at), conversation: conv.id, kind: "prompt", speaker: manager, to: t.to, text: t.text, status: "succeeded", detail: null });
      for (const sp of conv.specs) {
        const e = evalSpec(sp, w);
        if (!e) continue;
        if (conv.mode === "task" && sp.prompt)
          out.push({ id: `p:${conv.id}:${e.key}`, time: stamp(w, sp.start), conversation: conv.id, kind: "prompt", speaker: manager, to: [e.key], text: sp.prompt, status: "succeeded", detail: null });
        const base = { time: stamp(w, e.at), conversation: conv.id, speaker: speakerOf(e), to: [] };
        if (e.status === "running")
          typing.push({ ...base, id: e.key, time: nowIso(), kind: "typing", text: e.lines.join("\n"), status: "running", detail: null, _s: sp.start });
        else if (e.status === "succeeded")
          out.push({ ...base, id: `r:${conv.id}:${e.key}`, kind: "reply", text: e.out, status: "succeeded", detail: null });
        else if (e.status === "review")
          out.push({ ...base, id: `n:review:${conv.id}:${e.key}`, kind: "notice", text: "待审核，请在主控中决定", status: "review", detail: null });
        else
          out.push({ ...base, id: `n:${conv.id}:${e.key}`, kind: "notice", text: conv.mode === "show" ? "本轮缺席" : STATE_WORD[e.status] || e.status, status: e.status, detail: e.err || null });
      }
    }
    out.sort((a, b) => Date.parse(a.time) - Date.parse(b.time));
    typing.sort((a, b) => a._s - b._s).forEach((m) => delete m._s);
    return [...out, ...typing].slice(-300);
  }

  function chats() {
    const w = world();
    const rows = w.groups
      .map((g) => ({ g, ...summaryOf(g, w) }))
      .filter((r) => r.members.length)
      .map(({ g, status, updated }) => {
        const msgs = messagesOf(g, w);
        const last = msgs[msgs.length - 1];
        const conv = [...g.convs].sort((a, b) => Math.max(...b.specs.map((s) => s.end ?? w.s)) - Math.max(...a.specs.map((s) => s.end ?? w.s)))[0];
        const preview = last ? clip((last.kind === "typing" ? plain(last.text).slice(-1) : plain(last.text).slice(0, 1)).join(" "), 90) : "";
        return {
          group: g.group, host: g.host, label: label(g), color: g.color, live: g.live, status, updated,
          title: conv.title, conversations: g.convs.length, preview,
        };
      })
      .sort((a, b) => (URGENCY[a.status] ?? 2) - (URGENCY[b.status] ?? 2) || Date.parse(b.updated) - Date.parse(a.updated));
    return { groups: rows };
  }

  function chat(group) {
    const w = world();
    const g = w.groups.find((x) => x.group === group);
    if (!g || !summaryOf(g, w).members.length) throw Object.assign(new Error("404"), { status: 404 });
    const { members, status } = summaryOf(g, w);
    return {
      group: g.group, host: g.host, label: label(g), color: g.color, live: g.live, status,
      conversations: g.convs
        .filter((c) => c.specs.some((sp) => w.s >= sp.start))
        .map((c) => ({ id: c.id, title: c.title, mode: c.mode, status: convStatus(c, w), director_url: `/c/${c.id}/` })),
      members,
      messages: messagesOf(g, w),
    };
  }

  function status() {
    const update = q.get("update");
    const service = q.get("service");
    return {
      version: "0.21.0",
      update: update ? { state: update, version: "0.21.0" } : { state: "current" },
      service: service ? { state: service, version: service === "older" ? "0.19.2" : "0.21.0", error: "uv tool install 失败：无法连接 github.com" } : null,
    };
  }

  async function mockFetch(path) {
    await Promise.resolve();
    if (SCENE === "error") throw Object.assign(new Error("500"), { status: 500 });
    if (path === "/app/stage") return stage();
    if (path === "/app/chats") return chats();
    if (path === "/app/status") return status();
    const m = /^\/app\/chats\/([^/?#]+)$/.exec(path);
    if (m) return chat(decodeURIComponent(m[1]));
    throw Object.assign(new Error("404"), { status: 404 });
  }

  // Preview parameters follow chat links; normal app links retain their production routes.
  const keep = ["scene", "update", "service"].filter((k) => q.has(k)).map((k) => `${k}=${encodeURIComponent(q.get(k))}`);
  function href(path) {
    const match = /^\/chat\/?([^/?#]*)(#.*)?$/.exec(path);
    if (!match) return path;
    const params = ["mock", ...keep];
    if (location.protocol === "file:") {
      if (match[1]) params.push(`group=${encodeURIComponent(match[1])}`);
      return `chat.html?${params.join("&")}${match[2] || ""}`;
    }
    return `/chat/${match[1]}?${params.join("&")}${match[2] || ""}`;
  }

  window.AgentNaveMock = { enabled, fetch: mockFetch, href, scene: SCENE };
})();
