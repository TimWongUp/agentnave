"use strict";
// Managing history from the office: right-click a workspace or a meeting room to rename or delete
// it, or a workspace to hand it to another boss; ⌘/Ctrl-click and Shift-click pick several workspaces to delete together; "清理" deletes the
// workspaces that have been quiet for a chosen number of days. Deleting takes the same locks as the
// servers, so conversations in use, running or awaiting review are kept and reported.
// Loaded before chat.js, which calls it; it reaches chat.js's globals (state, go, refresh,
// renderList, openScope) only from event handlers, once both scripts have run.
const Actions = {};
{
  const { h, $, L, load, chatName } = AN;
  const picked = new Set(); // workspace ids picked for a batch action
  let anchor = null; // the last workspace clicked, where a Shift range starts
  const DAYS = [7, 30, 90, 180];

  const menu = h("div.ctx", { role: "menu", hidden: true });
  const dialog = h("dialog.act");
  const toast = h("div.toast", { role: "status", hidden: true });
  document.body.append(menu, dialog, toast);

  // ---- context menu -----------------------------------------------------------------------
  function closeMenu() {
    menu.hidden = true;
    menu.replaceChildren();
  }
  function openMenu(e, items) {
    e.preventDefault();
    e.stopPropagation();
    if (typeof ChatSidebar !== "undefined") {
      ChatSidebar?.closePopover?.();
      ChatSidebar?.closeNewChat?.();
    }
    menu.replaceChildren(
      ...items.map((item) => {
        const b = h(`button.ctx-item${item.danger ? ".danger" : ""}`, { type: "button", role: "menuitem" }, item.label);
        b.addEventListener("click", () => {
          closeMenu();
          item.run();
        });
        return b;
      }),
    );
    menu.hidden = false;
    // Keep the menu inside the window.
    const { width, height } = menu.getBoundingClientRect();
    menu.style.left = `${Math.min(e.clientX, innerWidth - width - 6)}px`;
    menu.style.top = `${Math.min(e.clientY, innerHeight - height - 6)}px`;
    menu.querySelector("button")?.focus({ preventScroll: true });
  }
  document.addEventListener("pointerdown", (e) => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); }, true);
  document.addEventListener("scroll", closeMenu, true);
  window.addEventListener("blur", closeMenu);
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!menu.hidden) closeMenu();
    else if (picked.size && !dialog.open) clearPicks();
  });
  menu.addEventListener("keydown", (e) => {
    const buttons = [...menu.querySelectorAll("button")];
    const i = buttons.indexOf(document.activeElement);
    const step = { ArrowDown: 1, ArrowUp: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    buttons[(i + step + buttons.length) % buttons.length].focus();
  });

  // ---- picking several workspaces ----------------------------------------------------------
  function clearPicks() {
    picked.clear();
    renderList();
  }
  // A click with ⌘/Ctrl toggles one workspace, with Shift picks the range from the last click;
  // returns false for a plain click, which opens the workspace and drops the picks.
  function pick(id, e) {
    if (!(e.metaKey || e.ctrlKey || e.shiftKey)) {
      anchor = id;
      if (picked.size) clearPicks();
      return false;
    }
    // The open workspace counts as picked when a selection starts, as in Finder.
    if (!picked.size && state.selected && state.selected !== id) picked.add(state.selected);
    if (e.metaKey || e.ctrlKey) {
      if (picked.has(id)) picked.delete(id); else picked.add(id);
      anchor = id;
    } else {
      const from = state.order.indexOf(anchor ?? state.selected);
      const to = state.order.indexOf(id);
      if (from < 0) picked.add(id);
      else for (const g of state.order.slice(Math.min(from, to), Math.max(from, to) + 1)) picked.add(g);
    }
    renderList();
    return true;
  }

  // ---- reading what can be deleted --------------------------------------------------------
  async function manageState() {
    const data = await load("/manage/state");
    if (data.error) throw new Error(data.error);
    return { items: data.items || [], workspaces: data.workspaces || [] };
  }
  async function conversations() {
    return (await manageState()).items;
  }
  const groupName = (id) => {
    const g = state.groupsData.find((x) => x.group === id);
    return g ? chatName(g) : id;
  };

  // ---- dialogs ----------------------------------------------------------------------------
  // One modal at a time: a title, a body, and buttons; resolves with the button's value.
  function ask({ title, body = [], buttons, input }) {
    return new Promise((resolve) => {
      const field = input && h("input.act-input", { type: "text", value: input.value, placeholder: input.placeholder || null, "aria-label": input.label, spellcheck: "false", autocorrect: "off", autocapitalize: "off", autocomplete: "off" });
      const error = h("p.act-error", { role: "alert", hidden: true });
      let focusButton = null;
      const row = h("div.act-buttons", {}, buttons.map((b) => {
        const button = h(`button${b.danger ? ".danger" : ""}${b.primary ? ".primary" : ""}`, { type: "button" }, b.label);
        if (b.focus) focusButton = button;
        button.addEventListener("click", async () => {
          if (!b.value) return finish(null);
          if (!b.check) return finish(b.value);
          const message = await b.check(field?.value ?? "");
          if (message) { error.textContent = message; error.hidden = false; return; }
          finish(b.value);
        });
        return button;
      }));
      dialog.replaceChildren(h("h2.act-title", {}, title), ...body, field || "", error, row);
      const finish = (value) => {
        dialog.close();
        resolve(value === null ? null : { value, text: field?.value });
      };
      dialog.oncancel = (e) => { e.preventDefault(); finish(null); };
      if (field) field.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.isComposing && e.keyCode !== 229) row.querySelector(".primary")?.click();
      });
      dialog.showModal();
      (field || focusButton || row.querySelector(".primary") || row.querySelector("button")).focus();
      if (field) field.select();
    });
  }
  let toastTimer = null;
  function say(text, bad = false) {
    toast.textContent = text;
    toast.classList.toggle("bad", bad);
    toast.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { toast.hidden = true; }, bad ? 8000 : 4000);
  }

  // ---- rename -----------------------------------------------------------------------------
  async function rename(id, current, what) {
    await ask({
      title: L(`给${what}改名`, `Rename ${what}`),
      body: [h("p.act-note", {}, L("最多 24 个汉字或 48 个英文字符。", "Up to 24 Chinese or 48 English characters."))],
      input: { value: current, label: L("新名字", "New name") },
      buttons: [
        { label: L("取消", "Cancel") },
        {
          label: L("保存", "Save"),
          primary: true,
          value: "save",
          check: async (text) => {
            const next = text.trim();
            if (!next) return L("名字不能为空", "Name cannot be empty");
            if (next === current) return "";
            try {
              const data = await AN.post("/app/conversations/rename", { id, title: next });
              if (data.status !== "renamed") return data.message || L("未能改名", "Failed to rename");
            } catch (error) {
              return L(`未能改名：${error.message}`, `Failed to rename: ${error.message}`);
            }
            say(L(`已改名为“${next}”`, `Renamed to "${next}"`));
            refresh();
            return "";
          },
        },
      ],
    });
  }

  // ---- delete -----------------------------------------------------------------------------
  // Deletes the given conversations (and expired claimed boss-only workspaces) after confirming.
  async function remove(names, items, what, cutoff = null, emptyIds = []) {
    const free = items.filter((i) => i.deletable);
    const kept = items.filter((i) => !i.deletable);
    if (!free.length && !emptyIds.length) {
      await ask({
        title: L(`${what}现在不能删除`, `Cannot delete ${what} right now`),
        body: [h("p.act-note", {}, kept[0]?.reason || L("里面没有可删除的会话。", "No deletable conversations found."))],
        buttons: [{ label: L("知道了", "OK"), primary: true }],
      });
      return;
    }
    const note = free.length && emptyIds.length
      ? L(`将删除 ${free.length} 个会话与 ${emptyIds.length} 个已过期空 agent 的记录，删除后不能恢复；各 CLI 自己的会话文件不受影响。`, `Deletes history for ${free.length} conversation(s) and ${emptyIds.length} expired empty agent(s). This cannot be undone; each CLI's own session files are not affected.`)
      : free.length
        ? L(`将删除 ${free.length} 个会话的历史记录，删除后不能恢复；各 CLI 自己的会话文件不受影响。`, `Deletes history for ${free.length} conversation(s). This cannot be undone; each CLI's own session files are not affected.`)
        : L("交接文档与 agent 记录会一起删除，删除后不能恢复；各 CLI 自己的会话文件不受影响。", "Handover notes and agent records will also be deleted and cannot be recovered; each CLI's own session files are not affected.");
    const answer = await ask({
      title: L(`删除${what}？`, `Delete ${what}?`),
      body: [
        h("ul.act-list", {}, names.map((n) => h("li", {}, n))),
        h("p.act-note", {}, note),
        kept.length > 0 && h("p.act-note.warn", {}, L(`${kept.length} 个会话正在运行、待审核或被宿主占用，这次会保留。`, `${kept.length} conversation(s) are running, awaiting review, or in use by a host and will be kept.`)),
      ].filter(Boolean),
      buttons: [{ label: L("取消", "Cancel") }, { label: L("删除", "Delete"), danger: true, primary: true, value: "delete" }],
    });
    if (!answer) return;
    try {
      const results = [];
      if (free.length) {
        if (cutoff === null) {
          const outcome = await AN.post("/app/conversations/delete", { ids: free.map((i) => i.id) });
          results.push(...(outcome.results || []));
        } else {
          const groups = new Map();
          for (const item of free) {
            const group = item.group;
            if (!groups.has(group)) groups.set(group, []);
            groups.get(group).push(item.id);
          }
          for (const ids of groups.values()) {
            const outcome = await AN.post("/app/conversations/delete", { ids, updated_before: cutoff });
            results.push(...(outcome.results || []));
          }
        }
      }
      if (emptyIds.length) {
        const forgot = await Promise.all(
          emptyIds.map((group) => AN.post("/app/workspaces/forget", cutoff === null ? { group } : { group, updated_before: cutoff })),
        );
        results.push(...forgot);
      }
      const failed = results.filter((r) => !["deleted", "missing"].includes(r.status));
      if (failed.length) say(L(`${failed.length} 项未删除：${failed[0].message}`, `${failed.length} item(s) not deleted: ${failed[0].message}`), true);
      else if (free.length && emptyIds.length) say(L(`已删除 ${free.length} 个会话与 ${emptyIds.length} 个 agent`, `Deleted ${free.length} conversation(s) and ${emptyIds.length} agent(s)`));
      else if (emptyIds.length) say(L(`已删除 ${emptyIds.length} 个 agent`, `Deleted ${emptyIds.length} agent(s)`));
      else say(L(`已删除 ${free.length} 个会话`, `Deleted ${free.length} conversation(s)`));
    } catch (error) {
      say(L(`删除没有完成：${error.message}。刷新后核对再试。`, `Deletion incomplete: ${error.message}. Refresh and check before trying again.`), true);
    }
    for (const id of picked) if (!state.groupsData.some((g) => g.group === id)) picked.delete(id);
    await refresh();
  }
  // An agent with no conversation exists only as its handover, with nothing else to delete.
  async function forgetGroups(ids) {
    const answer = await ask({
      title: ids.length > 1 ? L(`删除 ${ids.length} 个只有 boss 的 agent？`, `Delete ${ids.length} boss-only agents?`) : L(`删除 agent“${groupName(ids[0])}”？`, `Delete agent "${groupName(ids[0])}"?`),
      body: [
        ids.length > 1 && h("ul.act-list", {}, ids.map((id) => h("li", {}, groupName(id)))),
        h("p.act-note", {}, L("交接文档会一起删除，删除后不能恢复；各 CLI 自己的会话文件不受影响。", "Handover notes will also be deleted and cannot be recovered; each CLI's own session files are not affected.")),
      ].filter(Boolean),
      buttons: [{ label: L("取消", "Cancel") }, { label: L("删除", "Delete"), danger: true, primary: true, value: "delete" }],
    });
    if (!answer) return;
    try {
      const results = await Promise.all(ids.map((group) => AN.post("/app/workspaces/forget", { group })));
      const failed = results.filter((r) => r.status !== "deleted");
      if (failed.length) say(L(`${failed.length} 个 agent 未删除：${failed[0].message}`, `${failed.length} agent(s) not deleted: ${failed[0].message}`), true);
      else say(L(`已删除 ${ids.length} 个 agent`, `Deleted ${ids.length} agent(s)`));
    } catch (error) {
      say(L(`删除没有完成：${error.message}。刷新后核对再试。`, `Deletion incomplete: ${error.message}. Refresh and check before trying again.`), true);
    }
    await refresh();
  }
  async function removeGroups(ids) {
    const empty = ids.filter((id) => !state.groupsData.find((g) => g.group === id)?.conversations);
    const used = ids.filter((id) => !empty.includes(id));
    if (empty.length) await forgetGroups(empty);
    if (used.length) {
      let all;
      try { all = await conversations(); } catch (error) { say(L(`无法读取会话：${error.message}`, `Failed to load conversations: ${error.message}`), true); return; }
      const set = new Set(used);
      const what = used.length > 1 ? L(` ${used.length} 个 agent`, `${used.length} agents`) : L(`agent“${groupName(used[0])}”`, `agent "${groupName(used[0])}"`);
      await remove(used.map(groupName), all.filter((i) => set.has(i.group)), what);
    }
    for (const id of ids) picked.delete(id);
    renderList();
    if (ids.includes(state.selected) && !state.groupsData.some((g) => g.group === state.selected)) go("/chat/");
  }
  async function removeRoom(id, title) {
    let all;
    try { all = await conversations(); } catch (error) { say(L(`无法读取会话：${error.message}`, `Failed to load conversations: ${error.message}`), true); return; }
    await remove([title], all.filter((i) => i.id === id), L(`会议“${title}”`, `meeting "${title}"`));
  }

  // ---- delete isolated directory ----------------------------------------------------------
  let deletingIsolated = false;
  function isolatedConfirmBody(info) {
    const items = [];
    const target = info.branch ? `${info.branch} · ${info.path}` : info.path;
    items.push(h("p.act-note", {}, target));
    if (info.collapsed_rollback) {
      items.push(h("p.act-note", {}, L("这个已收起目录上次重建未完成。确认后将清理残留目录、恢复为已收起状态并保留受管分支；若目录内已有未提交改动、隐藏索引标记或嵌套仓库（含无法检查的目录），则会保留目录并说明原因。", "Rebuilding this collapsed directory did not finish. Confirming cleans up the leftover directory, restores the collapsed state, and keeps the managed branch; if uncommitted changes, hidden index flags, or nested repositories exist, the directory will be kept and the reason shown.")));
    } else if (info.missing_dir) {
      items.push(h("p.act-note", {}, L("目录已不在磁盘上。确认后将清理残留的受管分支和登记记录。", "The directory is no longer on disk. Confirming cleans up the remaining managed branch and registry entry.")));
    } else if (info.rollback_only) {
      items.push(h("p.act-note", {}, L("这个目录上次创建或回滚未完成。确认后将尝试删除残留目录、受管分支和登记记录，目录里被 git 忽略的文件也会一并丢弃；若目录内已有未提交改动、隐藏索引标记或嵌套仓库（含无法检查的目录），则会保留目录并说明原因。", "Creation or rollback of this directory did not finish. Confirming attempts to remove the leftover directory, managed branch, and registry entry, discarding any Git-ignored files; if uncommitted changes, hidden index flags, or nested repositories (including unreadable directories) exist, the directory will be kept and the reason shown.")));
    } else {
      items.push(h("p.act-note", {}, L("将永久删除这个隔离目录、受管分支和登记记录；目录里被 git 忽略的文件（如依赖或构建缓存）也会一并丢弃，无法恢复。", "Permanently deletes this isolated directory, its managed branch, and its registry entry. Any Git-ignored files inside (such as dependencies or build caches) will also be discarded and cannot be recovered.")));
    }
    if (info.attention) {
      items.push(h("p.act-note.warn", {}, L(`上次清理未完成：${info.attention}`, `Last cleanup failed: ${info.attention}`)));
    }
    if (info.orphan) {
      items.push(h("p.act-note.warn", {}, L("该目录已不是有效的 git worktree，确认后将直接删除目录内的剩余文件（不做 git 检查）。", "This directory is no longer a valid Git worktree. Confirming deletes all remaining files inside without Git inspection.")));
    } else if (info.uncommitted) {
      const files = info.uncommitted_files || [];
      const total = info.uncommitted_total || files.length;
      items.push(h("p.act-note.warn", {}, total
        ? L(`有 ${total} 处未提交的源码改动或未跟踪文件：`, `Contains ${total} uncommitted source change(s) or untracked file(s):`)
        : L("存在未提交的源码改动或未跟踪文件。", "Contains uncommitted source changes or untracked files.")));
      if (files.length) {
        const list = files.map((f) => h("li", {}, f));
        if (total > files.length) list.push(h("li", {}, L(`……其余 ${total - files.length} 项`, `…and ${total - files.length} more`)));
        items.push(h("ul.act-list", {}, list));
      }
    }
    if (info.merged_pr?.number) {
      const prLabel = `PR #${info.merged_pr.number}`;
      const prUrl = typeof info.merged_pr.url === "string" && info.merged_pr.url.startsWith("https://github.com/")
        ? info.merged_pr.url
        : null;
      let prNode = prLabel;
      if (prUrl) {
        prNode = h("a.iso-merged-link", { href: prUrl }, prLabel);
        prNode.addEventListener("click", (e) => {
          e.preventDefault();
          e.stopPropagation();
          if (typeof ChatGit !== "undefined" && ChatGit.openUrl) ChatGit.openUrl(prUrl);
          else AN.post("/app/git-pr/open-url", { url: prUrl }).catch(() => {});
        });
      }
      items.push(
        h(
          "p.act-note.iso-merged",
          {},
          L("受管分支上的提交已随 ", "Managed branch commits are already merged via "),
          prNode,
          L(" 合并。", "."),
        ),
      );
    } else if (info.unique_commits) {
      const commits = info.unmerged_commits || [];
      const total = info.unmerged_total || commits.length;
      items.push(h("p.act-note.warn", {}, total
        ? L(`受管分支上有 ${total} 个尚未合入其他分支的提交：`, `Managed branch has ${total} commit(s) not merged into any other branch:`)
        : L("受管分支当前提交尚未合入其他本地或远程分支。", "The managed branch commit is not merged into any other local or remote branch.")));
      if (commits.length) {
        const list = commits.map((c) => h("li", {}, c));
        if (total > commits.length) list.push(h("li", {}, L(`……其余 ${total - commits.length} 个`, `…and ${total - commits.length} more`)));
        items.push(h("ul.act-list", {}, list));
      }
    }
    if (info.nested_repos?.length) {
      items.push(h("p.act-note.warn", {}, L(`目录内包含未检查内容的嵌套仓库：${info.nested_repos.join("、")}`, `Contains nested repositories whose contents were not checked: ${info.nested_repos.join(", ")}`)));
    }
    if (info.nested_truncated) {
      items.push(h("p.act-note.warn", {}, L("目录条目过多，未完成全部嵌套仓库检查，可能仍有未检查的嵌套仓库或内容。", "Directory contains too many entries to complete nested repository inspection; unchecked nested repositories or contents may remain.")));
    }
    if (info.reason) {
      items.push(h("p.act-note.warn", {}, L("目录状态在确认期间发生变化，请核对以上最新情况后再确认。", "The directory state changed before deletion. Review the updated details above before confirming.")));
    }
    return items;
  }

  async function deleteIsolated(group, dir, button) {
    if (!group || !dir?.id || dialog.open || deletingIsolated) return;
    deletingIsolated = true;
    if (button) button.disabled = true;
    try {
      let info;
      try {
        info = await AN.post("/app/isolated/inspect", { id: dir.id, workspace: group });
      } catch (error) {
        say(error.message || L("无法检查隔离目录", "Failed to inspect isolated directory"), true);
        return;
      }
      if (info.status !== "confirm") {
        say(info.reason || info.message || L("暂时不能删除这个隔离目录", "Cannot delete this isolated directory right now"), true);
        await refresh();
        return;
      }
      while (true) {
        const answer = await ask({
          title: L("删除隔离目录？", "Delete isolated directory?"),
          body: isolatedConfirmBody(info),
          buttons: [{ label: L("取消", "Cancel") }, { label: L("删除", "Delete"), danger: true, primary: true, value: "delete" }],
        });
        if (!answer) return;
        try {
          const result = await AN.post("/app/isolated/delete", { id: dir.id, workspace: group, confirmation: info.confirmation });
          if (result.status === "confirmation_required") {
            info = result;
            continue;
          }
          if (result.status === "deleted") {
            say(result.reason || L("已删除隔离目录", "Deleted isolated directory"));
          } else if (result.status === "collapsed") {
            say(result.reason || result.message || L("已回滚未完成的重建并保留受管分支", "Rolled back unfinished rebuild and preserved managed branch"));
          } else {
            say(result.reason || result.message || L("未能删除隔离目录", "Failed to delete isolated directory"), true);
          }
        } catch (error) {
          say(error.message || L("删除隔离目录失败", "Failed to delete isolated directory"), true);
        }
        await refresh();
        return;
      }
    } finally {
      deletingIsolated = false;
      if (button) button.disabled = false;
    }
  }

  // ---- hand over --------------------------------------------------------------------------
  // For a boss that can no longer call tools: the workspace waits until a new chat takes it over.
  async function handOver(id) {
    const answer = await ask({
      title: L(`把 agent“${groupName(id)}”交给其他 boss？`, `Hand over agent "${groupName(id)}" to another boss?`),
      body: [
        h("p.act-note", {}, L("boss 还能回话时，直接对它说「转交这个会话」，它会写好交接文档；这里用于 boss 已经用不了的情况。", "When the current boss can still reply, tell it \"Hand over this session\" directly so it writes handover notes; use this when that boss can no longer be used.")),
        h("p.act-note", {}, L("这个 agent 会标为待接手。之后在任意宿主的聊天里请它接手这个 agent：接手会在两个 agent 之间建立关联，新 boss 可以在自己的 agent 里引用这里的已完成结果，原 agent 和原来的 boss 仍可继续使用；还在跑的任务会留在原处跑完。", "Marks the agent as awaiting handover. Ask a chat in any host to take over this agent: claiming links the agents so the new boss can forward finished results from here into its own agent, while this agent and its current boss remain usable and running tasks finish here.")),
      ],
      buttons: [{ label: L("取消", "Cancel") }, { label: L("交给其他 boss", "Hand over"), primary: true, value: "handover" }],
    });
    if (!answer) return;
    try {
      const data = await AN.post("/app/workspaces/handover", { group: id });
      say(data.message || L("已标为待接手", "Marked as awaiting handover"), data.status !== "waiting");
    } catch (error) {
      say(L(`没有交接：${error.message}`, `Handover failed: ${error.message}`), true);
    }
    await refresh();
  }
  async function cancelHandover(id) {
    try {
      const data = await AN.post("/app/workspaces/cancel-handover", { group: id });
      const text = {
        cancelled: L("已取消交接", "Handover cancelled"),
        not_pending: L("已被接手或已取消，刷新后再看", "Already taken over or cancelled; refresh to check"),
        only_boss: L("这个 agent 没有会话，取消后会从列表消失；不需要时请删除这个 agent", "This agent has no conversations and would disappear if cancelled; delete the agent instead"),
      }[data.status];
      say(text || data.message || L("未能取消交接", "Failed to cancel handover"), data.status !== "cancelled");
    } catch (error) {
      say(L(`未能取消交接：${error.message}`, `Failed to cancel handover: ${error.message}`), true);
    }
    await refresh();
  }

  // ---- clean up quiet workspaces -----------------------------------------------------------
  async function cleanup() {
    let days = 30;
    const now = Date.now();
    let stateNow;
    try { stateNow = await manageState(); } catch (error) { say(L(`无法读取会话：${error.message}`, `Failed to load conversations: ${error.message}`), true); return; }
    const all = stateNow.items;
    // The office list is capped at 200 workspaces; cleanup uses the complete history listing.
    const groups = (items) => {
      const result = new Map();
      for (const item of items) {
        if (!item.group) continue;
        const id = item.group;
        if (!result.has(id)) result.set(id, []);
        result.get(id).push(item);
      }
      return result;
    };
    const quiet = ({ items, workspaces }) => {
      const cutoff = now - days * 864e5;
      const wsMap = new Map((workspaces || []).map((w) => [w.id, w]));
      const expired = (ts) => Number.isFinite(ts) && ts > 0 && ts < cutoff;
      const convGroups = groups(items);
      const found = [...convGroups]
        .filter(([id, records]) => {
          const ws = wsMap.get(id);
          if (ws?.pending) return false;
          if (ws?.handover_ms != null && !(Number.isFinite(ws.handover_ms) && ws.handover_ms < cutoff)) return false;
          return records.every((i) => i.deletable && expired(AN.at(i.updated)));
        })
        .map(([id, records]) => ({ id, records, ws: wsMap.get(id) || null, empty: false }));
      for (const ws of workspaces || []) {
        if (convGroups.has(ws.id) || !ws.empty_claimed || ws.handover_ms == null) continue;
        if (Number.isFinite(ws.handover_ms) && ws.handover_ms < cutoff) found.push({ id: ws.id, records: [], ws, empty: true });
      }
      return found;
    };
    const ungrouped = all.filter((i) => !i.group || !Number.isFinite(AN.at(i.updated)) || AN.at(i.updated) <= 0);
    const count = h("p.act-note");
    const dayLabel = (d) => L(`${d} 天`, `${d} days`);
    const choices = h("div.act-days", { role: "radiogroup", "aria-label": L("多久没有动静", "Inactive duration") }, DAYS.map((d) => {
      const b = h("button", { type: "button", role: "radio" }, dayLabel(d));
      b.addEventListener("click", () => { days = d; paint(); });
      return b;
    }));
    const paint = () => {
      for (const b of choices.children) b.setAttribute("aria-checked", String(b.textContent === dayLabel(days)));
      const n = quiet(stateNow).length;
      count.textContent = n
        ? L(`有 ${n} 个 agent 超过 ${days} 天没有动静。`, `${n} agent(s) have been inactive for over ${days} days.`)
        : L(`没有超过 ${days} 天没有动静的 agent。`, `No agents have been inactive for over ${days} days.`);
    };
    paint();
    const answer = await ask({
      title: L("清理没有动静的 agent", "Clean up inactive agents"),
      body: [h("p.act-note", {}, L("删除超过所选天数没有动静的 agent。进行中和待审核的不会删除。", "Deletes agents inactive for longer than the selected period. Running or review items are kept.")), choices, count],
      buttons: [
        { label: L("取消", "Cancel") },
        ...(ungrouped.length ? [{ label: L(`其他记录（${ungrouped.length}）…`, `Other records (${ungrouped.length})…`), value: "ungrouped" }] : []),
        { label: L("下一步", "Next"), primary: true, value: "next" },
      ],
    });
    if (!answer) return;
    if (answer.value === "ungrouped") {
      const selected = new Set();
      const choices = h("ul.act-list", {}, ungrouped.map((item) => {
        const checkbox = h("input", { type: "checkbox" });
        checkbox.addEventListener("change", () => {
          if (checkbox.checked) selected.add(item.id); else selected.delete(item.id);
        });
        return h("li", {}, h("label", {}, checkbox, ` ${item.title || item.id}`));
      }));
      const chosen = await ask({
        title: L("无法归组或时间不明的记录", "Ungrouped or undated records"),
        body: [h("p.act-note", {}, L("这些记录不参与按天数自动清理。请选择要清理的记录，下一步会再次确认。", "These records are excluded from age-based cleanup. Select records to delete; you will confirm in the next step.")), choices],
        buttons: [{ label: L("取消", "Cancel") }, { label: L("下一步", "Next"), primary: true, value: "next", check: () => selected.size ? "" : L("请先选择记录", "Select at least one record") }],
      });
      if (chosen) {
        const records = ungrouped.filter((i) => selected.has(i.id));
        await remove(records.map((i) => i.title || i.id), records, L("这些记录", "these records"));
      }
      return;
    }
    const selected = new Set(quiet(stateNow).map((e) => e.id));
    let fresh;
    try { fresh = await manageState(); } catch (error) { say(L(`无法重新核对会话：${error.message}`, `Failed to recheck conversations: ${error.message}`), true); return; }
    const eligible = quiet(fresh).filter((e) => selected.has(e.id));
    if (!eligible.length) { say(L("没有需要清理的 agent，或 agent 已有新活动", "No agents to clean up, or new activity occurred")); return; }
    const confirmedIds = new Set(all.map((i) => i.id));
    const confirmedEmpty = new Set((stateNow.workspaces || []).filter((w) => w.empty_claimed).map((w) => w.id));
    await remove(
      eligible.map((e) => state.groupsData.some((g) => g.group === e.id) ? groupName(e.id) : e.records[0]?.title || e.ws?.title || e.id),
      eligible.flatMap((e) => e.records.filter((i) => confirmedIds.has(i.id))),
      L(`这 ${eligible.length} 个 agent`, `these ${eligible.length} agents`),
      now - days * 864e5,
      eligible.filter((e) => e.empty && confirmedEmpty.has(e.id)).map((e) => e.id),
    );
  }

  // ---- manual projects --------------------------------------------------------------------
  let addingProject = false;
  async function addProject() {
    if (addingProject) return;
    addingProject = true;
    const button = $("add-project");
    if (button) button.disabled = true;
    try {
      const picked = await AN.post("/app/projects/pick", {});
      if (!picked?.cwd) return;
      const result = await AN.post("/app/projects/add", { cwd: picked.cwd });
      if (Array.isArray(result.projects)) state.projects = result.projects;
      state.listKey = null;
      await refresh();
      if (result.project?.key) {
        revealProject(result.project.key);
        // bringing the new project into view scrolls the list a frame later, and a scroll closes menus
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        const rec = state.heads?.get(`project:${result.project.key}`);
        const anchor = rec?.add || rec?.el || button;
        const rect = anchor?.getBoundingClientRect?.();
        const event = {
          clientX: rect?.width ? rect.left : 160,
          clientY: rect?.width ? rect.bottom + 4 : 96,
          preventDefault() {},
          stopPropagation() {},
        };
        if (typeof ChatTerminal !== "undefined") ChatTerminal?.newAgent?.(event, result.project.key);
      }
    } catch (error) {
      say(error.message || L("未能添加项目", "Failed to add project"), true);
    } finally {
      addingProject = false;
      if (button) button.disabled = false;
    }
  }
  async function removeProject(key) {
    try {
      const result = await AN.post("/app/projects/remove", { key });
      if (Array.isArray(result.projects)) state.projects = result.projects;
      state.listKey = null;
      await refresh();
    } catch (error) {
      say(error.message || L("未能移除项目", "Failed to remove project"), true);
    }
  }

  // ---- sidebar state writes ---------------------------------------------------------------
  function applySidebar(result) {
    if (result?.sidebar) {
      state.sidebar = typeof ChatGroups !== "undefined" && ChatGroups.normalizeSidebar
        ? ChatGroups.normalizeSidebar(result.sidebar)
        : result.sidebar;
    }
    state.listKey = null;
    renderList();
  }
  const isProjectPinned = (key) => Boolean(state.sidebar?.pinned_projects?.includes(key));
  const isGroupPinned = (group) => Boolean(state.sidebar?.pinned_groups?.includes(group));
  async function postSidebar(path, body, fallbackError) {
    try {
      const result = await AN.post(path, body);
      applySidebar(result);
      return result;
    } catch (error) {
      say(error.message || fallbackError, true);
      return null;
    }
  }
  async function pinProject(key, pinned) {
    await postSidebar("/app/projects/pin", { key, pinned }, L("未能更新项目置顶状态", "Failed to update project pin"));
  }
  async function pinGroup(group, pinned) {
    if (String(group).startsWith("term-")) return;
    await postSidebar("/app/chats/pin", { group, pinned }, L("未能更新 agent 置顶状态", "Failed to update agent pin"));
  }
  async function reorderProjects(order) {
    await postSidebar("/app/projects/order", { order }, L("未能保存项目顺序", "Failed to save project order"));
  }
  async function renameProject(section) {
    const currentAlias = state.sidebar?.project_aliases?.[section.key] || "";
    const rawName = section.rawTitle || section.title || section.key;
    await ask({
      title: L("重命名项目显示名", "Rename project display name"),
      body: [h("p.act-note", {}, L(`留空将恢复为目录名（${rawName}）。`, `Leave empty to restore the folder name (${rawName}).`))],
      input: { value: currentAlias, placeholder: rawName, label: L("显示名", "Display name") },
      buttons: [
        { label: L("取消", "Cancel") },
        {
          label: L("保存", "Save"),
          primary: true,
          value: "save",
          check: async (text) => {
            const alias = text.trim();
            if (alias === currentAlias) return "";
            try {
              const result = await AN.post("/app/projects/rename", { key: section.key, alias });
              applySidebar(result);
            } catch (error) {
              return error.message || L("未能重命名项目", "Failed to rename project");
            }
            say(alias ? L(`已重命名为“${alias}”`, `Renamed to "${alias}"`) : L("已恢复目录名", "Restored folder name"));
            return "";
          },
        },
      ],
    });
  }
  async function copyPath(path) {
    if (!path) return;
    try {
      await navigator.clipboard.writeText(path);
      say(L("已复制路径", "Path copied"));
    } catch {
      say(L("复制失败", "Copy failed"), true);
    }
  }

  // ---- menus ------------------------------------------------------------------------------
  function workspaceMenu(id, e) {
    if (String(id).startsWith("term-")) return;
    if (picked.size && !picked.has(id)) clearPicks();
    if (picked.size > 1) {
      const ids = [...picked];
      openMenu(e, [
        { label: L(`删除所选 ${ids.length} 个 agent…`, `Delete ${ids.length} selected agents…`), danger: true, run: () => removeGroups(ids) },
        { label: L("取消选择", "Clear selection"), run: clearPicks },
      ]);
      return;
    }
    const g = state.groupsData.find((x) => x.group === id);
    if (!g) return;
    const pinned = isGroupPinned(id);
    const endedTerm = typeof ChatTerminal !== "undefined" ? ChatTerminal?.exitedOf?.(id) : null;
    const derivedCount = typeof ChatTerminal !== "undefined" ? (ChatTerminal?.derivedOf?.(id)?.length || 0) : 0;
    openMenu(e, [
      { label: L("打开", "Open"), run: () => go(`/chat/${id}`) },
      { label: pinned ? L("取消置顶", "Unpin") : L("置顶", "Pin"), run: () => pinGroup(id, !pinned) },
      g.title_id && { label: L("改名…", "Rename…"), run: () => rename(g.title_id, chatName(g), L("agent", "agent")) },
      endedTerm && { label: L("关闭终端", "Close terminal"), run: () => ChatTerminal.close(endedTerm.id) },
      derivedCount > 0 && {
        label: L(`关闭全部派生终端（${derivedCount}）`, `Close all derived terminals (${derivedCount})`),
        danger: true,
        run: () => ChatTerminal.closeDerived(id),
      },
      g.handover === "pending"
        ? g.conversations && { label: L("取消交接", "Cancel handover"), run: () => cancelHandover(id) }
        : { label: L("交给其他 boss 接手…", "Hand over to another boss…"), run: () => handOver(id) },
      { label: L("删除 agent…", "Delete agent…"), danger: true, run: () => removeGroups([id]) },
    ].filter(Boolean));
  }
  function projectMenu(section, e) {
    if (!section || section.pinned || section.key === ":handover") return;
    const pos = { clientX: e.clientX, clientY: e.clientY, preventDefault() {}, stopPropagation() {} };
    const pinned = isProjectPinned(section.key);
    openMenu(e, [
      { label: L("在此项目新开 agent", "New agent in this project"), run: () => typeof ChatTerminal !== "undefined" && ChatTerminal?.newAgent?.(pos, section.other ? null : section.key) },
      !section.other && { label: pinned ? L("取消置顶", "Unpin") : L("置顶", "Pin"), run: () => pinProject(section.key, !pinned) },
      !section.other && { label: L("重命名显示名…", "Rename display name…"), run: () => renameProject(section) },
      !section.other && { label: L("复制路径", "Copy path"), run: () => copyPath(section.key || section.path) },
      section.manual && !section.items.length && { label: L("移除项目", "Remove project"), run: () => removeProject(section.key) },
    ].filter(Boolean));
  }
  function roomMenu(id, e) {
    const room = state.ctx?.rooms.get(id);
    if (!room) return;
    openMenu(e, [
      { label: L("查看对话", "View conversation"), run: () => openScope({ kind: "room", id }) },
      { label: L("改名…", "Rename…"), run: () => rename(id, room.title, L("会议", "meeting")) },
      { label: L("删除会议…", "Delete meeting…"), danger: true, run: () => removeRoom(id, room.title) },
    ]);
  }

  $("cleanup")?.addEventListener("click", cleanup);
  $("add-project")?.addEventListener("click", addProject);

  Object.assign(Actions, { picked, pick, ask, workspaceMenu, projectMenu, roomMenu, cancelHandover, deleteIsolated, openMenu, say, cleanup, pinProject, pinGroup, reorderProjects, renameProject });
}
