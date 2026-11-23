"use strict";
// Read-only Git workspace & GitHub PR status for CLI sessions and isolated directories (ADR-0115).
// No background polling: refreshes on initial load, window focus, and when a turn finishes.
const ChatGit = (() => {
  const { h, L, setText, setAttr, relTime } = AN;
  const SVG_NS = "http://www.w3.org/2000/svg";
  const CLOSE_DELAY_MS = 180;
  const FOCUS_COOLDOWN_MS = 2000;

  const store = {
    snap: { gh_status: "unknown", terminals: {}, workspaces: {}, isolated: {}, updated_at: 0 },
    version: 0,
    loaded: false,
    inflight: false,
    queuedReason: null,
    timer: 0,
    lastFocusAt: 0,
    prevWsBusy: new Map(),
    prevTermBusy: new Map(),
    selected: null,
  };

  // ---- small SVG glyphs (standard Git/PR UI symbols, not brand logos) ---------------------
  function icon(kind) {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("class", `gp-ico gp-ico-${kind}`);
    const add = (tag, attrs) => {
      const node = document.createElementNS(SVG_NS, tag);
      for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
      svg.append(node);
    };
    if (kind === "branch") {
      add("circle", { cx: "4.5", cy: "3.5", r: "1.6" });
      add("circle", { cx: "4.5", cy: "12.5", r: "1.6" });
      add("circle", { cx: "11.5", cy: "5.5", r: "1.6" });
      add("path", { d: "M4.5 5.1v5.8M11.5 7.1c0 2.5-2.2 3.4-7 3.4" });
    } else if (kind === "merged") {
      add("circle", { cx: "4.5", cy: "3.5", r: "1.6" });
      add("circle", { cx: "4.5", cy: "12.5", r: "1.6" });
      add("circle", { cx: "11.5", cy: "10.5", r: "1.6" });
      add("path", { d: "M4.5 5.1v5.8M4.5 5.3c0 2.6 2.1 5.2 5.4 5.2" });
    } else if (kind === "closed") {
      add("circle", { cx: "4.5", cy: "3.5", r: "1.6" });
      add("circle", { cx: "4.5", cy: "12.5", r: "1.6" });
      add("circle", { cx: "11.5", cy: "12.5", r: "1.6" });
      add("path", { d: "M4.5 5.1v5.8M9.5 2.5l4 4M13.5 2.5l-4 4M11.5 8.5v2.4" });
    } else {
      // open / draft PR
      add("circle", { cx: "4.5", cy: "3.5", r: "1.6" });
      add("circle", { cx: "4.5", cy: "12.5", r: "1.6" });
      add("circle", { cx: "11.5", cy: "12.5", r: "1.6" });
      add("path", { d: "M4.5 5.1v5.8M9 3.5h1.2a1.3 1.3 0 0 1 1.3 1.3v6.1" });
    }
    return svg;
  }

  // ---- eligibility & status priority ------------------------------------------------------
  function isCliSession(g) {
    if (!g) return false;
    if (g.terminal != null || g.derived) return true;
    if (typeof ChatTerminal !== "undefined" && ChatTerminal?.hasTerminal?.(g.group)) return true;
    return g.cli_session === true;
  }

  function prPriority(pr) {
    const active = pr.state === "open" || pr.state === "draft";
    if (active && (pr.checks?.state === "failing" || pr.mergeable === "conflicting")) return 5;
    if (pr.state === "open") return 4;
    if (pr.state === "draft") return 3;
    if (pr.state === "merged") return 2;
    return 1;
  }

  function prTone(pr) {
    const rank = prPriority(pr);
    if (rank === 5) return "bad";
    if (rank === 4) return "open";
    if (rank === 3) return "draft";
    if (rank === 2) return "merged";
    return "closed";
  }

  function badgeSummary(entry) {
    if (!entry) return null;
    const ghReady = store.snap.gh_status === "ready";
    const prs = ghReady && Array.isArray(entry.prs) ? entry.prs : [];
    if (prs.length) {
      let best = prs[0];
      for (const pr of prs) if (prPriority(pr) > prPriority(best)) best = pr;
      const tone = prTone(best);
      return { kind: "pr", tone, count: prs.length, topState: best.state, prs, git: entry.git };
    }
    if (entry.git?.is_git) {
      const dirty = entry.git.changed_files || 0;
      const ahead = entry.git.ahead_of_default || 0;
      return { kind: "git", tone: "git", dirty, ahead, prs: [], git: entry.git };
    }
    return null;
  }

  function entryOf(g) {
    if (!g || !isCliSession(g)) return null;
    if (g.terminal != null) return store.snap.terminals?.[String(g.terminal)] || null;
    return store.snap.workspaces?.[g.group] || null;
  }

  function isolatedOf(id) {
    return (id && store.snap.isolated?.[id]) || null;
  }

  // ---- floating hover card ----------------------------------------------------------------
  const card = h("div.git-pr-card", { role: "dialog", hidden: true });
  let cardAnchor = null;
  let closeTimer = 0;

  function ensureCardMounted() {
    if (typeof document === "undefined" || !document.body || card.isConnected) return;
    document.body.append(card);
    card.addEventListener("pointerenter", () => clearTimeout(closeTimer));
    card.addEventListener("pointerleave", hideCardSoon);
    document.addEventListener("pointerdown", (e) => {
      if (!card.hidden && !card.contains(e.target) && !cardAnchor?.contains?.(e.target)) hideCard();
    }, true);
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !card.hidden) hideCard();
    });
  }

  function hideCard() {
    clearTimeout(closeTimer);
    card.hidden = true;
    cardAnchor = null;
  }

  function hideCardSoon() {
    clearTimeout(closeTimer);
    closeTimer = setTimeout(hideCard, CLOSE_DELAY_MS);
  }

  function prStateLabel(state) {
    if (state === "draft") return L("草稿", "Draft");
    if (state === "merged") return L("已合并", "Merged");
    if (state === "closed") return L("已关闭", "Closed");
    return L("开着", "Open");
  }

  function reviewLabel(decision) {
    if (decision === "approved") return [L("已批准", "Approved"), "good"];
    if (decision === "changes_requested") return [L("需修改", "Changes requested"), "bad"];
    if (decision === "review_required") return [L("待评审", "Review required"), "warn"];
    return null;
  }

  function checksLabel(checks) {
    if (!checks || !checks.total || checks.state === "none") return null;
    if (checks.state === "failing") return [L(`CI 失败 ${checks.failed}/${checks.total}`, `CI failed ${checks.failed}/${checks.total}`), "bad"];
    if (checks.state === "pending") return [L(`CI 进行中 ${checks.pending}/${checks.total}`, `CI pending ${checks.pending}/${checks.total}`), "warn"];
    return [L(`CI 通过 (${checks.passed})`, `CI passed (${checks.passed})`), "good"];
  }

  function buildGitSection(git) {
    if (!git?.is_git) return null;
    const branchText = git.branch || (git.detached ? L("游离 HEAD", "Detached HEAD") : "HEAD");
    const syncParts = [];
    if (git.upstream || git.ahead || git.behind) {
      if (git.ahead) syncParts.push(`↑${git.ahead}`);
      if (git.behind) syncParts.push(`↓${git.behind}`);
      if (git.upstream && !git.ahead && !git.behind) syncParts.push(L("与上游同步", "Synced"));
    }
    if (git.default_branch && git.ahead_of_default > 0 && git.branch !== git.default_branch) {
      syncParts.push(L(`领先 ${git.default_branch} ${git.ahead_of_default}`, `${git.ahead_of_default} ahead of ${git.default_branch}`));
    }
    const dirtyNodes = [];
    if (git.changed_files > 0) {
      dirtyNodes.push(h("span.gp-dirty", {}, L(`${git.changed_files} 个改动`, `${git.changed_files} changed`)));
      if (git.additions) dirtyNodes.push(h("span.gp-add", {}, `+${git.additions}`));
      if (git.deletions) dirtyNodes.push(h("span.gp-del", {}, `-${git.deletions}`));
    } else {
      dirtyNodes.push(h("span.gp-clean", {}, L("工作区干净", "Clean")));
    }
    return h(
      "div.gp-git",
      {},
      h(
        "div.gp-git-top",
        {},
        icon("branch"),
        h("code.gp-branch", {}, branchText),
        git.is_worktree ? h("span.gp-tag", {}, "worktree") : null,
      ),
      h(
        "div.gp-git-stats",
        {},
        ...dirtyNodes,
        ...syncParts.map((text) => h("span.gp-sync", {}, text)),
      ),
    );
  }

  function buildPrRow(pr) {
    const tone = prTone(pr);
    const prIconKind = pr.state === "merged" ? "merged" : pr.state === "closed" ? "closed" : "open";
    const chips = [];
    const ci = checksLabel(pr.checks);
    if (ci) {
      const ciUrl = pr.checks?.checks_url || `${pr.url}/checks`;
      chips.push(
        h(
          `a.gp-chip.gp-tone-${ci[1]}`,
          { href: ciUrl, target: "_blank", rel: "noopener" },
          ci[0],
        ),
      );
    }
    const rev = reviewLabel(pr.review_decision);
    if (rev) chips.push(h(`span.gp-chip.gp-tone-${rev[1]}`, {}, rev[0]));
    if (pr.mergeable === "conflicting" && (pr.state === "open" || pr.state === "draft")) {
      chips.push(h("span.gp-chip.gp-tone-bad", {}, L("有冲突", "Conflicts")));
    }
    const diffNodes = [];
    if (pr.additions || pr.deletions) {
      diffNodes.push(h("span.gp-add", {}, `+${pr.additions}`));
      diffNodes.push(h("span.gp-del", {}, `-${pr.deletions}`));
    }
    return h(
      "div.gp-pr",
      {},
      h(
        "a.gp-pr-head",
        { href: pr.url, target: "_blank", rel: "noopener" },
        h(`span.gp-pr-ico.gp-tone-${tone}`, {}, icon(prIconKind)),
        h("span.gp-pr-title", {}, pr.title || `#${pr.number}`),
      ),
      h(
        "div.gp-pr-meta",
        {},
        h("code.gp-pr-ref", {}, `${pr.repo}#${pr.number}`),
        h(`span.gp-pr-state.gp-tone-${tone}`, {}, prStateLabel(pr.state)),
        ...diffNodes,
        pr.updated_at && AN.at(pr.updated_at) ? h("time.gp-pr-time", {}, relTime(pr.updated_at)) : null,
      ),
      chips.length ? h("div.gp-pr-chips", {}, ...chips) : null,
    );
  }

  function showCard(anchor, entry) {
    ensureCardMounted();
    if (!anchor || !entry) return;
    clearTimeout(closeTimer);
    const ghReady = store.snap.gh_status === "ready";
    const prs = ghReady && Array.isArray(entry.prs) ? entry.prs : [];
    const gitNode = buildGitSection(entry.git);
    const prNodes = prs.map(buildPrRow);
    if (!gitNode && !prNodes.length) return;

    cardAnchor = anchor;
    card.replaceChildren(
      gitNode,
      prNodes.length ? h("div.gp-prs", {}, ...prNodes) : null,
    );
    card.hidden = false;
    positionCard(anchor);
  }

  function positionCard(anchor) {
    if (typeof window === "undefined" || !anchor.getBoundingClientRect) return;
    const rect = anchor.getBoundingClientRect();
    const cw = card.offsetWidth || 320;
    const ch = card.offsetHeight || 180;
    const margin = 8;
    const inSide = Boolean(anchor.closest?.(".side"));
    let left = inSide ? rect.right + 8 : rect.right - cw;
    let top = inSide ? rect.top - 4 : rect.bottom + 6;
    if (left + cw > window.innerWidth - margin) left = Math.max(margin, window.innerWidth - cw - margin);
    if (left < margin) left = margin;
    if (top + ch > window.innerHeight - margin) top = Math.max(margin, window.innerHeight - ch - margin);
    if (top < margin) top = margin;
    card.style.left = `${Math.round(left)}px`;
    card.style.top = `${Math.round(top)}px`;
  }

  function bindHoverTrigger(node, getEntry) {
    if (node._gpBound) {
      node._gpGetEntry = getEntry;
      return;
    }
    node._gpBound = true;
    node._gpGetEntry = getEntry;
    node.addEventListener("pointerenter", () => {
      const entry = node._gpGetEntry?.();
      if (entry) showCard(node, entry);
    });
    node.addEventListener("pointerleave", hideCardSoon);
    node.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const entry = node._gpGetEntry?.();
      if (!entry) return;
      if (!card.hidden && cardAnchor === node) hideCard();
      else showCard(node, entry);
    });
    node.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      e.stopPropagation();
      const entry = node._gpGetEntry?.();
      if (entry) showCard(node, entry);
    });
  }

  // ---- sidebar row decoration -------------------------------------------------------------
  function decorateItem(rec, g) {
    if (!rec?.el) return;
    if (!rec.prBadge) {
      rec.prBadge = h("span.gi-pr", { role: "button", tabindex: "0", hidden: true });
      const top = rec.top || rec.pin?.parentNode || rec.label?.parentNode;
      if (top) top.append(rec.prBadge);
    }
    const entry = entryOf(g);
    const summary = badgeSummary(entry);
    if (!summary) {
      rec.prBadge.hidden = true;
      return;
    }
    rec.prBadge.hidden = false;
    setAttr(rec.prBadge, "data-tone", summary.tone);
    const key = summary.kind === "pr"
      ? `pr:${summary.tone}:${summary.topState}:${summary.count}`
      : `git:${summary.dirty}:${summary.ahead}`;
    if (rec.prBadgeKey !== key) {
      rec.prBadgeKey = key;
      if (summary.kind === "pr") {
        const icoKind = summary.topState === "merged" ? "merged" : summary.topState === "closed" ? "closed" : "open";
        rec.prBadge.replaceChildren(icon(icoKind), h("span.gi-pr-n", {}, String(summary.count)));
      } else {
        const suffix = summary.dirty ? `*${summary.dirty}` : summary.ahead ? `↑${summary.ahead}` : "";
        rec.prBadge.replaceChildren(icon("branch"), suffix ? h("span.gi-pr-n", {}, suffix) : null);
      }
    }
    const aria = summary.kind === "pr"
      ? L(`${summary.count} 个关联 PR`, `${summary.count} linked PR(s)`)
      : L(`Git 分支 ${summary.git.branch || "HEAD"}`, `Git branch ${summary.git.branch || "HEAD"}`);
    setAttr(rec.prBadge, "aria-label", aria);
    bindHoverTrigger(rec.prBadge, () => entryOf(g));
  }

  // ---- office floor header pill -----------------------------------------------------------
  function decorateHead(headRec, c) {
    if (!headRec) return;
    if (!headRec.git) {
      headRec.git = h("button.pill.act.fh-git", { type: "button", hidden: true });
      headRec.iso?.parentNode?.insertBefore(headRec.git, headRec.iso);
    }
    const entry = entryOf(c);
    const summary = badgeSummary(entry);
    if (!summary) {
      headRec.git.hidden = true;
      return;
    }
    headRec.git.hidden = false;
    setAttr(headRec.git, "data-tone", summary.tone);
    const branch = entry.git?.branch || (entry.git?.detached ? "HEAD" : "");
    const parts = [];
    if (summary.kind === "pr") {
      const icoKind = summary.topState === "merged" ? "merged" : summary.topState === "closed" ? "closed" : "open";
      parts.push(icon(icoKind));
      parts.push(h("span", {}, `PR ${summary.count}`));
      if (branch) parts.push(h("span.fh-git-branch", {}, branch));
    } else {
      parts.push(icon("branch"));
      parts.push(h("span.fh-git-branch", {}, branch || "git"));
      if (summary.dirty) parts.push(h("span", {}, `*${summary.dirty}`));
    }
    const key = `${summary.kind}:${summary.tone}:${summary.count || 0}:${summary.dirty || 0}:${branch}`;
    if (headRec.gitKey !== key) {
      headRec.gitKey = key;
      headRec.git.replaceChildren(...parts);
    }
    bindHoverTrigger(headRec.git, () => entryOf(c));
  }

  // ---- isolated directory drawer enrichment -----------------------------------------------
  function normalizePath(p) {
    return String(p || "").replace(/[\\/]+$/, "");
  }

  function findIsolatedUser(d, ctx) {
    const dirPath = normalizePath(d.path);
    const derived = typeof ChatTerminal !== "undefined" ? (ChatTerminal?.derivedAll?.() || []) : [];
    const term = derived.find(
      (x) =>
        (x.isolation_id && x.isolation_id === d.id) ||
        (dirPath && x.cwd && (normalizePath(x.cwd) === dirPath || normalizePath(x.cwd).startsWith(`${dirPath}/`))),
    );
    if (term) {
      const active = term.status === "running" || term.status === "waiting" || term.live;
      return {
        kind: "terminal",
        group: term.group,
        label: term.display_title || term.label,
        active,
      };
    }
    const desk = d.task?.desk && ctx?.desks?.get?.(d.task.desk);
    if (desk || d.task?.desk) {
      const name = desk ? AN.deskName(desk) : "";
      const text = [name, d.task?.text].filter(Boolean).join(" · ") || d.task.desk;
      return { kind: "desk", key: d.task.desk, label: text, active: d.status === "using" };
    }
    return null;
  }

  function isolatedNodes(d, ctx) {
    const info = isolatedOf(d.id);
    const nodes = [];
    const git = info?.git;
    if (git?.is_git) {
      const chunks = [];
      if (git.changed_files > 0) {
        chunks.push(h("span.gp-dirty", {}, L(`${git.changed_files} 处改动`, `${git.changed_files} changed`)));
        if (git.additions) chunks.push(h("span.gp-add", {}, `+${git.additions}`));
        if (git.deletions) chunks.push(h("span.gp-del", {}, `-${git.deletions}`));
      } else {
        chunks.push(h("span.gp-clean", {}, L("工作区干净", "Clean")));
      }
      if (info.merged_pr) {
        chunks.push(
          h(
            "span.iso-merged-tag",
            {},
            L(`已随 PR #${info.merged_pr.number} 合并`, `Merged via PR #${info.merged_pr.number}`),
          ),
        );
      } else if (git.default_branch && typeof git.ahead_of_default === "number") {
        chunks.push(
          h(
            "span.gp-sync",
            {},
            git.ahead_of_default > 0
              ? L(`领先 ${git.default_branch} ${git.ahead_of_default} 个提交`, `${git.ahead_of_default} commit(s) ahead of ${git.default_branch}`)
              : L(`与 ${git.default_branch} 同步`, `Up to date with ${git.default_branch}`),
          ),
        );
      }
      nodes.push(h("div.iso-git", {}, ...chunks));
    }

    if (store.snap.gh_status === "ready" && info?.pr) {
      const pr = info.pr;
      const tone = prTone(pr);
      const icoKind = pr.state === "merged" ? "merged" : pr.state === "closed" ? "closed" : "open";
      const ci = checksLabel(pr.checks);
      nodes.push(
        h(
          "div.iso-pr",
          {},
          h(
            "a.iso-pr-link",
            { href: pr.url, target: "_blank", rel: "noopener" },
            h(`span.gp-pr-ico.gp-tone-${tone}`, {}, icon(icoKind)),
            h("code.gp-pr-ref", {}, `#${pr.number}`),
            h("span.iso-pr-title", {}, pr.title),
            h(`span.gp-pr-state.gp-tone-${tone}`, {}, prStateLabel(pr.state)),
          ),
          ci
            ? h(
                `a.gp-chip.gp-tone-${ci[1]}`,
                { href: pr.checks?.checks_url || `${pr.url}/checks`, target: "_blank", rel: "noopener" },
                ci[0],
              )
            : null,
        ),
      );
    }

    const user = findIsolatedUser(d, ctx);
    if (user) {
      const btn = h(
        "button.iso-user-btn",
        { type: "button" },
        user.kind === "terminal"
          ? L(`派生终端：${user.label}`, `Derived terminal: ${user.label}`)
          : L(`会话：${user.label}`, `Session: ${user.label}`),
      );
      btn.addEventListener("click", () => {
        if (user.kind === "terminal" && typeof go === "function") go(`/chat/${user.group}`);
        else if (user.kind === "desk" && typeof openScope === "function") openScope({ kind: "desk", key: user.key }, btn);
      });
      nodes.push(h("div.iso-user", {}, btn));
    }
    return { nodes, hasActiveTerminal: Boolean(user?.kind === "terminal" && user.active) };
  }

  // ---- refresh triggers (event-driven only, no background polling) ------------------------
  function applySnapshot(next) {
    if (!next || typeof next !== "object") return;
    store.snap = next;
    store.loaded = true;
    store.version += 1;
    if (typeof renderList === "function") renderList();
    if (typeof state !== "undefined" && state?.chat) {
      if (state.head && typeof decorateHead === "function") decorateHead(state.head, state.chat);
      if (state.drawer?.kind === "isolated" && state.ctx && typeof renderIsolatedDrawer === "function") {
        renderIsolatedDrawer(state.chat, state.ctx);
      }
    }
  }

  async function runRefresh(reason) {
    if (store.inflight) {
      store.queuedReason = reason;
      return;
    }
    store.inflight = true;
    try {
      const next = await AN.post("/app/git-pr/refresh", { reason, selected: store.selected });
      applySnapshot(next);
    } catch {
      // Fallback to cached GET snapshot if POST is unavailable (e.g. partial mock).
      try {
        const fallback = await AN.load("/app/git-pr/state");
        applySnapshot(fallback);
      } catch { /* ignore transient failures */ }
    } finally {
      store.inflight = false;
      if (store.queuedReason) {
        const queued = store.queuedReason;
        store.queuedReason = null;
        scheduleRefresh(queued, store.selected, 200);
      }
    }
  }

  function scheduleRefresh(reason, selected, delayMs = 150) {
    if (selected !== undefined) store.selected = selected;
    clearTimeout(store.timer);
    store.timer = setTimeout(() => {
      void runRefresh(reason);
    }, delayMs);
  }

  function observe(groups, terminals, selected) {
    store.selected = selected || null;
    let turnEnded = false;

    const nextWs = new Map();
    for (const g of groups || []) {
      const busy = g.status === "running" || g.status === "review";
      if (store.prevWsBusy.get(g.group) === true && !busy) turnEnded = true;
      nextWs.set(g.group, busy);
    }
    store.prevWsBusy = nextWs;

    const nextTerm = new Map();
    for (const t of terminals || []) {
      const busy = Boolean(t.derived ? t.derived_busy && t.exit == null : t.active && t.exit == null);
      if (store.prevTermBusy.get(t.id) === true && !busy) turnEnded = true;
      nextTerm.set(t.id, busy);
    }
    store.prevTermBusy = nextTerm;

    if (!store.loaded) {
      scheduleRefresh("init", selected, 80);
    } else if (turnEnded) {
      scheduleRefresh("turn_end", selected, 250);
    }
  }

  function onFocus() {
    const now = Date.now();
    if (now - store.lastFocusAt < FOCUS_COOLDOWN_MS) return;
    store.lastFocusAt = now;
    scheduleRefresh("focus", store.selected, 120);
  }

  if (typeof window !== "undefined" && window.addEventListener) {
    window.addEventListener("focus", onFocus);
    window.addEventListener("agentnave:focus", onFocus);
  }

  return {
    observe,
    refresh: (reason = "manual") => scheduleRefresh(reason, store.selected, 0),
    decorateItem,
    decorateHead,
    isolatedNodes,
    isolatedOf,
    isCliSession,
    prPriority,
    prTone,
    badgeSummary,
    version: () => store.version,
    _applyForTest: applySnapshot,
  };
})();
