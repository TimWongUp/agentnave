"use strict";
// Prototype mock backend. It serves the exact shapes of contract sections 2.1-2.3
// (/app/stage, /app/chats, /app/chats/<group>) plus the existing /app/status, driven by a
// small time-based scenario so that tails change, members finish and a manager appears.
//
// Loaded only by preview.html?mock&scene=live|calm|review|failed|six|quiet|empty|error
// &update=available|installing|failed &service=older|upgrading|done|failed &at=<seconds into live>
// &fail=download|service|app (the update stops at that step)
// &restart (reset the scenario clock; it is shared between pages through localStorage).
// Hosts page: &hosts=typical|fresh|reload|mixed (reload: every boss waiting on a reload; mixed: some
// waiting on a reload, one failing, one not connected; &copyfail fails the one-click stop/restart).
// &theme=light|dark picks the appearance.
(function () {
  const q = new URLSearchParams(location.search);
  const hostScene = q.get("hosts") || "typical";
  const SCENE = q.get("scene") || (hostScene === "fresh" ? "empty" : "live");
  const enabled = () => q.has("mock") || location.protocol === "file:";
  if (!enabled()) return;

  const mockHealth = q.has("health") ? { antigravity: { models: { "gemini-3.8-flash-high": { code: "quota", until: new Date(Date.now() + 3600000).toISOString() } } } } : {};
  // Named Claude accounts, shown with `?accounts`.
  const mockAccounts = q.has("accounts") ? [
    { id: "a1", provider: "claude", label: "工作", dir: "/Users/preview/.agentnave-accounts/claude/a1", managed: true },
    { id: "a2", provider: "claude", label: "个人 2", dir: "/Users/preview/.claude_personal", managed: false },
  ] : [];
  // Folders the scan of the home folder finds, shown in the add dialog.
  const mockFoundDirs = [
    { name: ".claude_work", dir: "/Users/preview/.claude_work", state: { logged_in: true, method: "claude.ai", email: "me@work.example", plan: "team", error: null } },
    { name: ".claude_trial", dir: "/Users/preview/.claude_trial", state: { logged_in: false, method: "none", email: null, plan: null, error: null } },
  ];
  const mockAccountStates = new Map([
    ["default", q.get("default") === "out"
      ? { logged_in: false, method: "none", email: null, plan: null, error: null }
      : { logged_in: true, method: "claude.ai", email: "tim@example.com", plan: "max", error: null }],
    ["a1", { logged_in: true, method: "claude.ai", email: "work@example.com", plan: "pro", error: null }],
    ["a2", { logged_in: false, method: "none", email: null, plan: null, error: null }],
  ]);
  const mockLoginAccounts = new Map();
  let nextLoginSession = 99;
  AN.onMockTerminalFrame = (frame, session, label) => {
    const accountId = mockLoginAccounts.get(session);
    const theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";
    const bg = theme === "light" ? "#f7f7f8" : "#14161a";
    const fg = theme === "light" ? "#1c2026" : "#e6eaf2";
    const muted = theme === "light" ? "#5f6875" : "#9aa4b2";
    const btnBg = theme === "light" ? "#1f6feb" : "#3b82f6";
    frame.removeAttribute("src");
    frame.srcdoc = `<!doctype html><html><body style="margin:0;padding:16px;font:13px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace;background:${bg};color:${fg};display:flex;flex-direction:column;justify-content:space-between;min-height:100vh;box-sizing:border-box;">
      <div>
        <div style="color:${muted}">$ claude auth login  # ${String(label || accountId || "default").replace(/[<>&]/g, "")}</div>
        <div style="margin-top:8px">Opening browser to authenticate with Claude.ai…</div>
        <div style="margin-top:4px;color:${muted}">Waiting for browser callback…</div>
      </div>
      <div>
        <button id="mock-complete" type="button" style="padding:6px 12px;border:0;border-radius:6px;background:${btnBg};color:#fff;font:12px/1.4 sans-serif;cursor:pointer;">模拟登录成功</button>
      </div>
      <script>
        document.getElementById("mock-complete").addEventListener("click", () => {
          window.parent.postMessage({ type: "agentnave:mock-login-complete", session: ${Number(session)}, id: ${JSON.stringify(accountId || "default")} }, "*");
        });
      </script>
    </body></html>`;
  };
  window.addEventListener("message", (event) => {
    if (event.data?.type !== "agentnave:mock-login-complete") return;
    const id = event.data.id || "default";
    mockAccountStates.set(id, {
      logged_in: true,
      method: "claude.ai",
      email: id === "default" ? "tim@example.com" : `${id}@example.com`,
      plan: "max",
      error: null,
    });
    window.postMessage({ type: "agentnave:terminal", action: "exited", session: event.data.session, code: 0 }, "*");
  });
  const mockMembersData = {
    accounts: { accounts: mockAccounts, error: null },
    provider_health: mockHealth,
    review: { version: 1, seats: { "security-reviewer": { provider: "codex", model: "gpt-6.1-sol", effort: "high", fallbacks: [{ provider: "claude", model: "claude-3-7-sonnet" }, { provider: "codex", model: "gpt-5.4-turbo", effort: "low" }] } } },
    permissions: { version: 1, clis: { codex: "auto", claude: "full" } },
    document: {
      version: 1,
      clis: {
        codex: {
          models: {
            "gpt-6.1-sol": {
              efforts: ["medium", "high", "xhigh"],
              effort: "high",
              options: {
                dangerously_bypass_approvals_and_sandbox: false,
              },
              notes: "多文件实现、需要跑测试的任务",
            },
            "gpt-5.4-turbo": {},
          },
        },
        claude: {
          models: {
            "claude-3-7-sonnet": {
              effort: "high",
              options: { custom_timeout: 60 },
              notes: "通用编码与复杂重构",
            },
          },
        },
      },
      modes: {
        task: {
          codex: {
            models: [], // Unlimited models
            default: "gpt-6.1-sol",
            overrides: {},
          },
          claude: {
            models: ["claude-3-7-sonnet"],
            default: "claude-3-7-sonnet",
            overrides: {
              // Tri-state override example: effort customized, notes customized, efforts & options inherit
              "claude-3-7-sonnet": {
                effort: "medium",
                notes: "本模式专用说明",
              },
            },
          },
        },
        // discussion is omitted => unrestricted mode
      },
    },
    error: null,
    detected: {
      codex: {
        installed: true,
        path: "/opt/homebrew/bin/codex",
        version: "0.161.0",
        source: "codex-cache",
        error: null,
        models: [
          {
            id: "gpt-6.1-sol",
            label: "gpt-6.1-sol",
            default: false,
            efforts: ["low", "medium", "high", "xhigh"],
            effort: "medium",
          },
          { id: "gpt-5.4-turbo", label: "gpt-5.4-turbo", default: false, effort: "low" },
        ],
        native: { model: "gpt-6.1-sol", effort: "medium" },
        efforts: ["minimal", "low", "medium", "high", "xhigh"],
      },
      claude: {
        installed: true,
        path: "/usr/local/bin/claude",
        version: "2.1.308",
        source: "command",
        error: null,
        models: [
          { id: "claude-3-7-sonnet", label: "claude-3-7-sonnet", default: true },
          { id: "claude-3-5-haiku", label: "claude-3-5-haiku", default: false },
        ],
        native: { model: "claude-3-7-sonnet", effort: null },
        efforts: ["low", "medium", "high"],
      },
      codebuddy: {
        installed: true,
        path: "/opt/homebrew/bin/codebuddy",
        source: "help",
        error: null,
        models: [
          { id: "codebuddy-pro", label: "codebuddy-pro", default: true },
        ],
        efforts: [],
      },
      grok: {
        installed: false,
        path: null,
        source: "none",
        error: "未在 PATH 中找到 grok 可执行文件",
        models: [],
        efforts: [],
      },
      antigravity: {
        installed: true,
        path: "/opt/homebrew/bin/agy",
        source: "command",
        error: null,
        models: [
          { id: "gemini-3.8-flash", label: "gemini-3.8-flash", default: true },
        ],
        efforts: ["low", "high"],
      },
      pi: {
        installed: false,
        path: null,
        source: "none",
        error: "未安装",
        models: [],
        efforts: [],
      },
    },
  };


  const mockOrchestration = {
    document: { version: 1, preset: "split", apps: { codex: "off" }, meeting: { count: 4, seats: [{ provider: "codex" }] } },
    error: null,
    bosses: [
      { keys: ["claude", "grok"], labels: ["Claude Code", "Grok"], mark: "claude" },
      { keys: ["codex"], labels: ["Codex"], mark: "codex" },
    ],
    presets: {
      meeting: null,
      meetings_parts: { head: "## AgentNave 编排偏好\n\n- （示例）只开会的规则", tail: "\n- （示例）开完会再收尾。" },
      meeting_parts: {
        default_count: 3, separator: "、",
        seat_provider: "{name}（provider=`{provider}`）",
        all: "\n- 开会默认 {count} 席：{seats}。", some: "\n- 开会默认 {count} 席：{seats}，其余由 boss 选。", none: "\n- 开会默认 {count} 席，成员由 boss 选。",
      },
      split_parts: {
        base: "## AgentNave 编排偏好\n\n- （示例）分工协作的规则", parallel_writes_asked: "\n- 并行写入须用户本人指明。", parallel_writes_own: "\n- 并行写入由 boss 判断。",
        checks: "\n- （示例）评审与验收。",
        meetings_asked: "\n- 只在用户要求时开会。", meetings_own: "\n- boss 可以主动开会。", tail: "\n- （示例）按固定工位派发。",
      },
      decision: "## AgentNave 编排偏好\n\n- （示例）决策模型的规则",
      decision_parts: {
        base: "## AgentNave 编排偏好\n\n- （示例）决策模型的规则", parallel_writes_asked: "\n- 并行写入须用户本人指明。", parallel_writes_own: "\n- 并行写入由 boss 判断。",
        checks: "\n- （示例）评审与验收。",
        meetings_asked: "\n- 只在用户要求时开会。", meetings_own: "\n- boss 可以主动开会。", tail: "\n- （示例）由决策模型选工位。",
      },
      meetings: "## AgentNave 编排偏好\n\n- （示例）只开会的规则", split: "## AgentNave 编排偏好\n\n- （示例）分工协作的规则" },
    roles: [
      ["explorer", "通用探索", "General exploration", "read", true],
      ["worker", "通用执行", "General execution", "write", true],
      ["worker_frontend", "界面制作", "Interface work", "write", true],
      ["worker_writing", "写作", "Writing", "write", false],
      ["explorer_web", "网页调研", "Web research", "read", false],
      ["explorer_social", "社交平台调研", "Social platform research", "read", false],
      ["worker_documents", "办公文件制作", "Office files", "write", false],
    ].map(([group, name_zh, name_en, access, legacy_enabled]) => ({
      group, name_zh, name_en, access, legacy_enabled,
      fallback_group: ["explorer", "worker"].includes(group) ? null : group.split("_")[0],
    })),
    files: [
      { path: "/Users/me/.claude/CLAUDE.md", readers: ["Claude Code", "Grok"], exists: true, target: true, current: true, blocked: false },
      { path: "/Users/me/.codex/AGENTS.md", readers: ["Codex"], exists: false, target: false, current: false, blocked: false },
    ],
  };


  // hosts=fresh is a new user: no employee confirmed, orchestration off (the first-run checklist shows).
  if (hostScene === "fresh") {
    mockMembersData.document = { version: 1, clis: {}, modes: {} };
    mockOrchestration.document = { version: 1, preset: "off" };
  }

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
    "codex-desktop": "ChatGPT（Codex）",
    codex: "Codex",
    codebuddy: "CodeBuddy",
    grok: "Grok",
    antigravity: "Antigravity",
    "antigravity-desktop": "Antigravity 桌面",
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
3. relay 重启后 \`live/\` 快照未清理，托盘会短暂显示已退出的会话。`;
  const OUT_GEMINI = `发布前评审通过。三点建议：安装文档补上升级后需重启宿主一次的说明；Release 说明把托盘与服务端改动分开写；更新包签名密钥说明放进内部文档而不是公开 README。`;
  const OUT_BUDDY = `文档已更新：docs/context/architecture.md 新增“空闲换进程”一节，说明判定条件、等待上限与已知限制。`;
  const OUT_NAME = `我倾向叫“舞台”：它直观表达“boss 在左、组员在右、中间是 AgentNave”，也与办公室页的“工作区”形成一对。\n\n风险是“舞台”暗示表演，和“节目”概念重叠；建议菜单面板叫“舞台”，App 里的会话叫“工作区”，会议保持原名。`;

  // ---- scenario specs ---------------------------------------------------------------------
  // One spec = one member's work inside one conversation (a task, or a seat of a show).
  // start/end are scenario seconds (negative = before the demo began); end:null runs forever.
  const deskKey = (provider, model, role) => `${provider}|${model || NATIVE}|${role || ""}`;
  const deskLabel = (provider, model, role) => [model || "未记录模型", HOSTS[provider] || provider, role].filter(Boolean).join(" · ");
  // Demo identities only: the real ones come from the service. `ordinal` on a spec shows a numbered name.
  const SPOKEN = { codex: "GPT", claude: "Claude", grok: "Grok", antigravity: "Gemini", codebuddy: "GLM", deepseek: "DeepSeek", pi: "Pi" };
  const identityOf = (s) => {
    const model_short = s.model ? s.model.split("/").pop() : "未记录模型";
    const cli = HOSTS[s.provider] || s.provider;
    const tail = `·${[cli, s.role].filter(Boolean).join("·")}${s.ordinal ? ` #${s.ordinal}` : ""}`;
    return {
      key: s.desk, provider: s.provider, cli, model: s.model, model_short, model_source: s.model ? "native" : "unknown",
      role: s.role, ordinal: s.ordinal || 1, numbered: Boolean(s.ordinal),
      handle: `@${s.model || model_short}${tail}`, handle_short: `@${model_short}${tail}`,
      spoken_name: `${SPOKEN[s.provider] || cli}${s.ordinal ? ` #${s.ordinal}` : ""}`,
    };
  };
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
    s.identity = identityOf(s);
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
      ordinal: 2,
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
    // The implementer's private task carries two replies in view and one gone from the list.
    codex.forwards = [{ from: room1, spec: gem }, { from: room2, spec: pm }, { id: "20261016-0000-000000:t:gone" }];
    const c1 = { id: "20261017-0912-a1b2c3", title: "relay 空闲换进程：保留活跃子进程树", mode: "task", specs: [codex] };
    const c4 = { id: "20261017-0915-b2c3d4", title: "审查 relay 换进程流程", mode: "task", specs: [grok] };
    const c3 = { id: "20261017-0958-c3d4e5", title: "补 relay 文档", mode: "task", specs: [buddy] };
    const c5 = { id: "20261017-0930-d4e5f6", title: "整理 relay 历史改动", mode: "task", specs: [ds] };
    return {
      group: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50", host: "claude", app: "claude-desktop", color: 0, live: true,
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
        group: `${String(gi + 1).repeat(32).slice(0, 32)}`, host, app: host === "antigravity" ? "antigravity-desktop" : null, color, live: gi % 2 === 0 || states.includes("running"),
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
  const recipientOf = (conv, sp) => ({ key: sp.key, provider: sp.provider, model: sp.model, label: sp.label, seat: seatOf(conv, sp), identity: sp.identity });

  function messagesOf(g, w) {
    const manager = { role: "manager", provider: g.host, model: null, label: label(g) };
    const speakerOf = (e) => ({ role: "member", provider: e.provider, model: e.model, label: e.label || e.model, identity: e.identity });
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
            { id: `p:${conv.id}:${e.key}`, time: stamp(w, sp.start), conversation: conv.id, kind: "prompt", speaker: manager, to: [e.key], recipients: [recipientOf(conv, sp)], text: sp.prompt, status: "succeeded", detail: null, ...(q.has("isolated") && sp === conv.specs[0] && g.group === w.groups[0].group ? { isolation_branch: ISOLATED[0].branch } : {}) },
            { visibility: "private", thread, desk, message_id: mid },
          ));
        if (!show && sp.prompt && sp.forwards)
          out.at(-1).forwards = sp.forwards.map((f) => ({ message_id: f.id || midOf(f.from, f.spec), source: f.spec ? f.spec.deskLabel : "" }));
        if (q.has("prompts") && conv === g.convs[0] && sp === conv.specs[0]) {
          const samples = [
            { kind: "permission", tool_name: "Write", status: "pending" },
            { kind: "question", status: "pending" },
            { kind: "permission", tool_name: "Write", status: "answered", answer: { actor: "boss", decision: "allow" } },
            { kind: "permission", tool_name: "Bash", status: "answered", answer: { actor: "boss", decision: "deny", message: "超出任务范围" } },
            { kind: "question", status: "answered", answer: { actor: "boss", answers: { "保存到哪里？": "项目目录", "是否继续？": "是" } } },
            { kind: "permission", tool_name: "Write", status: "answered", answer: { actor: "service_cancel", decision: "deny" } },
            { kind: "permission", tool_name: "Write", status: "closed" },
          ];
          samples.forEach((approval, i) => out.push(office({ id: `approval:${conv.id}:${i}`, time: stamp(w, e.at - 7 + i), conversation: conv.id, kind: "approval", speaker: speakerOf(e), text: "", approval: { tool_input: JSON.stringify({ file_path: "/tmp/<script>**literal**</script>" }), ...approval }, to: [], reply_to: show ? null : `p:${conv.id}:${e.key}` }, { visibility: "private", thread, desk })));
        }
        const base = { time: stamp(w, e.at), conversation: conv.id, speaker: speakerOf(e), to: [] };
        if (e.status === "running")
          typing.push(office(
            { ...base, id: `ty:${conv.id}:${e.key}`, time: nowIso(), kind: "typing", ...(g.host === "codex"
              ? { text: "", doing: "tool:Read", doing_since: nowIso(), tools: ["Grep", "Bash", "Edit", "web_search", "command_execution"] }
              : { text: e.lines.join("\n") }), status: "running", detail: null, _s: sp.start },
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
    const ho = handoverOf(g);
    if (ho) out.push({
      id: `${g.group}:handover`, time: stamp(w, w.s - 1), conversation: "", kind: "handover",
      speaker: { role: "system", label: "agent 交接" },
      text: ho === "pending" ? `${label(g)} 交出 agent，等待接手` : ho === "changed" ? `${label(g)} 交出 agent，Codex 已接手` : `${label(g)} 交出 agent，已由 Codex 接手`,
      detail: "下一步：评审三个 PR 后统一发版。\n说明按纯文本显示：<script>alert(1)</script>\n\n" + "保留已有改动，核对 CI 与正式发布产物。\n".repeat(25),
    });
    out.sort((a, b) => Date.parse(a.time) - Date.parse(b.time));
    typing.sort((a, b) => a._s - b._s).forEach((m) => delete m._s);
    return [...out, ...typing].slice(-300);
  }

  // The running limit caps drawn regular desks (?limit=N previews "已优化"); busy and derived desks always show.
  function limited(all, countOnly = false) {
    const limit = Number(q.get("limit") || 8);
    const derived = all.filter((d) => d.derived);
    const regular = all.filter((d) => !d.derived);
    const busy = (d) => d.status === "running";
    const stampOf = (d) => Date.parse(countOnly ? d.last_prompted || d.last_active : d.last_active);
    const order = [...regular].sort((a, b) => Number(busy(b)) - Number(busy(a)) || stampOf(b) - stampOf(a));
    const cap = countOnly ? regular.filter(busy).length + limit : Math.max(limit, regular.filter(busy).length);
    const keep = new Set(order.slice(0, cap).map((d) => d.key));
    const rest = order.filter((d) => !keep.has(d.key));
    return {
      desks: [...(countOnly ? order : regular).filter((d) => keep.has(d.key)), ...derived],
      optimized: countOnly ? { count: rest.length } : { count: rest.length, desks: rest },
    };
  }
  const MOCK_DERIVED_DESKS = [
    { key: "thread:drv-run", provider: "claude", model: "claude-opus-5-5", role: "修登录竞态", state: "running" },
    { key: "thread:drv-wait", provider: "codex", model: "gpt-6.1-sol", role: "确认迁移脚本", state: "waiting" },
    { key: "thread:drv-pick", provider: "antigravity", model: "gemini-3.5-pro", role: "整理接口文档", state: "uncollected" },
    { key: "thread:drv-idle", provider: "claude", model: "claude-3-7-sonnet", role: "补单测覆盖", state: "idle" },
    { key: "thread:drv-dormant", provider: "codex", model: "gpt-6-mini", role: "重构缓存层", state: "dormant" },
    { key: "thread:drv-old", provider: "claude", model: "claude-3-7-sonnet", role: "清理旧配置", state: "idle" },
  ];
  function desksOf(g, w) {
    const hostCli = g.host === "workbuddy" ? "codebuddy" : g.host;
    const map = new Map();
    for (const conv of g.convs)
      for (const sp of conv.specs) {
        const e = evalSpec(sp, w);
        if (!e) continue;
        const d = map.get(sp.desk) || { sp, first: sp.start, last: e.at, prompted: sp.start, active: false, latest: null, threads: 0, running: 0, meeting: null };
        map.set(sp.desk, d);
        d.first = Math.min(d.first, sp.start);
        d.last = Math.max(d.last, e.at);
        d.prompted = Math.max(d.prompted, sp.start);
        if (isActive(e.status)) d.active = true;
        else if (!d.latest || e.at >= d.latest.at) d.latest = { at: e.at, status: e.status };
        if (conv.mode === "task") {
          d.threads++;
          if (e.status === "running") d.running++;
        } else if (isActive(e.status) && (!d.meeting || e.at >= d.meeting.at)) d.meeting = { at: e.at, room: conv.id, seat: seatOf(conv, sp), status: e.status };
      }
    const baseDesks = [...map.values()]
      .sort((a, b) => b.first - a.first || (a.sp.desk < b.sp.desk ? -1 : 1))
      .map((d) => ({
        key: d.sp.desk, provider: d.sp.provider, model: d.sp.model, role: d.sp.role, access: d.sp.access ?? null, identity: d.sp.identity,
        first_seen: stamp(w, d.first), last_active: d.active ? nowIso() : stamp(w, d.last), last_prompted: stamp(w, d.prompted),
        status: d.active ? "running" : FAILED.includes(d.latest?.status) ? "failed" : "idle",
        private_allowed: d.sp.provider !== hostCli,
        kind: d.threads ? "task" : "seat", room: d.threads ? null : d.meeting?.room || null,
        private: { tasks: d.threads, running: d.running },
        meeting: d.meeting ? { room: d.meeting.room, seat: d.meeting.seat, status: d.meeting.status } : { room: null, seat: null, status: null },
        derived: false,
      }));
    if (g.group === "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50") {
      for (const dd of MOCK_DERIVED_DESKS) {
        const sp = spec(dd.provider, dd.model, { role: dd.role });
        baseDesks.push({
          key: dd.key, provider: dd.provider, model: dd.model, role: dd.role, access: "write", identity: sp.identity,
          first_seen: stamp(w, -300), last_active: nowIso(), last_prompted: stamp(w, -120),
          status: dd.state === "running" || dd.state === "waiting" ? "running" : "idle",
          private_allowed: true, kind: "task", room: null,
          private: { tasks: 1, running: dd.state === "running" ? 1 : 0 },
          meeting: { room: null, seat: null, status: null },
          derived: true,
        });
      }
    }
    return baseDesks;
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
          seats: conv.specs.map((sp) => ({ seat: seatOf(conv, sp), desk: sp.desk, identity: sp.identity })),
          pending_review: es.filter((e) => e.status === "review").length,
          board,
        };
      });
  }

  // ?isolated previews isolated directories: in use, left over and unfinished; the first one
  // was created by the first private task, whose card shows its branch.
  const ISOLATED = [
    { id: "wt-3f9a1c2b4d5e", path: "/work/app/.agentnave-worktrees/wt-3f9a1c2b4d5e", branch: "agentnave/fix-login", status: "using", created: new Date(Date.now() - 40 * 60000).toISOString() },
    { id: "wt-7b2e9d1a0c44", path: "/work/app/.agentnave-worktrees/wt-7b2e9d1a0c44", branch: "agentnave/docs-refresh", status: "idle", created: new Date(Date.now() - 3 * 3600000).toISOString() },
    { id: "wt-0c8d4e6f1a22", path: "/work/app/.agentnave-worktrees/wt-0c8d4e6f1a22", branch: "agentnave/old-experiment", status: "unfinished", created: new Date(Date.now() - 26 * 3600000).toISOString() },
    { id: "wt-5a6b7c8d9e0f", path: "/work/app/.agentnave-worktrees/wt-5a6b7c8d9e0f", branch: "agentnave/payments-spike", status: "collapsed", created: new Date(Date.now() - 9 * 86400000).toISOString() },
  ];
  // What the automatic cleanup did, as the tray reports it: one earlier pass and, after "clean now", a second.
  let mockCleanupRan = false;
  function mockCleanup() {
    const ago = (minutes) => new Date(Date.now() - minutes * 60000).toISOString();
    const workspace = world().groups[0].group;
    const kept = (id, branch, reason) => ({ id, workspace, branch, outcome: "kept", rule: null, reason, detail: null });
    const collapsed = { id: "wt-5a6b7c8d9e0f", workspace, branch: "agentnave/payments-spike", outcome: "collapsed", rule: "idle", reason: null, detail: null };
    const recent = [
      { ...collapsed, at: ago(mockCleanupRan ? 0 : 42) },
      { id: "wt-1d2e3f4a5b6c", workspace, branch: "agentnave/auth-cleanup", outcome: "removed", rule: "merged", reason: null, detail: "#214", at: ago(mockCleanupRan ? 0 : 42) },
      { id: "wt-8e9f0a1b2c3d", workspace, branch: "agentnave/typo-fix", outcome: "removed", rule: "unchanged", reason: null, detail: null, at: ago(190) },
    ];
    return {
      running: false,
      report: {
        at: ago(mockCleanupRan ? 0 : 42), trigger: mockCleanupRan ? "manual" : "auto", error: null,
        entries: [kept("wt-3f9a1c2b4d5e", "agentnave/fix-login", "busy"), kept("wt-7b2e9d1a0c44", "agentnave/docs-refresh", "dirty"), kept("wt-0c8d4e6f1a22", "agentnave/old-experiment", "not_ready"), recent[0], recent[1]],
      },
      recent,
    };
  }
  const isolatedOf = (g, w) => {
    if (!q.has("isolated") || g.group !== w.groups[0].group) return [];
    const first = g.convs.find((c) => c.mode === "task")?.specs[0];
    return ISOLATED.map((d, i) => ({ ...d, task: i === 0 && first ? { desk: first.desk, text: (first.prompt || "").split("\n")[0].slice(0, 80) } : null }));
  };

  // Where each workspace began: two projects share the name "app" to show how they are told apart.
  const PLACES = [
    { project_key: "/work/app", project_label: "app", project_path: "/work/app" },
    { project_key: "/work/app", project_label: "app", project_path: "/work/app" },
    { project_key: "other", project_label: null, project_path: null },
    { project_key: "/Users/me/old/app", project_label: "app", project_path: "~/old/app" },
    { project_key: "/work/docs", project_label: "docs", project_path: "/work/docs" },
    { project_key: "/work/docs", project_label: "docs", project_path: "/work/docs" },
  ];
  const placeOf = (g, w) => PLACES[w.groups.indexOf(g) % PLACES.length];

  const mockHandovers = new Map();
  const mockProjects = [];
  const handoverOf = (g) => mockHandovers.has(g.group) ? mockHandovers.get(g.group) : (g.group.startsWith("s-") ? q.get("handover") || null : null);

  const displayTitleOf = (g) => g.convs.find((c) => c.mode === "task")?.title || g.convs[0]?.title || "";

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
        const display_title = displayTitleOf(g);
        return {
          group: g.group, host: g.host, app: g.app || null, label: label(g), color: g.color, live: g.live, status, updated,
          cli_session: !g.app || !String(g.app).endsWith("-desktop"),
          handover: handoverOf(g), title: conv.title, display_title, title_id: conv.id, conversations: g.convs.length, preview,
          desks_count: desksOf(g, w).length, rooms_count: roomsOf(g, w).length,
          isolated_count: isolatedOf(g, w).length,
          ...placeOf(g, w), boss_key: `${g.host}|${label(g)}`,
        };
      })
      .sort((a, b) => (a.handover === "pending" ? -1 : URGENCY[a.status] ?? 2) - (b.handover === "pending" ? -1 : URGENCY[b.status] ?? 2) || Date.parse(b.updated) - Date.parse(a.updated));
    return { groups: rows, projects: structuredClone(mockProjects) };
  }

  function chat(group) {
    const w = world();
    const g = w.groups.find((x) => x.group === group);
    if (!g || !summaryOf(g, w).members.length) throw Object.assign(new Error("404"), { status: 404 });
    const { members, status } = summaryOf(g, w);
    const ho = handoverOf(g);
    const place = placeOf(g, w);
    const display_title = displayTitleOf(g);
    return {
      group: g.group, host: g.host, app: g.app || null, label: label(g), color: g.color, live: g.live, status,
      cli_session: !g.app || !String(g.app).endsWith("-desktop"),
      title: g.convs[0]?.title || "", display_title, title_id: g.convs[0]?.id || null, project: place.project_label,
      handover: ho,
      took_over: ho === "changed" ? `${label(g)} 接手了 Claude Code` : null,
      previous_host: ho === "changed" ? "claude" : null,
      previous_label: ho === "changed" ? "Claude Code" : null,
      ...place,
      conversations: g.convs
        .filter((c) => c.specs.some((sp) => w.s >= sp.start))
        .map((c) => ({ id: c.id, title: c.title, mode: c.mode, status: convStatus(c, w) })),
      members,
      ...limited(desksOf(g, w)),
      rooms: roomsOf(g, w),
      messages: messagesOf(g, w),
      isolated: isolatedOf(g, w),
      isolated_count: isolatedOf(g, w).length,
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

  // Keep every prototype navigation on the static preview entry.
  function href(path) {
    const match = /^\/(chat|settings|panel)\/?([^/?#]*)(\?[^#]*)?(#.*)?$/.exec(path);
    if (!match) return path;
    const params = new URLSearchParams(location.search);
    params.set("page", match[1]);
    params.delete("group");
    for (const [key, value] of new URLSearchParams(match[3] || "")) params.set(key, value);
    if (match[1] === "chat" && match[2]) params.set("group", decodeURIComponent(match[2]));
    return `preview.html?${params}${match[4] || ""}`;
  }

  // Host fixtures stay behind the existing explicit mock switch.
  const freshHosts = hostScene === "fresh";
  let serviceReady = !freshHosts, installedState = null;
  // hosts=reload: every boss is connected and waiting on a reload, with the old copies the App found.
  const reloadScene = hostScene === "reload";
  const mixedScene = hostScene === "mixed";
  const reloadIds = reloadScene ? ["claude", "codex", "hermes", "workbuddy", "pi"] : mixedScene ? ["claude", "hermes"] : [];
  const REOPEN = {
    claude: { daemon: "停止 Claude 后台服务，释放预热会话的旧连接；这会结束后台会话", desktop: "完全退出 Claude 桌面端后重开，或结束并重开其中的 Code 会话", cli: "退出当前 Claude Code 会话后重开，可用 claude --continue 接回" },
    codex: { daemon: "重启 Codex 后台服务；正在运行的后台或远程任务会中断", app: "完全退出 Codex 或 ChatGPT 桌面端后重开", ide: "在编辑器命令面板运行 Developer: Reload Window，或完全退出并重开编辑器", cli: "退出当前 Codex 会话后重开，可用 codex resume 接回" },
    hermes: { gateway: "重启 Hermes 网关；消息平台上的对话会短暂中断并重新连接", app: "完全退出 Hermes 桌面端后重开", cli: "退出当前 Hermes 会话后重开" },
    workbuddy: { app: "完全退出 WorkBuddy 后重开", cli: "完全退出 WorkBuddy 后重开" },
    pi: { cli: "在 Pi 会话里输入 /reload，或重开 Pi" },
  };
  const ago = (minutes) => Date.now() - minutes * 60e3;
  const RELOADS = {
    claude: {
      reason: "stale",
      copies: [
        { kind: "daemon", since: ago(180) },
        { kind: "desktop", since: ago(42), bundle: "/Applications/Claude.app" },
        { kind: "cli", pid: 41001, since: ago(660), cwd: "/Users/me/Code/agentnave", terminal: "herdr" },
        { kind: "cli", pid: 41002, since: ago(95), cwd: "/Users/me/Notes", terminal: "Ghostty" },
        { kind: "cli", pid: 41003, since: ago(20) },
      ],
    },
    codex: {
      reason: "host_running",
      copies: [
        { kind: "daemon", since: ago(300) },
        { kind: "app", since: ago(25), bundle: "/Applications/ChatGPT.app" },
        { kind: "cli", pid: 42001, since: ago(140), cwd: "/Users/me/Code/api", terminal: "iTerm2" },
      ],
    },
    hermes: { reason: "tools", copies: [{ kind: "gateway", since: ago(1500) }] },
    workbuddy: { reason: "unverified", copies: [] },
    pi: { reason: "config", copies: [{ kind: "cli", pid: 43001, since: ago(50), cwd: "/Users/me/Code/scripts", terminal: "tmux" }] },
  };
  const hostIds = reloadScene || mixedScene ? ["claude", "codex", "hermes", "workbuddy", "pi"] : ["codex", "pi", "workbuddy", "hermes"];
  const hostRows = hostIds.map((id, index) => {
    const root = `/preview/${id}`;
    const label = HOSTS[id] || ({ workbuddy: "WorkBuddy 桌面", hermes: "Hermes" })[id] || id;
    const reload = id === "codex" ? "完整退出并重启 Codex 应用" : id === "workbuddy" ? "重启 WorkBuddy" : `重启 ${label}`;
    const connected = reloadScene || (mixedScene ? ["claude", "codex", "hermes"].includes(id) : !freshHosts && index === 0);
    const orphan = !reloadScene && !mixedScene && !freshHosts && id === "hermes";
    return { id, label, installed: !orphan, detect_source: orphan ? "config" : "path",
      status: orphan ? "orphaned" : connected ? "connected" : "available", entry: connected || orphan ? "agentnave" : null,
      command: connected || orphan ? "/preview/bin/agentnave-mcp" : null, command_state: connected ? "present" : null,
      config_path: `${root}/config.json`, root: { path: root, source: "default" }, has_host_env: connected, shadowed_by: null,
      layers: [{ path: `${root}/config.json`, scope: "user", has_entry: connected || orphan }], tested: freshHosts,
      first_seen: "2026-10-17T00:00:00Z", reload_hint: reload, attention: null, last_action: null, boss: id, default_host_cli: id === "antigravity" ? "launch" : "subagent",
      reopen: REOPEN[id] || { cli: reload }, copy_actions: { claude: ["daemon"], codex: ["daemon"], hermes: ["gateway"] }[id] || [], trust: id === "workbuddy" ? "WorkBuddy 会把改动后的条目停用，请到“自定义连接器 › MCP 服务管理”点“信任”" : "",
      skill: { state: connected ? "app" : "missing", path: `${root}/skills/agentnave-manager`, version: connected ? mockVersion : null },
      skills: { "agentnave-review": { state: "missing", path: `${root}/skills/agentnave-review`, version: null, enabled: false } } };
  });
  if ((hostScene === "typical" || mixedScene) && !reloadScene) {
    const warning = hostRows.find((host) => host.id === "workbuddy");
    Object.assign(warning, { status: "needs_attention", entry: "agentnave", command: "/missing/agentnave-mcp", command_state: "missing", attention: { code: "command_missing", message: "条目的命令不存在或不可执行" } });
    warning.layers[0].has_entry = true;
  }
  // &fault shows the connection-lost note on the first host (the one already connected).
  if (q.has("fault")) hostRows[0].fault = { count: 1, at_ms: Date.now() - 60e3 };
  const confirmedReloads = new Map();
  let mockToolsSynced = q.get("activation") !== "refresh";
  if (!mockToolsSynced) setTimeout(() => { mockToolsSynced = true; }, 12000);
  const hostView = () => ({ absent: [{ mark: "zcode", label: "ZCode" }, { mark: "grok", label: "Grok CLI" }, { mark: "deepseek", label: "DeepSeek Harness" }], service: { installed: serviceReady, launcher: serviceReady ? "/Users/me/.agentnave/service/bin/agentnave-mcp" : null, skills: serviceReady ? "/Users/me/.agentnave/service/lib/python3.12/site-packages/agentnave/skills" : null, version: serviceReady ? mockVersion : null, command: `uv tool install --force --python 3.12 "git+https://github.com/TimWongUp/agentnave.git@v${mockVersion}"` }, hosts: hostRows.map((host) => {
    const token = `${host.id}:${host.root.path}`;
    if (reloadIds.includes(host.id)) {
      const r = RELOADS[host.id];
      const done = confirmedReloads.get(host.id) === token;
      return { ...host, reload: { state: done ? "confirmed" : "required", reason: r.reason, connections: r.reason === "unverified" ? 0 : 1, token, copies: done ? [] : structuredClone(r.copies) } };
    }
    const scene = q.get("activation");
    const state = scene === "stale" || scene === "tools" || !mockToolsSynced ? "required" : host.id === "codex" && scene !== "unknown" ? "current" : "unknown";
    return { ...host, reload: host.status === "connected" ? { state: confirmedReloads.get(host.id) === token ? "confirmed" : state, reason: scene === "stale" ? "stale" : state === "required" ? "tools" : "connection", connections: state === "unknown" ? 0 : 1, token } : { state: "not_connected" } };
  }), backups: { dir: "/preview/backups", config: 0, skill: 0 } });
  const mockPrefs = {
    general: {
      theme: q.get("theme") || "system",
      motion: "system",
      scale: 100,
      autostart: false,
      open_on_start: true,
      tray_click: "panel",
      tray_count: true,
      tray_font: "baskerville",
      keep_awake: false,
      battery_still: false,
      pause_unfocused: true,
    },
    alerts: {
      notify: "off",
      scope: "all",
      threshold: 10,
      problems: false,
    },
  };
  const preferencesView = () => ({
    preferences: structuredClone(mockPrefs),
    effective: structuredClone(mockPrefs),
    autostart_available: !q.has("preview"),
    error: null,
  });
  let mockRetentionDays = null;
  let mockVoiceCacheBytes = 14820352, mockPlayer = null;
  const mockVoice = {
    enabled: true,
    listen_minutes: 60,
    continue_read: true,
    read_scope: "all",
    code: "skip",
    cache_limit_mb: 200,
    engine: "elevenlabs",
    minimax_region: "cn",
    model: "eleven_turbo_v2_5",
    models: { elevenlabs: "eleven_turbo_v2_5", minimax: "speech-01-turbo" },
    keys: { elevenlabs: true, minimax: false },
    key_previews: { elevenlabs: "sk_3…a8b2" },
    voices: {},
    personas: {},
    chatgpt: { signed_in: true, signing_in: false, user: "preview@example.com", plan: "Plus", error: null },
    summary_platforms: [
      { id: "chatgpt", name: "ChatGPT" },
      { id: "magpie", name: "Magpie" },
      { id: "custom", name: "自定义" },
    ],
    summary: {
      enabled: true,
      level: "standard",
      language: "source",
      provider: "chatgpt",
      model: "gpt-5.4-mini",
      effort: "",
      base_url: "",
      prompt: "",
    },
  };
  const mockProvider = (id, name_zh, name_en, key, default_model, models, extra = {}) => ({
    id, name_zh, name_en, key, fixed_model: false, default_model, models: models.map((model) => ({ id: model, name: model })),
    catalog: false, custom_url: false, url_required: false, default_base_url: null, ...extra,
  });
  // The tray's mask: the first and last four of a long key, the last four of a middling one, nothing of a short one.
  const maskKey = (key) => {
    const chars = Array.from(key.trim());
    return chars.length >= 16 ? `${chars.slice(0, 4).join("")}…${chars.slice(-4).join("")}` : chars.length >= 8 ? `••••${chars.slice(-4).join("")}` : chars.length ? "••••" : "";
  };
  const mockDecision = {
    document: null, error: null, key_set: false, key_error: null,
    keys: { gemini: false, openai: false, typesafe: false, openrouter: false, custom: false },
    key_previews: {},
    providers: [
      mockProvider("gemini", "Google Gemini", "Google Gemini", "required", "gemini-decision-1", [], { fixed_model: true }),
      mockProvider("openai", "OpenAI", "OpenAI", "required", "gpt-6-luna", [], { fixed_model: true }),
      mockProvider("typesafe", "TypeSafe AI", "TypeSafe AI", "required", "jev-latest", ["jev-latest", "jev-1.14.0"], { catalog: true }),
      mockProvider("openrouter", "OpenRouter", "OpenRouter", "required", "liquid/d1.2-ultra", ["liquid/d1.2-ultra", "liquid/d1.2", "microsoft/microsoft-decision-1.1", "typesafe/jev-1.14", "perplexity/pplx-decider-v1.3:free", "parallel/decisions-fast", "google/gemini-decision-1", "openai/gpt-6-luna-decisions"], { catalog: true }),
      mockProvider("ollama", "本机 Ollama", "Local Ollama", "none", null, [], { catalog: true, custom_url: true, default_base_url: "http://127.0.0.1:11434" }),
      mockProvider("custom", "自定义端点", "Custom endpoint", "optional", null, [], { catalog: true, custom_url: true, url_required: true }),
    ],
  };
  const mockDecisionModels = {
    typesafe: ["jev-1.14.0", "jev-latest", "jev-mini"],
    openrouter: ["liquid/d1.2", "liquid/d1.2-ultra", "perplexity/pplx-decider-v1.3:free", "zeta/decider-small"],
    ollama: ["system-one:1b", "system-one:7b"],
    custom: ["decider-large", "decider-small"],
  };
  // Terminals the office started (ADR-0103): the first one's CLI is the boss of the first workspace.
  const nowSec = () => Math.floor(Date.now() / 1000);
  const mockTerminals = [
    { id: 1, provider: "claude", cwd: "/work/app", exit: null, active: true, workspaces: ["s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50"], project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: null },
    { id: 2, provider: "codex", cwd: "/work/docs", exit: null, active: false, workspaces: [], project_key: "/work/docs", project_label: "docs", project_path: "/work/docs", display_title: "整理迁移文档" },
    {
      id: 3, provider: "claude", cwd: "/work/app/.agentnave-worktrees/wt-3f9a1c2b4d5e", exit: null, active: true,
      derived: true, derived_state: "running", derived_since: nowSec() - 95, derived_busy: true,
      has_draft: false, resumable: true, parent_terminal_id: 1, parent_workspace_id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50",
      desk_key: "thread:drv-run", native: "drv-run", isolation_id: "wt-3f9a1c2b4d5e", workspaces: [],
      project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "修登录竞态",
    },
    {
      id: 4, provider: "codex", cwd: "/work/app", exit: null, active: false,
      derived: true, derived_state: "waiting", derived_since: nowSec() - 42, derived_busy: false, blocked_kind: "permission",
      has_draft: false, resumable: true, parent_terminal_id: 1, parent_workspace_id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50",
      desk_key: "thread:drv-wait", native: "drv-wait", workspaces: [],
      project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "确认迁移脚本",
    },
    {
      id: 5, provider: "antigravity", cwd: "/work/app", exit: null, active: false,
      derived: true, derived_state: "uncollected", derived_since: nowSec() - 180, derived_busy: false, pending_results: 2,
      has_draft: false, resumable: true, parent_terminal_id: 1, parent_workspace_id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50",
      desk_key: "thread:drv-pick", native: "drv-pick", workspaces: [],
      project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "整理接口文档",
    },
    {
      id: 6, provider: "claude", cwd: "/work/app", exit: null, active: false,
      derived: true, derived_state: "idle", derived_since: nowSec() - 65, derived_busy: false,
      has_draft: false, resumable: true, parent_terminal_id: 1, parent_workspace_id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50",
      desk_key: "thread:drv-idle", native: "drv-idle", workspaces: [],
      project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "补单测覆盖",
    },
    {
      id: 7, provider: "codex", cwd: "/work/app", exit: 0, active: false,
      derived: true, derived_state: "dormant", derived_since: nowSec() - 320, derived_busy: false,
      has_draft: false, resumable: true, parent_terminal_id: 1, parent_workspace_id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50",
      desk_key: "thread:drv-dormant", native: "drv-dormant", workspaces: [],
      project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "重构缓存层",
    },
  ];
  const mockGitPr = () => {
    const ghStatus = q.get("gh") === "off" ? "not_installed" : "ready";
    const prFixLogin = {
      repo: "acme/app", number: 214, title: "fix: resolve OAuth callback race condition", state: "open", is_draft: false,
      additions: 148, deletions: 32,
      checks: { state: "failing", total: 4, passed: 2, failed: 1, pending: 1, checks_url: "https://github.com/acme/app/pull/214/checks" },
      review_decision: "changes_requested", mergeable: "mergeable",
      updated_at: new Date(Date.now() - 8 * 60000).toISOString(),
      url: "https://github.com/acme/app/pull/214", head_ref_name: "agentnave/fix-login", head_ref_oid: "abc123", base_ref_name: "main",
    };
    const prDocsMerged = {
      repo: "acme/app", number: 209, title: "docs: refresh architecture & release notes", state: "merged", is_draft: false,
      additions: 84, deletions: 19,
      checks: { state: "passing", total: 3, passed: 3, failed: 0, pending: 0, checks_url: "https://github.com/acme/app/pull/209/checks" },
      review_decision: "approved", mergeable: "unknown",
      updated_at: new Date(Date.now() - 2 * 3600000).toISOString(),
      url: "https://github.com/acme/app/pull/209", head_ref_name: "agentnave/docs-refresh", head_ref_oid: "def456", base_ref_name: "main",
    };
    const prDraft = {
      repo: "acme/docs", number: 77, title: "WIP: migration guide for v2", state: "draft", is_draft: true,
      additions: 230, deletions: 11,
      checks: { state: "pending", total: 2, passed: 1, failed: 0, pending: 1, checks_url: "https://github.com/acme/docs/pull/77/checks" },
      review_decision: "review_required", mergeable: "mergeable",
      updated_at: new Date(Date.now() - 25 * 60000).toISOString(),
      url: "https://github.com/acme/docs/pull/77", head_ref_name: "docs/v2-migration", head_ref_oid: "998877", base_ref_name: "main",
    };
    const prs = (list) => (ghStatus === "ready" ? list : []);
    return {
      gh_status: ghStatus,
      terminals: {
        "1": {
          git: { is_git: true, branch: "feat/auth-cleanup", detached: false, head_sha: "aa11", is_worktree: false, upstream: "origin/feat/auth-cleanup", ahead: 2, behind: 0, default_branch: "main", ahead_of_default: 2, integrated_into_default: false, changed_files: 3, staged_files: 1, unstaged_files: 2, untracked_files: 0, additions: 42, deletions: 9, github_repo: "acme/app" },
          prs: prs([prFixLogin]),
        },
        "2": {
          git: { is_git: true, branch: "docs/v2-migration", detached: false, head_sha: "998877", is_worktree: false, upstream: "origin/docs/v2-migration", ahead: 1, behind: 0, default_branch: "main", ahead_of_default: 1, integrated_into_default: false, changed_files: 1, staged_files: 0, unstaged_files: 1, untracked_files: 0, additions: 18, deletions: 3, github_repo: "acme/docs" },
          prs: prs([prDraft]),
        },
        "3": {
          git: { is_git: true, branch: "agentnave/fix-login", detached: false, head_sha: "abc123", is_worktree: true, upstream: "origin/agentnave/fix-login", ahead: 1, behind: 0, default_branch: "main", ahead_of_default: 3, integrated_into_default: false, changed_files: 2, staged_files: 0, unstaged_files: 1, untracked_files: 1, additions: 27, deletions: 5, github_repo: "acme/app" },
          prs: prs([prFixLogin]),
        },
      },
      workspaces: {
        "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50": {
          git: { is_git: true, branch: "feat/auth-cleanup", detached: false, head_sha: "aa11", is_worktree: false, upstream: "origin/feat/auth-cleanup", ahead: 2, behind: 0, default_branch: "main", ahead_of_default: 2, integrated_into_default: false, changed_files: 3, staged_files: 1, unstaged_files: 2, untracked_files: 0, additions: 42, deletions: 9, github_repo: "acme/app" },
          prs: prs([prFixLogin, prDocsMerged]),
        },
      },
      isolated: {
        "wt-3f9a1c2b4d5e": {
          git: { is_git: true, branch: "agentnave/fix-login", detached: false, head_sha: "abc123", is_worktree: true, upstream: "origin/agentnave/fix-login", ahead: 1, behind: 0, default_branch: "main", ahead_of_default: 3, integrated_into_default: false, changed_files: 2, staged_files: 0, unstaged_files: 1, untracked_files: 1, additions: 27, deletions: 5, github_repo: "acme/app" },
          pr: ghStatus === "ready" ? prFixLogin : null,
          merged_pr: null,
        },
        "wt-7b2e9d1a0c44": {
          git: { is_git: true, branch: "agentnave/docs-refresh", detached: false, head_sha: "def456", is_worktree: true, upstream: null, ahead: 0, behind: 0, default_branch: "main", ahead_of_default: 2, integrated_into_default: false, changed_files: 0, staged_files: 0, unstaged_files: 0, untracked_files: 0, additions: 0, deletions: 0, github_repo: "acme/app" },
          pr: ghStatus === "ready" ? prDocsMerged : null,
          merged_pr: ghStatus === "ready" ? { number: 209, repo: "acme/app", title: prDocsMerged.title, url: prDocsMerged.url } : null,
        },
        "wt-0c8d4e6f1a22": {
          git: { is_git: true, branch: "agentnave/old-experiment", detached: false, head_sha: "112233", is_worktree: true, upstream: null, ahead: 0, behind: 0, default_branch: "main", ahead_of_default: 1, integrated_into_default: false, changed_files: 1, staged_files: 0, unstaged_files: 1, untracked_files: 0, additions: 4, deletions: 1, github_repo: "acme/app" },
          pr: null,
          merged_pr: null,
        },
      },
      updated_at: Date.now(),
    };
  };
  if (q.has("handover")) mockTerminals.shift();
  function terminalPost(path, body) {
    const query = new URLSearchParams(path.split("?")[1] || "");
    const id = Number(query.get("session"));
    const route = path.split("?")[0];
    if (route === "/app/terminal/pick") return { cwd: "/work/app" };
    if (route === "/app/terminal/start" || route === "/app/terminal/handover") {
      if (route === "/app/terminal/handover") body.cwd = PLACES.find((p) => p.group === body.workspace)?.project_path || "/work/app";
      const manual = mockProjects.find((p) => p.key === body.cwd);
      const project = PLACES.find((p) => p.project_path === body.cwd)
        || (manual ? { project_key: manual.key, project_label: manual.label, project_path: manual.path } : { project_key: "other" });
      const account = mockAccounts.find((a) => a.id === body.account);
      const session = { id: Math.max(0, ...mockTerminals.map((t) => t.id)) + 1, provider: body.provider, account: account?.id ?? null, account_label: account?.label ?? null, cwd: body.cwd, exit: null, active: false, workspaces: [], display_title: null, ...project };
      mockTerminals.push(session);
      return { session };
    }
    const at = mockTerminals.findIndex((t) => t.id === id);
    if (route === "/app/terminal/stop" && at >= 0) {
      const target = mockTerminals[at];
      target.exit = 0;
      if (target.derived) {
        target.derived_state = "dormant";
        target.derived_busy = false;
        target.blocked_kind = null;
        target.derived_since = nowSec();
      }
    }
    if (route === "/app/terminal/wake") {
      if (at < 0 || !mockTerminals[at].resumable) {
        throw Object.assign(new Error("该派生会话不可恢复"), { status: 409 });
      }
      const target = mockTerminals[at];
      target.exit = null;
      target.derived_state = "idle";
      target.derived_busy = false;
      target.blocked_kind = null;
      target.derived_since = nowSec();
      return { session: structuredClone(target) };
    }
    if (route === "/app/terminal/open-derived") {
      const parent = mockTerminals.find((t) => !t.derived && t.workspaces?.includes(query.get("workspace")));
      const desk = query.get("desk");
      const session = {
        id: Math.max(0, ...mockTerminals.map((t) => t.id)) + 1, provider: "claude", cwd: parent?.cwd || "/work/app", exit: null, active: false,
        derived: true, derived_state: "idle", derived_since: nowSec(), derived_busy: false, has_draft: false, resumable: false,
        parent_terminal_id: parent?.id ?? null, parent_workspace_id: query.get("workspace"), desk_key: desk, native: desk.replace(/^thread:/, ""), workspaces: [],
        project_key: "/work/app", project_label: "app", project_path: "/work/app", display_title: "清理旧配置",
      };
      mockTerminals.push(session);
      return { session };
    }
    if (route === "/app/terminal/close" && at >= 0) {
      const [removed] = mockTerminals.splice(at, 1);
      if (removed && !removed.derived) {
        const wsSet = new Set(removed.workspaces || []);
        for (let i = mockTerminals.length - 1; i >= 0; i--) {
          const s = mockTerminals[i];
          if (s.derived && (s.parent_terminal_id === removed.id || wsSet.has(s.parent_workspace_id))) {
            mockTerminals.splice(i, 1);
          }
        }
      }
    }
    if (route === "/app/terminal/close-derived") {
      const workspace = query.get("workspace");
      let closed = 0;
      for (let i = mockTerminals.length - 1; i >= 0; i--) {
        if (mockTerminals[i].derived && mockTerminals[i].parent_workspace_id === workspace) {
          mockTerminals.splice(i, 1);
          closed += 1;
        }
      }
      return { closed };
    }
    if (route === "/app/terminal/resume") {
      if (q.has("elsewhere") && query.get("confirm") !== "1") {
        throw Object.assign(new Error("这段聊天正在Ghostty里打开（运行中）"), { status: 409, elsewhere: { app: "Ghostty", paused: false } });
      }
      const workspace = query.get("workspace");
      const session = { id: Math.max(0, ...mockTerminals.map((t) => t.id)) + 1, provider: "claude", cwd: "/work/app", exit: null, active: false, workspaces: [workspace], project_key: "/work/app", display_title: null };
      mockTerminals.push(session);
      return { session };
    }
    if (route === "/app/terminal/restart" && at >= 0) {
      const ended = mockTerminals[at];
      const resumes = ended.workspaces.length && ["claude", "codex", "grok", "antigravity"].includes(ended.provider);
      if (resumes && q.has("elsewhere") && query.get("confirm") !== "1") {
        throw Object.assign(new Error("这段聊天正在Ghostty里打开（运行中）"), { status: 409, elsewhere: { app: "Ghostty", paused: false } });
      }
      const session = { ...ended, id: Math.max(...mockTerminals.map((t) => t.id)) + 1, exit: null, workspaces: resumes ? ended.workspaces : [] };
      mockTerminals.splice(at, 1, session);
      return { session };
    }
    return { ok: true };
  }
  function projectPost(path, body) {
    if (path === "/app/projects/pick") return { cwd: "/work/sandbox" };
    if (path === "/app/projects/add") {
      const cwd = String(body.cwd || body.path || "").trim();
      if (!cwd) throw Object.assign(new Error("文件夹不存在"), { status: 400 });
      const known = PLACES.find((p) => p.project_key === cwd || p.project_path === cwd);
      const key = known ? known.project_key : cwd;
      const label = known?.project_label || cwd.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || cwd;
      const pathShown = known?.project_path || cwd;
      const existing = mockProjects.find((p) => p.key === key);
      if (existing) return { project: structuredClone(existing), added: false, projects: structuredClone(mockProjects) };
      if (chats().groups.some((g) => g.project_key === key) || mockTerminals.some((t) => t.project_key === key)) {
        return { project: { key, label, path: pathShown, added_at: Date.now() }, added: false, projects: structuredClone(mockProjects) };
      }
      const entry = { key, label, path: pathShown, added_at: Date.now() };
      mockProjects.push(entry);
      return { project: structuredClone(entry), added: true, projects: structuredClone(mockProjects) };
    }
    if (path === "/app/projects/remove") {
      const idx = mockProjects.findIndex((p) => p.key === body.key);
      if (idx >= 0) mockProjects.splice(idx, 1);
      return { removed: idx >= 0, projects: structuredClone(mockProjects) };
    }
    throw Object.assign(new Error("404"), { status: 404 });
  }
  async function p3Fetch(path) {
    if (path === "/app/projects") return { projects: structuredClone(mockProjects) };
    if (path === "/app/git-pr/state") return mockGitPr();
    if (path === "/app/isolated/cleanup" && q.has("isolated")) return mockCleanup();
    if (path === "/app/isolated/cleanup") return { running: false, report: null, recent: [] };
    if (path.split("?")[0] === "/app/terminal/state") return { sessions: structuredClone(mockTerminals), cwd: "/work/app" };
    if (path === "/app/terminal/clis") return { clis: ["claude", "codex", "antigravity", "grok"].map((provider) => ({ provider, accounts: provider === "claude" ? mockAccounts.map(({ id, label }) => ({ id, label })) : [] })) };
    if (path === "/app/accounts") return { accounts: structuredClone(mockAccounts), error: null };
    if (path === "/app/accounts/discover") {
      return { candidates: mockFoundDirs.filter((found) => !mockAccounts.some((a) => a.dir === found.dir)).map((found) => structuredClone(found)) };
    }
    if (path.startsWith("/app/accounts/status")) {
      const id = new URLSearchParams(path.split("?")[1]).get("id");
      return structuredClone(mockAccountStates.get(id) || { logged_in: false, method: "none", email: null, plan: null, error: null });
    }
    if (path === "/app/provider-health") return { providers: structuredClone(mockHealth), accounts: Object.fromEntries(mockAccounts.map((a) => [a.id, a.label])) };
    if (path.split("?")[0] === "/settings/members") return structuredClone(mockMembersData);
    if (path === "/app/orchestration") return structuredClone(mockOrchestration);
    if (path === "/app/decision-seats") return { ...structuredClone(mockDecision), key_set: mockDecision.keys[mockDecision.document?.decision_provider || "gemini"] === true };
    if (path.split("?")[0] === "/app/decision-models") {
      const provider = new URLSearchParams(path.split("?")[1]).get("provider");
      return { provider, models: (mockDecisionModels[provider] || []).map((id) => ({ id, name: id })) };
    }
    if (path === "/app/settings") return { settings: { version: 1, max_running: 8, history_retention_days: mockRetentionDays, claude_interactive: true, antigravity_interactive: true } };
    if (path === "/app/preferences") return preferencesView();
    if (path === "/app/characters" || path === "/avatars/avatar-settings") return {};
    if (path === "/app/storage") return { path: "/preview/.agentnave", bytes: 48234496, retention_days: mockRetentionDays };
    const storagePreview = /^\/app\/storage\/preview\?days=(\d+)$/.exec(path);
    if (storagePreview) {
      const days = Number(storagePreview[1]);
      return { workspaces: days <= 30 ? 3 : days <= 90 ? 1 : 0 };
    }
    if (path === "/app/voice/player") return structuredClone(mockPlayer);
    if (path === "/app/voice") return structuredClone(mockVoice);
    if (path === "/app/voice/cache") return { bytes: mockVoiceCacheBytes };
    if (path.startsWith("/app/voice/models/")) return [{ id: mockVoice.model, name: mockVoice.model }];
    if (path.startsWith("/app/voice/voices/")) return [{ id: "voice-1", name: "默认音色" }];
    if (path === "/app/hosts") return structuredClone(hostView());
    if (path === "/app/skills") return { skills: [{ name: "agentnave-review", title: "AgentNave 评审门控", description: "代码评审门控：创建或更新 PR、用户要求评审代码或合并到主分支前使用。", content: "---\nname: agentnave-review\n---\n\n# AgentNave 评审门控\n\n（预览）完整写法在 App 中读取内嵌的 SKILL.md。" }] };
    if (path === "/app/status" && installedState) return { ...status(), service: installedState };
    return mockFetch(path);
  }
  function accountPost(path, body) {
    const state = () => ({ accounts: structuredClone(mockAccounts), error: null });
    const route = path.split("?")[0];
    const at = mockAccounts.findIndex((a) => a.id === body.id);
    if (route === "/app/accounts/create") {
      const id = `a${Math.max(0, ...mockAccounts.map((a) => Number(a.id.slice(1)))) + 1}`;
      const dir = `/Users/preview/.agentnave-accounts/claude/${id}`;
      const account = { id, provider: "claude", label: body.label, dir, managed: true };
      mockAccounts.push(account);
      mockAccountStates.set(id, { logged_in: false, method: "none", email: null, plan: null, error: null });
      return { account, sync: { ok: true, message: "" }, state: state() };
    }
    if (route === "/app/accounts/link") {
      const dir = String(body.dir || "").replace(/^~/, "/Users/preview");
      if (!dir.startsWith("/Users/preview/.claude") || mockAccounts.some((a) => a.dir === dir)) {
        throw Object.assign(new Error(mockAccounts.some((a) => a.dir === dir) ? "这个目录已关联" : "目录不存在"), { status: 400 });
      }
      const id = `a${Math.max(0, ...mockAccounts.map((a) => Number(a.id.slice(1)))) + 1}`;
      const account = { id, provider: "claude", label: body.label, dir, managed: false };
      const found = mockFoundDirs.find((item) => item.dir === dir);
      mockAccounts.push(account);
      mockAccountStates.set(id, found ? structuredClone(found.state) : { logged_in: false, method: "none", email: null, plan: null, error: null });
      return { account, sync: { ok: true, message: "" }, state: state() };
    }
    if (route === "/app/accounts/rename" && at >= 0) { mockAccounts[at].label = body.label; return { state: state() }; }
    if (route === "/app/accounts/remove" && at >= 0) {
      mockAccountStates.delete(body.id);
      mockAccounts.splice(at, 1);
      return { state: state(), deleted: Boolean(body.delete_dir), delete_error: null, unsync_error: null };
    }
    if (route === "/app/accounts/login" && (body.id === "default" || at >= 0)) {
      const session = nextLoginSession++;
      mockLoginAccounts.set(session, body.id);
      return { session };
    }
    throw Object.assign(new Error("404"), { status: 404 });
  }
  async function post(path, body = {}) {
    if (path === "/app/git-pr/refresh") return mockGitPr();
    if (path === "/app/git-pr/open-url") {
      const url = String(body.url || "");
      if (!url.startsWith("https://github.com/")) {
        throw Object.assign(new Error("仅允许打开 https://github.com/ 开头的链接"), { status: 400 });
      }
      window.__mockOpenedUrls = window.__mockOpenedUrls || [];
      window.__mockOpenedUrls.push(url);
      return { ok: true, url };
    }
    if (path === "/app/git-pr/prepare") {
      const op = body.op;
      if (op === "commit") {
        return {
          op: "commit", branch: "feat/auth-cleanup", detached: false, working: false,
          files: [
            { path: "apps/tray/src/git_pr/ops.rs", code: "M.", staged: true, unstaged: false, untracked: false, unmerged: false },
            { path: "apps/tray/ui/chat-git.js", code: ".M", staged: false, unstaged: true, untracked: false, unmerged: false },
            { path: "apps/tray/ui/chat-git-ops.js", code: "??", staged: false, unstaged: false, untracked: true, unmerged: false },
          ],
          has_staged: true, has_unstaged: true, has_unmerged: false,
        };
      }
      if (op === "pull") {
        return { op: "pull", branch: "feat/auth-cleanup", detached: false, upstream: "origin/feat/auth-cleanup", ahead: 2, behind: 1, working: false };
      }
      if (op === "push") {
        return { op: "push", branch: "feat/auth-cleanup", remote: "origin", remote_branch: "feat/auth-cleanup", set_upstream: false, upstream: "origin/feat/auth-cleanup", ahead: 2, behind: 0, working: false };
      }
      if (op === "create_pr") {
        return { op: "create_pr", repo: "acme/app", head: "feat/auth-cleanup", base: "main", title: "feat: clean up OAuth session flow", body: "- extract token refresh helper\n- add race condition guard", remote: "origin", needs_push: true, working: false };
      }
      if (op === "merge_pr") {
        return { op: "merge_pr", repo: body.repo || "acme/app", number: body.number || 214, allowed_methods: ["squash", "rebase"], default_method: "squash", delete_branch_on_merge: true, working: false };
      }
      throw Object.assign(new Error(`unsupported op: ${op}`), { status: 400 });
    }
    if (path === "/app/git-pr/commit") {
      if (q.get("gitfail") === "commit") throw Object.assign(new Error("error: pathspec failed"), { status: 400 });
      return { ok: true, sha: "9f3a2c1", output: "[feat/auth-cleanup 9f3a2c1] committed" };
    }
    if (path === "/app/git-pr/pull") {
      if (q.get("gitfail") === "pull") throw Object.assign(new Error("分支已分叉，无法快进拉取（未自动合并或变基）：\nfatal: Not possible to fast-forward, aborting."), { status: 409 });
      return { ok: true, branch: "feat/auth-cleanup", upstream: "origin/feat/auth-cleanup", up_to_date: false, sha: "7c4b8e2", output: "Fast-forward" };
    }
    if (path === "/app/git-pr/push") {
      if (q.get("gitfail") === "push") throw Object.assign(new Error("! [rejected] feat/auth-cleanup -> feat/auth-cleanup (non-fast-forward)"), { status: 400 });
      return { ok: true, branch: "feat/auth-cleanup", remote: "origin", upstream: "origin/feat/auth-cleanup", set_upstream: false, output: "Everything up-to-date" };
    }
    if (path === "/app/git-pr/pr/create") {
      return { ok: true, repo: "acme/app", head: "feat/auth-cleanup", base: "main", url: "https://github.com/acme/app/pull/215", number: 215 };
    }
    if (path === "/app/git-pr/pr/merge") {
      return { ok: true, repo: body.repo || "acme/app", number: body.number || 214, method: body.method || "squash", delete_branch_on_merge: true, output: "Merged" };
    }
    if (path === "/app/isolated/inspect") {
      const iso = mockGitPr().isolated?.[body.id];
      const d = ISOLATED.find((x) => x.id === body.id);
      return {
        status: "confirm", id: body.id, path: d?.path || "", branch: d?.branch || "", confirmation: "mock-digest",
        uncommitted: Boolean(iso?.git?.changed_files), uncommitted_files: iso?.git?.changed_files ? ["src/auth.rs"] : [], uncommitted_total: iso?.git?.changed_files || 0,
        unique_commits: Boolean(iso?.git?.ahead_of_default), unmerged_commits: ["abc1234 feat: update"], unmerged_total: iso?.git?.ahead_of_default || 0,
        merged_pr: iso?.merged_pr || null,
      };
    }
    if (path === "/app/isolated/cleanup/run") {
      mockCleanupRan = true;
      return mockCleanup();
    }
    if (path === "/app/isolated/delete") return { status: "deleted", id: body.id, message: "已删除隔离目录" };
    if (path.startsWith("/app/projects/")) return projectPost(path, body);
    if (path.startsWith("/app/terminal/")) return terminalPost(path, body);
    if (path.startsWith("/app/accounts/")) return accountPost(path, body);
    if (path === "/app/voice/player") { mockPlayer = structuredClone(body); window.dispatchEvent(new CustomEvent("agentnave:voice-player", { detail: mockPlayer })); return { ok: true }; }
    if (path === "/app/voice/player/command") { window.dispatchEvent(new CustomEvent("agentnave:voice-command", { detail: body })); return { ok: true, opening: false }; }
    if (path.split("?")[0] === "/settings/members") return p3Fetch(path);
    if (path === "/app/provider-health/clear") { const entry = mockHealth[body.provider]; if (entry) { if (body.model) delete entry.models?.[body.model]; else if (body.group) delete entry.groups?.[body.group]; else delete entry.default; } return { cleared: true }; }
    if (path === "/app/members") { mockMembersData.document = structuredClone(body); return { saved: true }; }
    if (path === "/app/review") { mockMembersData.review = structuredClone(body); return { saved: true }; }
    if (path === "/app/permissions") { mockMembersData.permissions = structuredClone(body); return { saved: true }; }
    if (path === "/app/decision-seats") { mockDecision.document = structuredClone(body); return { saved: true }; }
    if (path === "/app/decision-key") {
      const provider = body.provider || "gemini";
      mockDecision.keys[provider] = Boolean(body.key);
      if (body.key) mockDecision.key_previews[provider] = maskKey(body.key);
      else delete mockDecision.key_previews[provider];
      return { provider, key_set: mockDecision.keys[provider], keys: { ...mockDecision.keys }, key_previews: { ...mockDecision.key_previews } };
    }
    if (path === "/app/orchestration") { mockOrchestration.document = structuredClone(body); return { ...structuredClone(mockOrchestration), failures: [] }; }
    if (path === "/app/orchestration/host-cli") {
      const choices = { ...mockOrchestration.document.host_cli };
      for (const key of body.keys) {
        const def = key.split("@")[0] === "antigravity" ? "launch" : "subagent";
        if (body.choice === def) delete choices[key]; else choices[key] = body.choice;
      }
      mockOrchestration.document.host_cli = choices;
      return { saved: true, host_cli: choices };
    }
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
    if (path === "/app/preferences") {
      for (const [section, fields] of Object.entries(body)) mockPrefs[section] = { ...mockPrefs[section], ...fields };
      return { saved: true, ...preferencesView() };
    }
    if (path === "/app/alerts/test") return { sent: true };
    if (path === "/app/storage/open") return { opened: true };
    if (path === "/app/workspaces/handover") {
      mockHandovers.set(body.group, "pending");
      return { id: body.group, status: "waiting", message: "已标为待接手。在任意宿主开一个新聊天，请它接手这个 agent" };
    }
    if (path === "/app/workspaces/cancel-handover") {
      if (handoverOf({ group: body.group }) !== "pending") return { id: body.group, status: "not_pending", message: "已被接手或已取消，刷新后再看" };
      mockHandovers.set(body.group, null);
      return { id: body.group, status: "cancelled", message: "已取消交接" };
    }
    if (path === "/app/settings") {
      if (q.has("preview")) throw Object.assign(new Error("预览版不修改正式数据"), { status: 403 });
      if (Object.hasOwn(body, "history_retention_days")) mockRetentionDays = body.history_retention_days;
      return { settings: { version: 1, max_running: 8, history_retention_days: mockRetentionDays, claude_interactive: true, antigravity_interactive: true } };
    }
    if (path === "/app/voice") {
      Object.assign(mockVoice, body);
      if (body.summary) mockVoice.summary = { ...mockVoice.summary, ...body.summary };
      return structuredClone(mockVoice);
    }
    if (path === "/app/voice/key") {
      mockVoice.keys[body.engine] = Boolean(body.key);
      if (body.key) mockVoice.key_previews[body.engine] = maskKey(body.key);
      else delete mockVoice.key_previews[body.engine];
      return structuredClone(mockVoice);
    }
    if (path === "/app/voice/summary/models") return [{ id: "gpt-5.4-mini", owned_by: "openai", reasoning_levels: ["low", "medium"] }];
    if (path === "/app/voice/cache/clear") {
      mockVoiceCacheBytes = 0;
      return { cleared: true };
    }
    if (path === "/app/hosts/refresh") return structuredClone(hostView());
    if (path === "/app/hosts/reveal-backups") return { ok: true, message: "预览：已打开备份目录" };
    if (path === "/app/service/install") { installedState = { state: "installing" }; setTimeout(() => { serviceReady = true; installedState = { state: "done" }; }, 900); return { ok: true }; }
    const host = hostRows.find((h) => h.id === body.id);
    if (!host) throw new Error("未知宿主");
    if (path.endsWith("/reload-copy")) {
      // &copyfail makes every one-click action fail, to show the card's error note.
      if (q.has("copyfail")) return { ok: false, message: "宿主未能完成操作，请在宿主中检查后台服务状态", host: structuredClone(host) };
      const r = RELOADS[host.id];
      if (r) r.copies = r.copies.filter((copy) => copy.kind !== body.kind);
      return { ok: true, message: body.kind === "daemon" && host.id === "claude" ? "已停止 Claude 后台服务" : `预览：已重启 ${host.label} 的${body.kind === "gateway" ? "网关" : "后台服务"}`, host: structuredClone(host) };
    }
    if (path.endsWith("/stop-copy")) {
      if (q.has("copyfail")) return { ok: false, message: "该终端会话已变化或不再是旧副本，已刷新状态", host: structuredClone(host) };
      const r = RELOADS[host.id];
      if (r) r.copies = r.copies.filter((copy) => !(copy.kind === "cli" && copy.pid === body.pid && copy.since === body.since));
      return { ok: true, message: "预览：已结束终端会话", host: structuredClone(host) };
    }
    if (path.endsWith("/dismiss-fault")) {
      host.fault = null;
      return { ok: true, message: "预览：已清除连接中断提示" };
    }
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
    // Connecting a boss confirms its CLI as an employee.
    if (freshHosts && host.status === "connected" && mockMembersData.detected[host.id]) {
      for (const scene of ["task", "discussion"]) (mockMembersData.document.modes[scene] ||= {})[host.id] ||= { models: [], overrides: {} };
    }
    host.attention = null;
    host.last_action = { ok: true, message: "预览操作完成", at: new Date().toISOString() };
    return { host: structuredClone(host), ok: true, message: host.last_action.message };
  }
  AN.href = href;
  const realFetch = window.fetch.bind(window);
  window.fetch = async (input, options = {}) => {
    const url = new URL(input, location.href);
    const path = url.pathname + url.search;
    if (!url.pathname.startsWith("/app/") && url.pathname !== "/settings/members" && url.pathname !== "/avatars/avatar-settings") return realFetch(input, options);
    try {
      const value = options.method === "POST" ? await post(path, JSON.parse(options.body || "{}")) : await p3Fetch(path);
      return new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });
    } catch (error) {
      return new Response(JSON.stringify({ error: error.message, ...(error.elsewhere ? { elsewhere: error.elsewhere } : {}) }), { status: error.status || 500 });
    }
  };
  // Static links and links created later follow the same routing as AN.href.
  document.addEventListener("click", (event) => {
    const link = event.target.closest("a[href]");
    if (!link) return;
    const path = link.getAttribute("href");
    if (path.startsWith("/app/")) event.preventDefault();
    else if (href(path) !== path) link.setAttribute("href", href(path));
  }, true);
  if (q.has("gitopen")) {
    const mode = q.get("gitopen");
    const tryOpen = (tries = 0) => {
      if (tries > 40) return;
      const headBtn = document.querySelector(".fh-git:not([hidden])");
      const badge = document.querySelector(".gi-pr:not([hidden])");
      if (!headBtn && !badge) return setTimeout(() => tryOpen(tries + 1), 120);
      if (mode === "card") {
        (headBtn || badge).click();
      } else if (mode === "sidecard" && badge) {
        badge.click();
      } else if (mode === "commit" && typeof ChatGitOps !== "undefined") {
        void ChatGitOps.openCommit({ kind: "workspace", id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50" });
      } else if (mode === "create_pr" && typeof ChatGitOps !== "undefined") {
        void ChatGitOps.openCreatePr({ kind: "workspace", id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50" });
      } else if (mode === "merge_pr" && typeof ChatGitOps !== "undefined") {
        void ChatGitOps.openMergePr({ kind: "workspace", id: "s-3f9c1d7a5b2e4c808f6d9a1b7e2c4d50" }, mockGitPr().terminals["1"].prs[0]);
      } else if (mode === "isolated") {
        document.querySelector(".fh-sum .pill.act:not(.fh-git):not([hidden])")?.click();
      }
    };
    window.addEventListener("load", () => setTimeout(() => tryOpen(0), 400));
  }
})();
