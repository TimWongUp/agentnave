"use strict";
// Git workspace & GitHub PR status for CLI sessions (ADR-0115; read-only since ADR-0126).
// No background polling: refreshes on initial load, window focus, and turn end.
const ChatGit = (() => {
  const { h, L, setAttr, relTime } = AN;
  const SVG_NS = "http://www.w3.org/2000/svg";
  const OPEN_DELAY_MS = 400;
  const CLOSE_DELAY_MS = 360;
  const FOCUS_COOLDOWN_MS = 2000;
  const HOVER_COOLDOWN_MS = 1000;

  const store = {
    snap: { gh_status: "unknown", terminals: {}, workspaces: {}, isolated: {}, updated_at: 0 },
    version: 0,
    loaded: false,
    inflight: false,
    queuedReason: null,
    queuedSelected: undefined,
    pendingReason: null,
    pendingSelected: undefined,
    pendingKeepSelected: false,
    timer: 0,
    lastFocusAt: 0,
    lastHoverAt: 0,
    lastHoverAnchor: null,
    prevWsBusy: new Map(),
    prevTermBusy: new Map(),
    selected: null,
  };

  function isEditableTarget(target) {
    if (!target) return false;
    if (target.isContentEditable) return true;
    const tag = String(target.tagName || "").toUpperCase();
    if (tag === "TEXTAREA" || tag === "SELECT") return true;
    if (tag === "INPUT") {
      const type = String(target.type || "text").toLowerCase();
      return !["checkbox", "radio", "button", "submit", "reset", "range", "color", "file", "image", "hidden"].includes(type);
    }
    return false;
  }

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
      add("circle", { cx: "4.5", cy: "3.5", r: "1.6" });
      add("circle", { cx: "4.5", cy: "12.5", r: "1.6" });
      add("circle", { cx: "11.5", cy: "12.5", r: "1.6" });
      add("path", { d: "M4.5 5.1v5.8M9 3.5h1.2a1.3 1.3 0 0 1 1.3 1.3v6.1" });
    }
    return svg;
  }

  // ---- external link opening (strictly https://github.com/ via Rust browse_address) -------
  function isGithubUrl(url) {
    return typeof url === "string" && url.startsWith("https://github.com/") && !/[\s\x00-\x1f]/.test(url);
  }

  async function openUrl(url) {
    if (!isGithubUrl(url)) return false;
    try {
      await AN.post("/app/git-pr/open-url", { url });
      return true;
    } catch (error) {
      if (typeof Actions !== "undefined" && Actions.say) {
        Actions.say(error?.message || L("无法打开链接", "Failed to open link"), true);
      }
      return false;
    }
  }

  function githubAnchor(tag, url, ...children) {
    if (!isGithubUrl(url)) {
      const spanTag = tag.replace(/^a(?=\.|$)/, "span");
      return h(spanTag, {}, ...children);
    }
    const link = h(tag, { href: url }, ...children);
    link.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      void openUrl(url);
    });
    return link;
  }

  // ---- eligibility, targets & status priority ---------------------------------------------
  function isCliSession(g) {
    if (!g) return false;
    if (g.terminal != null || g.derived === true) return true;
    if (typeof ChatTerminal !== "undefined" && ChatTerminal?.hasTerminal?.(g.group)) return true;
    return g.cli_session === true;
  }

  function targetOf(g) {
    if (!g || !isCliSession(g)) return null;
    if (g.terminal != null) return { kind: "terminal", id: g.terminal };
    if (typeof g.group === "string" && g.group.startsWith("term-")) {
      const tid = Number(g.group.slice(5));
      if (Number.isFinite(tid)) return { kind: "terminal", id: tid };
    }
    if (g.derived === true) return null;
    return g.group ? { kind: "workspace", id: g.group } : null;
  }

  function isWorking(g, entry) {
    if (entry?.working) return true;
    if (!g) return false;
    if (g.derived === true && g.status === "running") return true;
    if (g.terminal != null && store.prevTermBusy.get(g.terminal) === true) return true;
    return false;
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
    if (typeof g.group === "string" && g.group.startsWith("term-")) {
      return store.snap.terminals?.[g.group.slice(5)] || null;
    }
    if (g.derived === true) return null;
    return store.snap.workspaces?.[g.group] || null;
  }

  function isolatedOf(id) {
    return (id && store.snap.isolated?.[id]) || null;
  }

  // ---- floating hover/click card ----------------------------------------------------------
  const cardBody = h("div.git-pr-body");
  const card = h("div.git-pr-card", { role: "dialog", hidden: true }, cardBody);
  let cardAnchor = null;
  let cardPinned = false;
  let closeTimer = 0;
  let openTimer = 0;
  let pendingAnchor = null;

  function cancelPendingOpen(node = null) {
    if (node && pendingAnchor !== node) return;
    clearTimeout(openTimer);
    openTimer = 0;
    pendingAnchor = null;
  }

  function isTriggerVisible(node, wasConnected) {
    if (!node || node.hidden) return false;
    if (wasConnected && !node.isConnected) return false;
    if (typeof document !== "undefined" && document.body?.isConnected && !node.isConnected) return false;
    if (node.closest?.("[hidden]")) return false;
    if (typeof node.getClientRects === "function" && node.getClientRects().length === 0) return false;
    return true;
  }

  function ensureCardMounted() {
    if (typeof document === "undefined" || !document.body || card.isConnected) return;
    document.body.append(card);
    card.addEventListener("pointerenter", () => {
      clearTimeout(closeTimer);
      closeTimer = 0;
    });
    card.addEventListener("pointerleave", () => { if (!cardPinned) hideCardSoon(); });
    document.addEventListener("pointerdown", (e) => {
      if (!card.hidden && !card.contains(e.target) && !cardAnchor?.contains?.(e.target)) hideCard();
      else if (pendingAnchor && !pendingAnchor.contains?.(e.target) && !card.contains(e.target)) cancelPendingOpen();
    }, true);
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && (!card.hidden || openTimer)) hideCard();
    });
  }

  function closeCard() {
    clearTimeout(closeTimer);
    closeTimer = 0;
    card.hidden = true;
    cardAnchor = null;
    cardPinned = false;
  }

  function hideCard() {
    cancelPendingOpen();
    closeCard();
  }

  function hideCardSoon() {
    if (cardPinned) return;
    clearTimeout(closeTimer);
    closeTimer = setTimeout(closeCard, CLOSE_DELAY_MS);
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

  function buildGitSection(git, working) {
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
        working ? h("span.gp-tag.gp-tag-busy", {}, L("工作中", "Working")) : null,
      ),
      h(
        "div.gp-git-stats",
        {},
        ...dirtyNodes,
        ...syncParts.map((text) => h("span.gp-sync", {}, text)),
      ),
    );
  }

  function resolveChecksUrl(pr) {
    const raw = pr.checks?.checks_url;
    if (isGithubUrl(raw)) return raw;
    if (isGithubUrl(pr.url)) return `${pr.url}/checks`;
    return null;
  }

  function buildPrRow(pr) {
    const tone = prTone(pr);
    const prIconKind = pr.state === "merged" ? "merged" : pr.state === "closed" ? "closed" : "open";
    const chips = [];
    const ci = checksLabel(pr.checks);
    if (ci) {
      chips.push(githubAnchor(`a.gp-chip.gp-tone-${ci[1]}`, resolveChecksUrl(pr), ci[0]));
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
      githubAnchor(
        "a.gp-pr-head",
        pr.url,
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
      chips.length ? h("div.gp-pr-foot", {}, h("div.gp-pr-chips", {}, ...chips)) : null,
    );
  }

  function refreshOnHover(anchor, payload) {
    const now = Date.now();
    if (store.lastHoverAnchor === anchor && now - store.lastHoverAt < HOVER_COOLDOWN_MS) return;
    store.lastHoverAt = now;
    store.lastHoverAnchor = anchor;
    const selected = payload?.target?.kind === "workspace" ? payload.target.id : store.selected;
    scheduleRefresh("hover", selected, 0, true);
  }

  function showCard(anchor, payload, pin = false) {
    ensureCardMounted();
    if (!anchor || !payload?.entry) return;
    if (pin || pendingAnchor === anchor) cancelPendingOpen();
    clearTimeout(closeTimer);
    closeTimer = 0;
    const { entry, working } = payload;
    const ghReady = store.snap.gh_status === "ready";
    const prs = ghReady && Array.isArray(entry.prs) ? entry.prs : [];
    const gitNode = buildGitSection(entry.git, Boolean(working));
    const prNodes = prs.map((pr) => buildPrRow(pr));
    if (!gitNode && !prNodes.length) return;

    const opening = card.hidden || cardAnchor !== anchor;
    cardAnchor = anchor;
    if (pin) cardPinned = true;
    const bodyNodes = [];
    if (gitNode) bodyNodes.push(gitNode);
    if (prNodes.length) bodyNodes.push(h("div.gp-prs", {}, ...prNodes));
    cardBody.replaceChildren(...bodyNodes);
    card.hidden = false;
    positionCard(anchor);
    if (opening) refreshOnHover(anchor, payload);
  }

  function positionCard(anchor) {
    if (typeof window === "undefined" || !anchor.getBoundingClientRect) return;
    const rect = anchor.getBoundingClientRect();
    const cw = card.offsetWidth || 360;
    const ch = card.offsetHeight || 200;
    const margin = 8;
    const inSide = Boolean(anchor.closest?.(".side"));
    let left = inSide ? rect.right + 4 : rect.right - cw;
    let top = inSide ? rect.top - 4 : rect.bottom + 4;
    let side = inSide ? "right" : "bottom";
    if (inSide && left + cw > window.innerWidth - margin) {
      left = Math.max(margin, rect.left - cw - 4);
      side = "left";
    }
    if (!inSide && top + ch > window.innerHeight - margin && rect.top - ch - 4 >= margin) {
      top = rect.top - ch - 4;
      side = "top";
    }
    if (left + cw > window.innerWidth - margin) left = Math.max(margin, window.innerWidth - cw - margin);
    if (left < margin) left = margin;
    if (top + ch > window.innerHeight - margin) top = Math.max(margin, window.innerHeight - ch - margin);
    if (top < margin) top = margin;
    card.setAttribute("data-side", side);
    card.style.left = `${Math.round(left)}px`;
    card.style.top = `${Math.round(top)}px`;
  }

  function scheduleOpen(node) {
    if (cardPinned && !card.hidden && cardAnchor !== node) return;
    if (!card.hidden && cardAnchor === node) {
      clearTimeout(closeTimer);
      closeTimer = 0;
      cancelPendingOpen();
      return;
    }
    const payload = node._gpGetPayload?.();
    if (!payload?.entry) return;
    cancelPendingOpen();
    const wasConnected = Boolean(node.isConnected);
    pendingAnchor = node;
    openTimer = setTimeout(() => {
      if (pendingAnchor !== node) return;
      openTimer = 0;
      pendingAnchor = null;
      if (cardPinned && !card.hidden && cardAnchor !== node) return;
      if (!isTriggerVisible(node, wasConnected)) return;
      const latest = node._gpGetPayload?.();
      if (latest?.entry) showCard(node, latest, false);
    }, OPEN_DELAY_MS);
  }

  function bindHoverTrigger(node, getPayload) {
    if (node._gpBound) {
      node._gpGetPayload = getPayload;
      return;
    }
    node._gpBound = true;
    node._gpGetPayload = getPayload;
    node.addEventListener("pointerenter", (e) => {
      if (e?.pointerType === "touch") return;
      scheduleOpen(node);
    });
    node.addEventListener("pointerleave", () => {
      cancelPendingOpen(node);
      hideCardSoon();
    });
    node.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      cancelPendingOpen();
      const payload = node._gpGetPayload?.();
      if (!payload?.entry) return;
      if (!card.hidden && cardAnchor === node && cardPinned) hideCard();
      else showCard(node, payload, true);
    });
    node.addEventListener("keydown", (e) => {
      if (isEditableTarget(e.target)) return;
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      e.stopPropagation();
      cancelPendingOpen();
      const payload = node._gpGetPayload?.();
      if (payload?.entry) showCard(node, payload, true);
    });
  }

  function payloadFor(g) {
    const entry = entryOf(g);
    if (!entry) return null;
    return { entry, target: targetOf(g), working: isWorking(g, entry) };
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
      cancelPendingOpen(rec.prBadge);
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
        rec.prBadge.replaceChildren(...[icon("branch"), suffix ? h("span.gi-pr-n", {}, suffix) : null].filter(Boolean));
      }
    }
    const aria = summary.kind === "pr"
      ? L(`${summary.count} 个关联 PR`, `${summary.count} linked PR(s)`)
      : L(`Git 分支 ${summary.git.branch || "HEAD"}`, `Git branch ${summary.git.branch || "HEAD"}`);
    setAttr(rec.prBadge, "aria-label", aria);
    bindHoverTrigger(rec.prBadge, () => payloadFor(g));
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
      cancelPendingOpen(headRec.git);
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
    bindHoverTrigger(headRec.git, () => payloadFor(c));
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
        (x.terminal != null || String(x.group || "").startsWith("term-")) &&
        ((x.isolation_id && x.isolation_id === d.id) ||
          (dirPath && x.cwd && (normalizePath(x.cwd) === dirPath || normalizePath(x.cwd).startsWith(`${dirPath}/`)))),
    );
    if (term) {
      const active = term.status === "running" || term.status === "waiting" || term.live;
      return {
        kind: "terminal",
        group: term.group,
        label: term.display_title || term.label,
        active,
        open: term.exit == null,
      };
    }
    const desk = d.task?.desk && ctx?.desks?.get?.(d.task.desk);
    if (desk || d.task?.desk) {
      const name = desk ? AN.deskName(desk) : "";
      const text = [name, d.task?.text].filter(Boolean).join(" · ") || d.task.desk;
      return { kind: "desk", key: d.task.desk, label: text, active: d.status === "using", open: false };
    }
    return null;
  }

  function isolatedNodes(d, ctx) {
    const info = isolatedOf(d.id);
    const nodes = [];
    const git = info?.git;
    const ghReady = store.snap.gh_status === "ready";
    const user = findIsolatedUser(d, ctx);
    const hasActiveTerminal = Boolean(user?.kind === "terminal" && (user.active || user.open));

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

    if (ghReady && info?.pr) {
      const pr = info.pr;
      const tone = prTone(pr);
      const icoKind = pr.state === "merged" ? "merged" : pr.state === "closed" ? "closed" : "open";
      const ci = checksLabel(pr.checks);
      nodes.push(
        h(
          "div.iso-pr",
          {},
          githubAnchor(
            "a.iso-pr-link",
            pr.url,
            h(`span.gp-pr-ico.gp-tone-${tone}`, {}, icon(icoKind)),
            h("code.gp-pr-ref", {}, `#${pr.number}`),
            h("span.iso-pr-title", { title: pr.title || "" }, pr.title || `#${pr.number}`),
            h(`span.gp-pr-state.gp-tone-${tone}`, {}, prStateLabel(pr.state)),
          ),
          ci ? githubAnchor(`a.gp-chip.gp-tone-${ci[1]}`, resolveChecksUrl(pr), ci[0]) : null,
        ),
      );
    }

    if (user) {
      const userLabel = user.kind === "terminal"
        ? L(`派生终端：${user.label || ""}`, `Derived terminal: ${user.label || ""}`)
        : L(`会话：${user.label || ""}`, `Session: ${user.label || ""}`);
      const btn = h(
        "button.iso-user-btn",
        { type: "button", title: userLabel },
        userLabel,
      );
      btn.addEventListener("click", () => {
        if (user.kind === "terminal") {
          if (typeof ChatTerminal !== "undefined" && ChatTerminal?.openDerived?.(user.group)) return;
          if (typeof go === "function") go(`/chat/${user.group}`);
        } else if (user.kind === "desk" && typeof openScope === "function") openScope({ kind: "desk", key: user.key }, btn);
      });
      nodes.push(h("div.iso-user", {}, btn));
    }
    return { nodes, hasActiveTerminal };
  }

  // ---- refresh triggers (event-driven only, no background polling) ------------------------
  function applySnapshot(next) {
    if (!next || typeof next !== "object" || (!next.terminals && !next.workspaces && !next.isolated)) return;
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
    if (!card.hidden && cardAnchor?.isConnected && cardAnchor._gpGetPayload) {
      const updated = cardAnchor._gpGetPayload();
      if (updated?.entry) showCard(cardAnchor, updated, cardPinned);
    }
  }

  const REASON_PRIORITY = { init: 1, hover: 2, focus: 3, turn_end: 4, manual: 5 };

  function mergeReason(current, incoming) {
    if (!current) return incoming;
    return (REASON_PRIORITY[incoming] || 0) >= (REASON_PRIORITY[current] || 0) ? incoming : current;
  }

  async function runRefresh(reason, selectedOverride) {
    if (store.inflight) {
      const merged = mergeReason(store.queuedReason, reason);
      if (merged !== store.queuedReason) {
        store.queuedReason = merged;
        if (selectedOverride !== undefined) store.queuedSelected = selectedOverride;
      } else if (selectedOverride !== undefined) {
        store.queuedSelected = selectedOverride;
      }
      return;
    }
    store.inflight = true;
    try {
      const selected = selectedOverride !== undefined ? selectedOverride : store.selected;
      const next = await AN.post("/app/git-pr/refresh", { reason, selected });
      applySnapshot(next);
    } catch {
      try {
        const fallback = await AN.load("/app/git-pr/state");
        applySnapshot(fallback);
      } catch { /* ignore transient failures */ }
    } finally {
      store.inflight = false;
      if (store.queuedReason) {
        const queued = store.queuedReason;
        const queuedSelected = store.queuedSelected;
        store.queuedReason = null;
        store.queuedSelected = undefined;
        scheduleRefresh(queued, queuedSelected, queued === "manual" ? 0 : 200, queuedSelected !== undefined);
      }
    }
  }

  function scheduleRefresh(reason, selected, delayMs = 150, keepStoreSelected = false) {
    if (!keepStoreSelected && selected !== undefined) store.selected = selected;
    const merged = mergeReason(store.pendingReason, reason);
    store.pendingReason = merged;
    if (keepStoreSelected) {
      store.pendingSelected = selected;
      store.pendingKeepSelected = true;
    } else if (merged === reason) {
      store.pendingSelected = undefined;
      store.pendingKeepSelected = false;
    }
    clearTimeout(store.timer);
    store.timer = setTimeout(() => {
      const runReason = store.pendingReason || reason;
      const runSelected = store.pendingKeepSelected ? store.pendingSelected : undefined;
      store.pendingReason = null;
      store.pendingSelected = undefined;
      store.pendingKeepSelected = false;
      void runRefresh(runReason, runSelected);
    }, merged === "manual" ? 0 : delayMs);
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
    isGithubUrl,
    openUrl,
    prPriority,
    prTone,
    badgeSummary,
    version: () => store.version,
    _applyForTest: applySnapshot,
  };
})();
