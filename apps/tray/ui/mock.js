"use strict";
// Prototype mock backend. It serves the exact shapes of contract sections 2.1-2.3
// (/app/stage, /app/chats, /app/chats/<group>) plus the existing /app/status, driven by a
// small time-based scenario so that tails change, members finish and a manager appears.
//
// Production: this file ships only as the fallback of load(): enable it with
//   new URLSearchParams(location.search).has("mock") || location.protocol === "file:"
// The prototype keeps it always on. Query: ?scene=live|calm|review|failed|six|quiet|empty|error
// &update=available|installing|failed &service=older|upgrading|done|failed &at=<seconds into live>
// &stale (the last check is too old) &fail=download|service|app (the update stops at that step)
// &restart (reset the scenario clock; it is shared between pages through localStorage).
(function () {
  const q = new URLSearchParams(location.search);
  const hostScene = q.get("hosts") || "typical";
  const SCENE = q.get("scene") || (hostScene === "fresh" ? "empty" : "live");
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
    "claude-desktop": "Claude 桌面",
    codex: "Codex",
    codebuddy: "CodeBuddy",
    grok: "Grok",
    antigravity: "Antigravity",
    pi: "Pi",
    deepseek: "DeepSeek Harness",
  };
  const NATIVE = "";
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
  const OUT_NAME = `我倾向叫“舞台”：它直观表达“boss 在左、组员在右、中间是 AgentNave”，也与办公室页的“工作区”形成一对。\n\n风险是“舞台”暗示表演，和“节目”概念重叠；建议菜单面板叫“舞台”，App 里的会话叫“工作区”，会议保持原名。`;

  // ---- scenario specs ---------------------------------------------------------------------
  // One spec = one member's work inside one conversation (a task, or a seat of a show).
  // start/end are scenario seconds (negative = before the demo began); end:null runs forever.
  const deskKey = (provider, model, role) => `${provider}|${model || NATIVE}|${role || ""}`;
  const deskLabel = (provider, model, role) => [model || "未记录模型", HOSTS[provider] || provider, role].filter(Boolean).join(" · ");
  const spec = (provider, model, o) => {
    const s = {
      provider,
      label: HOSTS[provider] || provider,
      model: model || NATIVE,
      key: keyOf(provider, model),
      role: null,
      pace: 3,
      base: [],
      filler: (k) => `继续处理第 ${k + 1} 步…`,
      final: "succeeded",
      ...o,
    };
    s.desk = deskKey(provider, s.model, s.role);
    s.deskLabel = deskLabel(provider, s.model, s.role);
    return s;
  };
  const keysOf = (specs) => specs.map((s) => s.key);

  function groupA() {
    const codex = spec("codex", "gpt-6.1-sol", {
      role: "实现者", access: "write", start: -8, end: 40, pace: 3.4,
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
      role: "文档", start: 22, end: 95, pace: 3.2,
      prompt: "把“空闲换进程”的判定条件、等待上限和已知限制补进 docs/context/architecture.md。",
      base: [
        "读取 docs/context/architecture.md 现有章节",
        "起草“空闲换进程”小节",
        "补充等待上限与已知限制",
      ],
      filler: (k) => `润色第 ${k - 2} 段表述，保持与 ADR-0010 一致`,
      out: OUT_BUDDY,
    });
    // Two rooms: a finished release review, and a running design discussion with a pending review.
    const gem = spec("antigravity", "gemini-3.5-pro", { role: "评审员", access: "read", start: -3700, end: -3480, out: OUT_GEMINI });
    const pi = spec("pi", null, { role: "挑刺者", start: -3700, end: -3300, final: "failed", err: "Provider 进程 600 秒无输出，已超时" });
    const room1 = {
      id: "20261017-0830-f1a2b3", title: "v0.20 发布前评审", mode: "show",
      topics: [{ at: -3700, text: "v0.20 准备发布：请各自从自己的角度给出发布前最后的风险与建议，三句话以内。", to: keysOf([gem, pi]) }],
      specs: [gem, pi],
    };
    const reviewer = spec("codex", "gpt-6.1-sol", {
      role: "审查员", access: "read", start: -30, end: 60, pace: 3.6,
      base: ["对照实现者的补丁，逐条检查换进程的边界", "关注 Windows Job Object 路径是否受影响"],
      filler: (k) => `核对第 ${k - 1} 条边界条件…`,
      out: "我认为补丁本身正确，但“最后一个子进程退出”的判定需要加一个上限，避免孤儿进程让空闲计时永远不开始。",
    });
    const grokSeat = spec("grok", "grok-5-fast", {
      start: -20, end: 48, pace: 4,
      base: ["回顾上一轮审查的三条路径", "整理对补丁的反对意见"],
      filler: (k) => `补充第 ${k - 1} 条意见…`,
      out: "同意加上限；另外建议换进程前显式等待 supervisor 回收进程组。",
    });
    const gemSeat = spec("antigravity", "gemini-3.5-pro", {
      role: "评审员", access: "read", start: -10, end: 12, pace: 3, final: "review",
      base: ["从发布节奏看这个补丁的风险", "评估是否需要写进 Release 说明"],
      out: "建议作为 patch 版本发布，Release 说明里单列“换进程更稳”一条即可。",
    });
    const pm = spec("claude", "opus-5", {
      role: "产品经理", start: -600, end: -300, out: "用户可见的变化只有一点：长任务期间不会再被意外打断。文案里不需要出现“换进程”。",
    });
    // A DeepSeek desk that finished a private task walks over to the meeting, then back.
    const ds = spec("deepseek", "deepseek-v4", { role: "资料员", access: "read", start: -900, end: -820, out: "已整理 relay 相关的三次历史改动，清单如下。" });
    const dsSeat = spec("deepseek", "deepseek-v4", {
      role: "资料员", access: "read", start: 8, end: 30, pace: 3,
      base: ["翻出上次换进程失败的日志", "核对补丁是否覆盖那次的场景"],
      filler: (k) => `补充第 ${k - 1} 条记录…`,
      out: "上次失败的场景补丁已经覆盖，日志对得上。",
    });
    const room2 = {
      id: "20261018-0905-c5d6e7", title: "relay 修复方案讨论", mode: "show",
      topics: [{ at: -620, text: "实现者给出了 relay 补丁。请各自从自己的角色出发评价这个方案，指出风险。", to: keysOf([pm]) }, { at: -30, text: "请审查员与评审员继续看补丁，其余人补充。", to: keysOf([reviewer, grokSeat, gemSeat]) }, { at: 8, text: "请资料员核对历史记录。", to: keysOf([dsSeat]) }],
      forwards: [{ at: -400, text: "（转发的评审结论）三点建议：安装文档补上升级后需重启宿主一次的说明；Release 说明把托盘与服务端改动分开写；更新包签名密钥说明放进内部文档而不是公开 README。", source: gem.deskLabel, from: room1, spec: gem }],
      specs: [reviewer, grokSeat, gemSeat, pm, dsSeat],
    };
    const c1 = { id: "20261017-0912-a1b2c3", title: "relay 空闲换进程：保留活跃子进程树", mode: "task", specs: [codex] };
    const c4 = { id: "20261017-0915-b2c3d4", title: "审查 relay 换进程流程", mode: "task", specs: [grok] };
    const c3 = { id: "20261017-0958-c3d4e5", title: "补 relay 文档", mode: "task", specs: [buddy] };
    const c5 = { id: "20261017-0930-d4e5f6", title: "整理 relay 历史改动", mode: "task", specs: [ds] };
    return {
      group: "3f9c1d7a5b2e4c808f6d9a1b7e2c4d50", host: "claude", app: "claude-desktop", color: 0, live: true,
      convs: [room1, c1, c4, room2, c3, c5],
    };
  }

  function groupB(s, decided) {
    const grok = spec("grok", "grok-5", {
      role: "挑刺者", start: -900, end: -420, out: OUT_NAME,
    });
    const claude = spec("claude", "opus-5", {
      role: "产品经理", start: -900, end: -30, final: decided ? "succeeded" : "review",
      out: "我的结论：叫“舞台”。理由三条：与产品形态一致、与“群”配对、用户无需学习新词。",
    });
    const gem = spec("antigravity", "gemini-3.5-pro", {
      role: "设计师", start: -20, end: 15, pace: 3, final: decided ? "succeeded" : "review",
      base: ["先看三个候选名在菜单栏里的字宽", "“舞台”两个字最稳，比“指挥台”短", "整理理由：一是辨识度，二是与图标搭配"],
      filler: (k) => `补充第 ${k - 2} 条理由…`,
      out: "我选“舞台”。它两字、好认，放在菜单栏里不拥挤。",
    });
    const pi = spec("pi", null, { start: -5200, end: -5000, final: "succeeded", out: "安装文档已逐条核对，没有发现与 v0.20 不一致的命令。", prompt: "逐条核对安装文档里的命令是否与 v0.20 一致，只报告差异。" });
    const show = {
      id: "20261017-1021-aabbcc", title: "舞台面板命名与边界", mode: "show",
      topics: [{ at: -900, text: "我们要给新的菜单面板起名，并划清它和办公室页的边界。请各自给出你认为最合适的名字，并说明理由与风险。", to: keysOf([grok, claude, gem]) }],
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
    if (e.status === "review") return "待审核：请在 boss 中决定";
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
  const label = (g) => (g.host ? HOSTS[g.app || g.host] : "未记录 boss");
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
    // Same order as the service: work going on, then failures, then the most recent; at most five.
    const TIER = { running: 0, review: 0, failed: 1 };
    const managers = w.groups
      .map((g) => ({ g, ...summaryOf(g, w) }))
      .filter(({ members }) => members.length)
      .sort((a, b) => (TIER[a.status] ?? 2) - (TIER[b.status] ?? 2) || Date.parse(b.updated) - Date.parse(a.updated))
      .slice(0, 5)
      .map(({ g, members, status, updated }) => ({
        group: g.group, host: g.host, app: g.app || null, label: label(g), color: g.color, live: g.live, status, updated, members,
        ...limited(desksOf(g, w), true), rooms: roomsOf(g, w),
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

  // ---- office projection: desks, rooms and message fields (contract: GET /app/chats/<group>) ----
  const seatOf = (conv, sp) => sp.seat || `seat-${conv.specs.indexOf(sp) + 1}`;
  const midOf = (conv, sp) => `m-${conv.id.slice(-6)}-${conv.specs.indexOf(sp)}`;
  const isActive = (status) => status === "running" || status === "review";
  const office = (msg, { visibility, thread, desk = null, message_id = null, forward = null }) =>
    Object.assign(msg, { visibility, thread, desk, message_id, forward });
  const recipientOf = (conv, sp) => ({ key: sp.key, provider: sp.provider, model: sp.model, label: sp.label, seat: seatOf(conv, sp) });

  function messagesOf(g, w) {
    const manager = { role: "manager", provider: g.host, model: null, label: label(g) };
    const speakerOf = (e) => ({ role: "member", provider: e.provider, model: e.model, label: e.label || e.model });
    const out = [];
    const typing = [];
    for (const conv of g.convs) {
      const show = conv.mode === "show";
      const room = { kind: "room", id: conv.id };
      for (const [i, t] of (conv.topics || []).entries())
        if (w.s >= t.at) {
          const targets = conv.specs.filter((sp) => t.to.includes(sp.key));
          out.push(office(
            { id: `t:${conv.id}:${i}`, time: stamp(w, t.at), conversation: conv.id, kind: "prompt", speaker: manager, to: t.to, recipients: targets.map((sp) => recipientOf(conv, sp)), text: t.text, status: "succeeded", detail: null },
            { visibility: "public", thread: room, desk: targets.length === 1 ? targets[0].desk : null },
          ));
        }
      for (const f of conv.forwards || [])
        if (w.s >= f.at)
          out.push(office(
            { id: `f:${conv.id}:${f.at}`, time: stamp(w, f.at), conversation: conv.id, kind: "forward", speaker: { role: "manager", provider: g.host, model: null, label: "boss 转发" }, to: [], text: f.text, status: "succeeded", detail: null },
            { visibility: "public", thread: room, message_id: `m-fw-${Math.abs(f.at)}`, forward: { message_id: midOf(f.from, f.spec), source: f.source } },
          ));
      for (const sp of conv.specs) {
        const e = evalSpec(sp, w);
        if (!e) continue;
        const desk = sp.desk;
        const thread = show ? room : { kind: "desk", key: desk };
        const mid = midOf(conv, sp);
        if (!show && sp.prompt)
          out.push(office(
            { id: `p:${conv.id}:${e.key}`, time: stamp(w, sp.start), conversation: conv.id, kind: "prompt", speaker: manager, to: [e.key], recipients: [recipientOf(conv, sp)], text: sp.prompt, status: "succeeded", detail: null },
            { visibility: "private", thread, desk, message_id: mid },
          ));
        const base = { time: stamp(w, e.at), conversation: conv.id, speaker: speakerOf(e), to: [] };
        if (e.status === "running")
          typing.push(office(
            { ...base, id: `ty:${conv.id}:${e.key}`, time: nowIso(), kind: "typing", text: e.lines.join("\n"), status: "running", detail: null, _s: sp.start },
            { visibility: "private", thread, desk, message_id: show ? null : mid },
          ));
        else if (e.status === "succeeded")
          out.push(office(
            { ...base, id: `r:${conv.id}:${e.key}:${desk}`, kind: "reply", text: e.out, status: "succeeded", detail: null },
            { visibility: show ? "public" : "private", thread, desk, message_id: mid },
          ));
        else if (e.status === "review")
          out.push(office(
            { ...base, id: `n:review:${conv.id}:${e.key}`, kind: "notice", text: "待审核，请在 boss 中决定", status: "review", detail: null },
            { visibility: "private", thread, desk },
          ));
        else
          out.push(office(
            { ...base, id: `n:${conv.id}:${e.key}:${desk}`, kind: "notice", text: show ? "本轮缺席" : STATE_WORD[e.status] || e.status, status: e.status, detail: e.err || null },
            { visibility: show ? "public" : "private", thread, desk, message_id: show ? null : mid },
          ));
      }
    }
    out.sort((a, b) => Date.parse(a.time) - Date.parse(b.time));
    typing.sort((a, b) => a._s - b._s).forEach((m) => delete m._s);
    return [...out, ...typing].slice(-300);
  }

  // The running limit caps drawn desks (?limit=N previews "已优化"); busy desks always show.
  function limited(all, countOnly = false) {
    const limit = Number(q.get("limit") || 8);
    const busy = (d) => d.status === "running";
    const order = [...all].sort((a, b) => Number(busy(b)) - Number(busy(a)) || Date.parse(b.last_active) - Date.parse(a.last_active));
    const keep = new Set(order.slice(0, Math.max(limit, all.filter(busy).length)).map((d) => d.key));
    const rest = order.filter((d) => !keep.has(d.key));
    return {
      desks: all.filter((d) => keep.has(d.key)),
      optimized: countOnly ? { count: rest.length } : { count: rest.length, desks: rest },
    };
  }
  function desksOf(g, w) {
    const hostCli = g.host === "workbuddy" ? "codebuddy" : g.host;
    const map = new Map();
    for (const conv of g.convs)
      for (const sp of conv.specs) {
        const e = evalSpec(sp, w);
        if (!e) continue;
        const d = map.get(sp.desk) || { sp, first: sp.start, last: e.at, active: false, latest: null, threads: 0, running: 0, meeting: null };
        map.set(sp.desk, d);
        d.first = Math.min(d.first, sp.start);
        d.last = Math.max(d.last, e.at);
        if (isActive(e.status)) d.active = true;
        else if (!d.latest || e.at >= d.latest.at) d.latest = { at: e.at, status: e.status };
        if (conv.mode === "task") {
          d.threads++;
          if (e.status === "running") d.running++;
        } else if (isActive(e.status) && (!d.meeting || e.at >= d.meeting.at)) d.meeting = { at: e.at, room: conv.id, seat: seatOf(conv, sp), status: e.status };
      }
    return [...map.values()]
      .sort((a, b) => b.first - a.first || (a.sp.desk < b.sp.desk ? -1 : 1))
      .map((d) => ({
        key: d.sp.desk, provider: d.sp.provider, model: d.sp.model, role: d.sp.role, access: d.sp.access ?? null, label: d.sp.deskLabel,
        first_seen: stamp(w, d.first), last_active: d.active ? nowIso() : stamp(w, d.last),
        status: d.active ? "running" : FAILED.includes(d.latest?.status) ? "failed" : "idle",
        private_allowed: d.sp.provider !== hostCli,
        kind: "task", room: null,
        private: { tasks: d.threads, running: d.running },
        meeting: d.meeting ? { room: d.meeting.room, seat: d.meeting.seat, status: d.meeting.status } : { room: null, seat: null, status: null },
      }));
  }

  function roomsOf(g, w) {
    return g.convs
      .filter((conv) => conv.mode === "show" && conv.specs.some((sp) => w.s >= sp.start))
      .map((conv) => {
        const es = conv.specs.map((sp) => evalSpec(sp, w)).filter(Boolean);
        const last = [...es].sort((a, b) => b.at - a.at)[0];
        const status = es.some((e) => e.status === "running") ? "running" : es.some((e) => e.status === "review") ? "review" : last && FAILED.includes(last.status) ? "failed" : "idle";
        const board = (conv.topics || []).filter((t) => w.s >= t.at).length + (conv.forwards || []).filter((f) => w.s >= f.at).length + es.filter((e) => e.status === "succeeded").length;
        return {
          id: conv.id, title: conv.title, status,
          seats: conv.specs.map((sp) => ({ seat: seatOf(conv, sp), desk: sp.desk })),
          pending_review: es.filter((e) => e.status === "review").length,
          board,
        };
      });
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
          group: g.group, host: g.host, app: g.app || null, label: label(g), color: g.color, live: g.live, status, updated,
          title: conv.title, conversations: g.convs.length, preview,
          desks_count: desksOf(g, w).length, rooms_count: roomsOf(g, w).length,
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
      group: g.group, host: g.host, app: g.app || null, label: label(g), color: g.color, live: g.live, status,
      conversations: g.convs
        .filter((c) => c.specs.some((sp) => w.s >= sp.start))
        .map((c) => ({ id: c.id, title: c.title, mode: c.mode, status: convStatus(c, w) })),
      members,
      ...limited(desksOf(g, w)),
      rooms: roomsOf(g, w),
      messages: messagesOf(g, w),
    };
  }

  let mockVersion = "0.21.0", updateOverride = null, mockElapsed = () => 0;
  const NOTES = { link: null, items: [
    { version: "0.21.1", notes: "## 变更\n\n- 更新时先看更新内容，确认后一次完成本机服务和 App 的更新。\n- 修复 `wait` 偶尔漏报的问题。" },
  ] };
  function status() {
    const update = q.get("update");
    const service = q.get("service");
    return {
      version: mockVersion,
      update: updateOverride || (update ? { state: update, version: "0.21.1", notes: NOTES, step: update === "installing" ? "service" : null, ...(update === "failed" ? { step: "service", error: "本机服务升级失败，App 未更新：无法连接 github.com" } : {}) } : { state: "current" }),
      service: service ? { state: service === "older" ? "outdated" : service, version: service === "older" ? "0.19.2" : mockVersion, error: service === "failed" ? "uv tool install 失败：无法连接 github.com" : null } : null,
      check_fresh: !q.has("stale"),
      elapsed: mockElapsed(),
    };
  }

  async function mockFetch(path) {
    await Promise.resolve();
    if (SCENE === "error") throw Object.assign(new Error("500"), { status: 500 });
    if (path === "/app/stage") return stage();
    if (path === "/app/chats") return chats();
    if (path === "/app/status") return status();
    const m = /^\/app\/chats\/([^/?#]+)(?:\/focus\/[^/?#]+)?$/.exec(path);
    if (m) return chat(decodeURIComponent(m[1]));
    throw Object.assign(new Error("404"), { status: 404 });
  }

  // Preview parameters follow chat links; normal app links retain their production routes.
  const keep = ["scene", "update", "service", "hosts", "activation"].filter((k) => q.has(k)).map((k) => `${k}=${encodeURIComponent(q.get(k))}`);
  function href(path) {
    const match = /^\/chat\/?([^/?#]*)(#.*)?$/.exec(path);
    if (!match) {
      const settings = /^\/settings\/(#.*)?$/.exec(path);
      return settings ? `/settings/?${["mock", ...keep].join("&")}${settings[1] || ""}` : path;
    }
    const params = ["mock", ...keep];
    if (location.protocol === "file:") {
      if (match[1]) params.push(`group=${encodeURIComponent(match[1])}`);
      return `chat.html?${params.join("&")}${match[2] || ""}`;
    }
    if (match[1]) params.push(`group=${encodeURIComponent(match[1])}`);
    return `/chat/?${params.join("&")}${match[2] || ""}`;
  }

  // Host/onboarding fixtures stay behind the existing explicit mock switch.
  const freshHosts = hostScene === "fresh";
  let serviceReady = !freshHosts, dismissed = false, installedState = null;
  const hostRows = ["codex", "pi", "workbuddy", "hermes"].map((id, index) => {
    const root = `/preview/${id}`;
    const label = HOSTS[id] || ({ workbuddy: "WorkBuddy 桌面", hermes: "Hermes" })[id] || id;
    const reload = id === "codex" ? "完整退出并重启 Codex 应用" : id === "workbuddy" ? "重启 WorkBuddy" : `重启 ${label}`;
    const connected = !freshHosts && index === 0;
    const orphan = !freshHosts && id === "hermes";
    return { id, label, installed: !orphan, detect_source: orphan ? "config" : "path",
      status: orphan ? "orphaned" : connected ? "connected" : "available", entry: connected || orphan ? "agentnave" : null,
      command: connected || orphan ? "/preview/bin/agentnave-mcp" : null, command_state: connected ? "present" : null,
      config_path: `${root}/config.json`, root: { path: root, source: "default" }, has_host_env: connected, shadowed_by: null,
      layers: [{ path: `${root}/config.json`, scope: "user", has_entry: connected || orphan }], tested: false,
      first_seen: "2026-10-17T00:00:00Z", reload_hint: reload, attention: null, last_action: null,
      skill: { state: connected ? "app" : "missing", path: `${root}/skills/agentnave-manager`, version: connected ? mockVersion : null },
      skills: { "agentnave-review": { state: "missing", path: `${root}/skills/agentnave-review`, version: null, enabled: false } } };
  });
  if (hostScene === "typical") {
    const warning = hostRows.find((host) => host.id === "workbuddy");
    Object.assign(warning, { status: "needs_attention", entry: "agentnave", command: "/missing/agentnave-mcp", command_state: "missing", attention: { code: "command_missing", message: "条目的命令不存在或不可执行" } });
    warning.layers[0].has_entry = true;
  }
  let membersReady = ["midway", "typical", "done"].includes(hostScene);
  let triedReady = hostScene === "done";
  const confirmedReloads = new Map();
  let mockToolsSynced = q.get("activation") !== "refresh";
  if (!mockToolsSynced) setTimeout(() => { mockToolsSynced = true; }, 12000);
  const hostView = () => ({ service: { installed: serviceReady, version: serviceReady ? mockVersion : null, command: `uv tool install --force --python 3.12 "git+https://github.com/TimWongUp/agentnave.git@v${mockVersion}"` }, hosts: hostRows.map((host) => {
    const token = `${host.id}:${host.root.path}`;
    const scene = q.get("activation");
    const state = scene === "tools" || !mockToolsSynced ? "required" : scene === "updating" ? "updating" : host.id === "codex" && scene !== "unknown" ? "current" : "unknown";
    return { ...host, reload: host.status === "connected" ? { state: confirmedReloads.get(host.id) === token ? "confirmed" : state, reason: state === "required" ? "tools" : "connection", connections: state === "unknown" ? 0 : 1, token } : { state: "not_connected" } };
  }), backups: { dir: "/preview/backups", config: 0, skill: 0 } });
  const onboarding = () => ({ dismissed, steps: [
    { id: "service", done: serviceReady, detail: "安装 AgentNave 本机服务" },
    { id: "host", done: hostRows.some((h) => h.status === "connected"), detail: "接入一个宿主" },
    { id: "members", done: membersReady, detail: "选择可以派出的 CLI 与模型" },
    { id: "try", done: triedReady, detail: "在宿主里派一次任务" }] });
  async function p3Fetch(path) {
    if (path === "/app/hosts") return structuredClone(hostView());
    if (path === "/app/skills") return { skills: [{ name: "agentnave-review", title: "AgentNave 评审门控", description: "代码评审门控：创建或更新 PR、用户要求评审代码或合并到主分支前使用。", content: "---\nname: agentnave-review\n---\n\n# AgentNave 评审门控\n\n（预览）完整写法在 App 中读取内嵌的 SKILL.md。" }] };
    if (path === "/app/onboarding") return onboarding();
    if (path === "/app/status" && installedState) return { ...status(), service: installedState };
    return mockFetch(path);
  }
  async function post(path, body = {}) {
    if (path.split("?")[0] === "/app/update/check") return p3Fetch("/app/status");
    if (path === "/app/update/apply?service=1") {
      installedState = { state: "upgrading" };
      setTimeout(() => { installedState = { state: "done", version: mockVersion }; }, 1500);
      return { ...await p3Fetch("/app/status"), started: true };
    }
    if (path.split("?")[0] === "/app/update/apply") {
      // Walks the real steps: download, service, the App, then the restart. &fail=<step> stops there.
      const total = 43 * 1048576;
      const fail = q.get("fail");
      const steps = [["download", 0], ["service", 4000], ["app", 9000], ["restart", 10500]];
      const stop = (step) => {
        timers.forEach(clearTimeout);
        const error = { download: "下载失败，App 和本机服务都没有改动：error sending request for url", service: "本机服务升级失败，App 未更新：无法连接 github.com", app: "安装失败：磁盘空间不足" }[step];
        updateOverride = { state: "failed", version: "0.21.1", step, error };
      };
      const started = Date.now();
      const timers = [];
      updateOverride = { state: "installing", version: "0.21.1", step: "download", downloaded: 0, total };
      for (let at = 200; at < 4000; at += 200)
        timers.push(setTimeout(() => { updateOverride = { ...updateOverride, downloaded: Math.floor(total * at / 4000) }; }, at));
      for (const [step, at] of steps.slice(1))
        timers.push(setTimeout(() => { updateOverride = { ...updateOverride, step }; mockElapsed = () => Math.floor((Date.now() - started - at) / 1000); }, at));
      if (fail) timers.push(setTimeout(() => stop(fail), fail === "download" ? 2000 : steps.find(([step]) => step === fail)[1] + 1500));
      timers.push(setTimeout(() => {
        if (fail) return;
        mockVersion = "0.21.1";
        updateOverride = { state: "none" };
        installedState = { state: "done", version: mockVersion };
      }, 11500));
      return { ...await p3Fetch("/app/status"), started: true };
    }
    if (path === "/app/hosts/refresh") return structuredClone(hostView());
    if (path === "/app/onboarding/dismiss") { dismissed = true; return { dismissed }; }
    if (path === "/app/onboarding/reopen") { dismissed = false; return { dismissed }; }
    if (path === "/app/hosts/reveal-backups") return { ok: true, message: "预览：已打开备份目录" };
    if (path === "/app/service/install") { installedState = { state: "installing" }; setTimeout(() => { serviceReady = true; installedState = { state: "done" }; }, 900); return { ok: true }; }
    const host = hostRows.find((h) => h.id === body.id);
    if (!host) throw new Error("未知宿主");
    if (path.endsWith("/confirm-reload")) {
      if (body.token !== `${host.id}:${host.root.path}` || host.status !== "connected") throw new Error("接入配置已变化，请刷新后再确认");
      confirmedReloads.set(host.id, body.token);
      return { ok: true, message: "预览：已记录你已重启或重连" };
    }
    if (path.endsWith("/skill")) {
      const installed = body.enabled && host.status === "connected";
      host.skills[body.skill] = { ...host.skills[body.skill], enabled: body.enabled, state: installed ? "app" : "missing", version: installed ? status().version : null };
      host.last_action = { ok: true, message: body.enabled ? (installed ? "已安装" : "已启用；接入此宿主时自动安装") : "已停用并移除", at: new Date().toISOString() };
      return { host: structuredClone(host), ok: true, message: host.last_action.message };
    }
    if (path.endsWith("/root")) host.root = { path: body.path || `/preview/${host.id}`, source: body.path ? "user" : "default" };
    else if (path.endsWith("/remove")) { confirmedReloads.delete(host.id); host.status = "available"; host.entry = host.command = host.command_state = null; host.layers.forEach((l) => l.has_entry = false); }
    else { host.status = "connected"; host.entry = "agentnave"; host.command = "/preview/bin/agentnave-mcp"; host.has_host_env = true; host.command_state = "present"; host.layers.forEach((l) => l.has_entry = true); if (host.skill.state === "missing") host.skill = { ...host.skill, state: "app", version: status().version }; }
    host.attention = null;
    host.last_action = { ok: true, message: "预览操作完成", at: new Date().toISOString() };
    return { host: structuredClone(host), ok: true, message: host.last_action.message };
  }
  window.AgentNaveMock = { enabled, fetch: p3Fetch, post, href, scene: SCENE };
})();
