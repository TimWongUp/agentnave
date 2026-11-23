"use strict";
// The automatic isolated-directory cleanup's results for the isolated-directory list (ADR-0115 §6):
// a "clean now" button, what the passes removed or collapsed, and why a directory stayed.
// The tray keeps the results in memory; this file only reads them.
const ChatIsolatedCleanup = (() => {
  const { h, L, relTime } = AN;
  const REFRESH_MS = 30000;
  const SHOWN = 8;

  const store = { report: null, recent: [], running: false, version: 0, loadedAt: 0, inflight: false };

  const OUTCOME = {
    removed: () => L("已删除", "Deleted"),
    collapsed: () => L("已收起", "Collapsed"),
    failed: () => L("失败", "Failed"),
  };
  const RULE = {
    merged: () => L("PR 已合并", "PR merged"),
    unchanged: () => L("无新提交", "No new commits"),
    idle: () => L("闲置", "Idle"),
    workspace_deleted: () => L("工作区已删除", "Workspace deleted"),
  };
  const REASON = {
    not_ready: () => L("状态未就绪", "Not ready"),
    busy: () => L("CLI 正在使用", "In use by a CLI"),
    leased: () => L("任务持有租约", "Held by a task"),
    recent: () => L("刚用过", "Used moments ago"),
    dirty: () => L("有未提交改动", "Uncommitted changes"),
    nested: () => L("含嵌套仓库", "Contains a nested repository"),
    detached: () => L("已脱离受管分支", "Detached from its branch"),
    orphan: () => L("不是有效的 worktree", "Not a valid worktree"),
    unverified: () => L("无法核对", "Could not verify"),
    locked: () => L("仓库正忙", "Repository busy"),
    changed: () => L("检查期间有变化", "Changed during the check"),
    no_match: () => L("没有符合的规则", "No rule applies"),
  };

  function ruleText(entry) {
    const word = RULE[entry.rule]?.() || "";
    return entry.rule === "merged" && entry.detail ? `${word} ${entry.detail}` : word;
  }

  function keptReason(id) {
    const entry = store.report?.entries?.find((e) => e.id === id && e.outcome === "kept");
    if (!entry) return "";
    const word = REASON[entry.reason]?.() || entry.reason;
    return entry.reason === "unverified" && entry.detail ? `${word}：${entry.detail}` : word;
  }

  const doneOf = (group) => store.recent.filter((e) => e.workspace === group);
  const doneCount = (group) => doneOf(group).length;

  function changed() {
    store.version += 1;
    if (typeof renderList === "function") renderList();
    if (typeof state !== "undefined" && state?.drawer?.kind === "isolated" && state.ctx && typeof renderIsolatedDrawer === "function") {
      renderIsolatedDrawer(state.chat, state.ctx);
    }
  }

  function apply(next) {
    store.report = next?.report || null;
    store.recent = Array.isArray(next?.recent) ? next.recent : [];
    store.running = Boolean(next?.running);
    store.loadedAt = Date.now();
    changed();
  }

  async function load(force = false) {
    if (store.inflight || (!force && Date.now() - store.loadedAt < REFRESH_MS)) return;
    store.inflight = true;
    try {
      apply(await AN.load("/app/isolated/cleanup"));
    } catch {
      /* The list works without results. */
    } finally {
      store.inflight = false;
    }
  }

  async function runNow() {
    if (store.running) return;
    store.running = true;
    changed();
    try {
      apply(await AN.post("/app/isolated/cleanup/run", {}));
      if (typeof refresh === "function") void refresh();
    } catch (error) {
      store.running = false;
      changed();
      if (typeof Actions !== "undefined") Actions.say?.(error.message || L("清理失败", "Cleanup failed"), true);
    }
  }

  // The button and the last pass, then what recent passes did to this workspace's directories.
  function panel(group) {
    const button = h("button.iso-clean-btn", { type: "button", disabled: store.running }, store.running ? L("清理中…", "Cleaning…") : L("立即清理", "Clean now"));
    button.addEventListener("click", () => void runNow());
    const last = store.report ? L(`上次清理 ${relTime(store.report.at)}`, `Last cleanup ${relTime(store.report.at)}`) : "";
    const lines = doneOf(group).slice(0, SHOWN).map((e) =>
      h("div.iso-clean-line", { title: e.detail && e.outcome === "failed" ? e.detail : "" },
        h(`span.iso-clean-word.${e.outcome}`, {}, OUTCOME[e.outcome]?.() || e.outcome),
        h("span.iso-clean-branch", {}, e.branch),
        h("span.iso-clean-why", {}, [ruleText(e), relTime(e.at)].filter(Boolean).join(" · ")),
      ));
    return h("div.iso-clean", {},
      h("div.iso-clean-head", {}, h("span.iso-clean-last", {}, last), button),
      ...lines,
    );
  }

  function ensureLoaded() {
    void load();
  }

  if (typeof window !== "undefined" && window.addEventListener) {
    window.addEventListener("focus", () => void load());
    window.addEventListener("agentnave:focus", () => void load());
  }
  void load(true);

  return { panel, keptReason, doneCount, ensureLoaded, version: () => store.version, _applyForTest: apply };
})();
