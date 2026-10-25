"use strict";
// Managing history from the office: right-click a workspace or a meeting room to rename or delete
// it; ⌘/Ctrl-click and Shift-click pick several workspaces to delete together; "清理" deletes the
// workspaces that have been quiet for a chosen number of days. Deleting takes the same locks as the
// servers, so conversations in use, running or awaiting review are kept and reported.
// Loaded before chat.js, which calls it; it reaches chat.js's globals (state, go, refresh,
// renderList, openScope) only from event handlers, once both scripts have run.
const Actions = (() => {
  const { h, $, load, chatName } = AN;
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
  async function conversations() {
    const data = await load("/manage/state");
    if (data.error) throw new Error(data.error);
    return data.items || [];
  }
  const groupName = (id) => {
    const g = state.groupsData.find((x) => x.group === id);
    return g ? chatName(g) : id;
  };

  // ---- dialogs ----------------------------------------------------------------------------
  // One modal at a time: a title, a body, and buttons; resolves with the button's value.
  function ask({ title, body = [], buttons, input }) {
    return new Promise((resolve) => {
      const field = input && h("input.act-input", { type: "text", value: input.value, "aria-label": input.label });
      const error = h("p.act-error", { role: "alert", hidden: true });
      const row = h("div.act-buttons", {}, buttons.map((b) => {
        const button = h(`button${b.danger ? ".danger" : ""}${b.primary ? ".primary" : ""}`, { type: "button" }, b.label);
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
      (field || row.querySelector(".primary") || row.querySelector("button")).focus();
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
      title: `给${what}改名`,
      body: [h("p.act-note", {}, "最多 24 个汉字或 48 个英文字符。")],
      input: { value: current, label: "新名字" },
      buttons: [
        { label: "取消" },
        {
          label: "保存",
          primary: true,
          value: "save",
          check: async (text) => {
            const next = text.trim();
            if (!next) return "名字不能为空";
            if (next === current) return "";
            try {
              const data = await AN.post("/app/conversations/rename", { id, title: next });
              if (data.status !== "renamed") return data.message || "未能改名";
            } catch (error) {
              return `未能改名：${error.message}`;
            }
            say(`已改名为“${next}”`);
            refresh();
            return "";
          },
        },
      ],
    });
  }

  // ---- delete -----------------------------------------------------------------------------
  // Deletes the given conversations after confirming; `names` describe what the reader picked.
  async function remove(names, items, what, cutoff = null) {
    const free = items.filter((i) => i.deletable);
    const kept = items.filter((i) => !i.deletable);
    if (!free.length) {
      await ask({
        title: `${what}现在不能删除`,
        body: [h("p.act-note", {}, kept[0]?.reason || "里面没有可删除的会话。")],
        buttons: [{ label: "知道了", primary: true }],
      });
      return;
    }
    const answer = await ask({
      title: `删除${what}？`,
      body: [
        h("ul.act-list", {}, names.map((n) => h("li", {}, n))),
        h("p.act-note", {}, `将删除 ${free.length} 个会话的历史记录，删除后不能恢复；各 CLI 自己的会话文件不受影响。`),
        kept.length > 0 && h("p.act-note.warn", {}, `${kept.length} 个会话正在运行、待审核或被宿主占用，这次会保留。`),
      ].filter(Boolean),
      buttons: [{ label: "取消" }, { label: "删除", danger: true, primary: true, value: "delete" }],
    });
    if (!answer) return;
    try {
      let data;
      if (cutoff === null) {
        data = await AN.post("/app/conversations/delete", { ids: free.map((i) => i.id) });
      } else {
        const groups = new Map();
        for (const item of free) {
          const group = item.group || `c-${item.id}`;
          if (!groups.has(group)) groups.set(group, []);
          groups.get(group).push(item.id);
        }
        data = { results: [] };
        for (const ids of groups.values()) {
          const outcome = await AN.post("/app/conversations/delete", { ids, updated_before: cutoff });
          data.results.push(...(outcome.results || []));
        }
      }
      const failed = (data.results || []).filter((r) => !["deleted", "missing"].includes(r.status));
      if (failed.length) say(`${failed.length} 个会话未删除：${failed[0].message}`, true);
      else say(`已删除 ${free.length} 个会话`);
    } catch (error) {
      say(`删除没有完成：${error.message}。刷新后核对再试。`, true);
    }
    for (const id of picked) if (!state.groupsData.some((g) => g.group === id)) picked.delete(id);
    await refresh();
  }
  async function removeGroups(ids) {
    let all;
    try { all = await conversations(); } catch (error) { say(`无法读取会话：${error.message}`, true); return; }
    const set = new Set(ids);
    const what = ids.length > 1 ? ` ${ids.length} 个工作区` : `工作区“${groupName(ids[0])}”`;
    await remove(ids.map(groupName), all.filter((i) => set.has(i.group)), what);
    for (const id of ids) picked.delete(id);
    renderList();
    if (ids.includes(state.selected) && !state.groupsData.some((g) => g.group === state.selected)) go("/chat/");
  }
  async function removeRoom(id, title) {
    let all;
    try { all = await conversations(); } catch (error) { say(`无法读取会话：${error.message}`, true); return; }
    await remove([title], all.filter((i) => i.id === id), `会议“${title}”`);
  }

  // ---- clean up quiet workspaces -----------------------------------------------------------
  async function cleanup() {
    let days = 30;
    const now = Date.now();
    let all;
    try { all = await conversations(); } catch (error) { say(`无法读取会话：${error.message}`, true); return; }
    // The office list is capped at 200 workspaces; cleanup uses the complete history listing.
    const groups = (items) => {
      const result = new Map();
      for (const item of items) {
        const id = item.group || `c-${item.id}`;
        if (!result.has(id)) result.set(id, []);
        result.get(id).push(item);
      }
      return result;
    };
    const quiet = (items) => [...groups(items)].filter(([, records]) => records.every((i) => {
      const updated = AN.at(i.updated);
      return i.deletable && Number.isFinite(updated) && updated > 0 && now - updated > days * 864e5;
    }));
    const ungrouped = all.filter((i) => !i.group || !Number.isFinite(AN.at(i.updated)) || AN.at(i.updated) <= 0);
    const count = h("p.act-note");
    const choices = h("div.act-days", { role: "radiogroup", "aria-label": "多久没有动静" }, DAYS.map((d) => {
      const b = h("button", { type: "button", role: "radio" }, `${d} 天`);
      b.addEventListener("click", () => { days = d; paint(); });
      return b;
    }));
    const paint = () => {
      for (const b of choices.children) b.setAttribute("aria-checked", String(b.textContent === `${days} 天`));
      const n = quiet(all).length;
      count.textContent = n ? `有 ${n} 个工作区超过 ${days} 天没有动静。` : `没有超过 ${days} 天没有动静的工作区。`;
    };
    paint();
    const answer = await ask({
      title: "清理没有动静的工作区",
      body: [h("p.act-note", {}, "删除超过所选天数没有动静的工作区。进行中和待审核的不会删除。"), choices, count],
      buttons: [
        { label: "取消" },
        ...(ungrouped.length ? [{ label: `其他记录（${ungrouped.length}）…`, value: "ungrouped" }] : []),
        { label: "下一步", primary: true, value: "next" },
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
        title: "无法归组或时间不明的记录",
        body: [h("p.act-note", {}, "这些记录不参与按天数自动清理。请选择要清理的记录，下一步会再次确认。"), choices],
        buttons: [{ label: "取消" }, { label: "下一步", primary: true, value: "next", check: () => selected.size ? "" : "请先选择记录" }],
      });
      if (chosen) {
        const records = ungrouped.filter((i) => selected.has(i.id));
        await remove(records.map((i) => i.title || i.id), records, "这些记录");
      }
      return;
    }
    const selected = new Set(quiet(all).map(([id]) => id));
    let fresh;
    try { fresh = await conversations(); } catch (error) { say(`无法重新核对会话：${error.message}`, true); return; }
    const eligible = quiet(fresh).filter(([id]) => selected.has(id));
    if (!eligible.length) { say("没有需要清理的工作区，或工作区已有新活动"); return; }
    const confirmedIds = new Set(all.map((i) => i.id));
    await remove(
      eligible.map(([id, records]) => state.groupsData.some((g) => g.group === id) ? groupName(id) : records[0].title || id),
      eligible.flatMap(([, records]) => records.filter((i) => confirmedIds.has(i.id))),
      `这 ${eligible.length} 个工作区`,
      now - days * 864e5,
    );
  }

  // ---- menus ------------------------------------------------------------------------------
  function workspaceMenu(id, e) {
    if (picked.size && !picked.has(id)) clearPicks();
    if (picked.size > 1) {
      const ids = [...picked];
      openMenu(e, [
        { label: `删除所选 ${ids.length} 个工作区…`, danger: true, run: () => removeGroups(ids) },
        { label: "取消选择", run: clearPicks },
      ]);
      return;
    }
    const g = state.groupsData.find((x) => x.group === id);
    if (!g) return;
    openMenu(e, [
      { label: "打开", run: () => go(`/chat/${id}`) },
      g.title_id && { label: "改名…", run: () => rename(g.title_id, chatName(g), "工作区") },
      { label: "删除工作区…", danger: true, run: () => removeGroups([id]) },
    ].filter(Boolean));
  }
  function roomMenu(id, e) {
    const room = state.ctx?.rooms.get(id);
    if (!room) return;
    openMenu(e, [
      { label: "查看对话", run: () => openScope({ kind: "room", id }) },
      { label: "改名…", run: () => rename(id, room.title, "会议") },
      { label: "删除会议…", danger: true, run: () => removeRoom(id, room.title) },
    ]);
  }

  $("cleanup").addEventListener("click", cleanup);

  return { picked, pick, workspaceMenu, roomMenu };
})();
