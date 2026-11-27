"use strict";
// Sidebar controls: search with keyboard navigation, view/sort/filter popover,
// top-bar more menu (collapse/expand all, clean up), and project header drag-and-drop.
(() => {
  const { h, $, L, setAttr } = AN;
  const VIEW_KEY = "agentnave.office.sidebarView";
  const normalizePrefs = ChatGroups.normalizePrefs;
  const isDefaultPrefs = ChatGroups.isDefaultPrefs;

  let currentPrefs = loadPrefs();
  let searchQuery = "";
  let activeId = null;
  let popover = null;
  let newChatPop = null;
  let newChatAnchor = null;
  let newChatStep = null;
  let newChatToken = 0;
  let dragState = null;
  let initialized = false;

  function loadPrefs() {
    try {
      const raw = localStorage.getItem(VIEW_KEY);
      return normalizePrefs(raw ? JSON.parse(raw) : null);
    } catch {
      return normalizePrefs(null);
    }
  }

  function savePrefs(next) {
    currentPrefs = normalizePrefs(next);
    try {
      if (isDefaultPrefs(currentPrefs)) localStorage.removeItem(VIEW_KEY);
      else localStorage.setItem(VIEW_KEY, JSON.stringify(currentPrefs));
    } catch { /* storage may be unavailable */ }
  }

  function prefs() {
    return currentPrefs;
  }

  function setPrefs(patch) {
    savePrefs({ ...currentPrefs, ...patch });
    syncUI();
    if (popover && !popover.hidden) renderPopover();
    if (typeof renderList === "function") renderList();
  }

  function query() {
    return searchQuery;
  }

  function isSearching() {
    return Boolean(searchQuery.trim());
  }

  function setQuery(next) {
    const val = String(next ?? "");
    if (val === searchQuery) return;
    searchQuery = val;
    const input = $("side-search-input");
    if (input && input.value !== val) input.value = val;
    if (isSearching()) {
      activeId = null;
    }
    syncUI();
    if (typeof renderList === "function") renderList();
    if (isSearching() && typeof state !== "undefined") {
      if (!activeId || !state.order.includes(activeId)) activeId = state.order[0] || null;
      syncActiveHighlight();
    }
  }

  function syncUI() {
    const viewBtn = $("side-view-btn");
    if (viewBtn) setAttr(viewBtn, "data-active", isDefaultPrefs(currentPrefs) ? null : "true");
    const input = $("side-search-input");
    if (input) {
      const text = L("搜索", "Search");
      setAttr(input, "placeholder", text);
      setAttr(input, "aria-label", text);
    }
    const clearBtn = $("side-search-clear");
    if (clearBtn) clearBtn.hidden = !searchQuery;
  }

  function syncActiveHighlight() {
    if (typeof state === "undefined" || !state.items) return;
    if (activeId && !state.order.includes(activeId)) {
      activeId = isSearching() ? (state.order[0] || null) : null;
    }
    for (const [id, rec] of state.items) {
      rec.el?.classList?.toggle("kbd-active", id === activeId);
    }
    const input = $("side-search-input");
    if (input) {
      if (activeId) {
        const activeEl = state.items.get(activeId)?.el;
        if (activeEl && !activeEl.id) activeEl.id = `gi-${activeId}`;
        setAttr(input, "aria-activedescendant", activeEl?.id || null);
      } else {
        setAttr(input, "aria-activedescendant", null);
      }
    }
  }

  function moveActive(step) {
    if (typeof state === "undefined" || !state.order?.length) return;
    const order = state.order;
    const idx = activeId ? order.indexOf(activeId) : -1;
    let nextIdx;
    if (idx < 0) nextIdx = step > 0 ? 0 : order.length - 1;
    else nextIdx = (idx + step + order.length) % order.length;
    activeId = order[nextIdx];
    syncActiveHighlight();
    state.items.get(activeId)?.el?.scrollIntoView?.({ block: "nearest" });
  }

  function openActive() {
    if (typeof state === "undefined" || typeof go !== "function") return;
    const target = activeId && state.order.includes(activeId) ? activeId : (isSearching() ? state.order[0] : null);
    if (!target) return;
    if (typeof ChatTerminal !== "undefined" && ChatTerminal?.openDerived?.(target)) return;
    go(`/chat/${encodeURIComponent(target)}`);
  }

  function isTerminalFocused(e) {
    if (document.activeElement?.tagName === "IFRAME") return true;
    return Boolean(e.target?.closest?.(".dr-term, iframe"));
  }

  function isTextInput(target) {
    if (!target) return false;
    if (target.isContentEditable) return true;
    const tag = target.tagName;
    if (tag === "TEXTAREA" || tag === "SELECT") return true;
    if (tag === "INPUT") {
      const nonText = new Set(["checkbox", "radio", "button", "submit", "reset", "range", "color", "file", "image", "hidden"]);
      return !nonText.has(target.type);
    }
    return false;
  }

  function focusSearch() {
    if (typeof state !== "undefined" && typeof narrow !== "undefined" && !narrow.matches) {
      if (typeof railMode === "function" && railMode()) {
        if (typeof toggleRailPeek === "function") toggleRailPeek(true);
      } else if (state.sideCollapsed && typeof setPeek === "function") {
        setPeek("side", true);
      }
    }
    const input = $("side-search-input");
    if (!input) return;
    input.focus({ preventScroll: true });
    input.select?.();
  }

  // ---- view/sort/filter popover --------------------------------------------------------
  function closePopover() {
    if (!popover || popover.hidden) return;
    popover.hidden = true;
    const btn = $("side-view-btn");
    if (btn) setAttr(btn, "aria-expanded", "false");
  }

  function popoverRow(label, checked, onClick, key) {
    const mark = h("span.side-pop-mark", { "aria-hidden": "true" }, checked ? "✓" : "");
    const btn = h("button.side-pop-item", { type: "button", role: "menuitemcheckbox", "aria-checked": checked ? "true" : "false", "data-pop-key": key }, mark, label);
    btn.addEventListener("click", () => onClick());
    return btn;
  }

  function renderPopover() {
    if (!popover) return;
    const activeEl = document.activeElement;
    const activeKey = activeEl && popover.contains?.(activeEl)
      ? activeEl.getAttribute?.("data-pop-key")
      : null;
    const p = currentPrefs;
    const flat = p.view === "flat";
    const nodes = [
      h("div.side-pop-sec", {}, L("视图", "View")),
      popoverRow(L("按项目分组", "By project"), p.view === "project", () => setPrefs({ view: "project" }), "view:project"),
      popoverRow(L("按时间平铺", "Flat timeline"), p.view === "flat", () => setPrefs({ view: "flat" }), "view:flat"),
      !flat && h("div.side-pop-sep"),
      !flat && h("div.side-pop-sec", {}, L("项目排序", "Project order")),
      !flat && popoverRow(L("最近活动", "Recent activity"), p.projectSort === "recent", () => setPrefs({ projectSort: "recent" }), "projectSort:recent"),
      !flat && popoverRow(L("名称 A–Z", "Name A–Z"), p.projectSort === "name", () => setPrefs({ projectSort: "name" }), "projectSort:name"),
      !flat && popoverRow(L("手动顺序", "Manual order"), p.projectSort === "manual", () => setPrefs({ projectSort: "manual" }), "projectSort:manual"),
      h("div.side-pop-sep"),
      h("div.side-pop-sec", {}, L("Agent 排序", "Agent order")),
      popoverRow(L("最近活动", "Recent activity"), p.itemSort === "updated", () => setPrefs({ itemSort: "updated" }), "itemSort:updated"),
      popoverRow(L("创建时间", "Created time"), p.itemSort === "created", () => setPrefs({ itemSort: "created" }), "itemSort:created"),
      h("div.side-pop-sep"),
      h("div.side-pop-sec", {}, L("筛选与选项", "Filter & options")),
      popoverRow(L("全部", "All"), p.filter === "all", () => setPrefs({ filter: "all" }), "filter:all"),
      popoverRow(L("仅活跃", "Active only"), p.filter === "active", () => setPrefs({ filter: "active" }), "filter:active"),
      h("div.side-pop-sep"),
      popoverRow(L("运行中/待审置顶", "Urgent first"), p.urgentFirst, () => setPrefs({ urgentFirst: !p.urgentFirst }), "urgentFirst"),
      !flat && popoverRow(L("隐藏空项目", "Hide empty projects"), p.hideEmpty, () => setPrefs({ hideEmpty: !p.hideEmpty }), "hideEmpty"),
    ].filter(Boolean);
    popover.replaceChildren(...nodes);
    if (activeKey) {
      const match = nodes.find((n) => n.getAttribute?.("data-pop-key") === activeKey);
      (match || popover.querySelector("button"))?.focus?.({ preventScroll: true });
    }
  }

  function openPopover(anchor) {
    if (!popover) {
      popover = h("div.side-popover", { role: "menu", hidden: true });
      popover.addEventListener("keydown", (e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          closePopover();
          $("side-view-btn")?.focus({ preventScroll: true });
          return;
        }
        const buttons = [...popover.querySelectorAll("button")];
        if (!buttons.length) return;
        const i = buttons.indexOf(document.activeElement);
        const step = { ArrowDown: 1, ArrowUp: -1 }[e.key];
        if (!step) return;
        e.preventDefault();
        buttons[(i + step + buttons.length) % buttons.length].focus();
      });
      document.body.append(popover);
    }
    if (!popover.hidden) {
      closePopover();
      return;
    }
    closeNewChat();
    renderPopover();
    popover.hidden = false;
    setAttr(anchor, "aria-expanded", "true");
    positionPopover(popover, anchor);
    popover.querySelector("button")?.focus({ preventScroll: true });
  }

  function positionPopover(boxEl, anchor) {
    const rect = anchor.getBoundingClientRect();
    const width = boxEl.offsetWidth || boxEl.getBoundingClientRect().width;
    const height = boxEl.offsetHeight || boxEl.getBoundingClientRect().height;
    const left = Math.max(6, Math.min(rect.left, innerWidth - width - 6));
    const top = Math.max(6, Math.min(rect.bottom + 4, innerHeight - height - 6));
    boxEl.style.left = `${left}px`;
    boxEl.style.top = `${top}px`;
  }

  // ---- new chat two-step popover (project -> CLI -> terminal) ---------------------------
  function closeNewChat() {
    newChatToken += 1;
    if (!newChatPop || newChatPop.hidden) return;
    newChatPop.hidden = true;
    newChatStep = null;
    const btn = $("new-chat");
    if (btn) setAttr(btn, "aria-expanded", "false");
  }

  function pickableProjects() {
    if (typeof state === "undefined") return [];
    const sections = ChatGroups.arrange(state.groupsData || [], {
      projects: state.projects || [],
      urgent: AN.urgent,
      L,
      sidebar: state.sidebar,
      prefs: { ...currentPrefs, view: "project", filter: "all", hideEmpty: false },
      query: "",
    });
    return sections.filter((s) => !s.pinned && !s.other);
  }

  function ensureNewChatPop() {
    if (newChatPop) return newChatPop;
    newChatPop = h("div.side-popover", { role: "menu", hidden: true });
    newChatPop.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        closeNewChat();
        $("new-chat")?.focus({ preventScroll: true });
        return;
      }
      if ((e.key === "ArrowLeft" || e.key === "Backspace") && newChatStep === "cli" && pickableProjects().length) {
        e.preventDefault();
        renderNewChatProjects(-1);
        return;
      }
      const buttons = [...newChatPop.querySelectorAll("button")];
      if (!buttons.length) return;
      const i = buttons.indexOf(document.activeElement);
      const step = { ArrowDown: 1, ArrowUp: -1 }[e.key];
      if (!step) return;
      e.preventDefault();
      buttons[(i + step + buttons.length) % buttons.length].focus();
    });
    document.body.append(newChatPop);
    return newChatPop;
  }

  function renderNewChatProjects(dir = 0) {
    newChatToken += 1;
    const pop = ensureNewChatPop();
    const list = pickableProjects();
    if (!list.length) {
      pop.replaceChildren();
      void renderNewChatClis({ key: null, title: L("选择文件夹", "Choose folder") }, false);
      return;
    }
    newChatStep = "project";
    const rows = list.map((s) => {
      const btn = h(
        "button.side-pop-item",
        { type: "button", role: "menuitem", title: s.path || s.title },
        h("span.side-pop-folder", { "aria-hidden": "true" }),
        h("span.side-pop-name", {}, s.title),
        s.pinnedProject && h("span.gl-grp-pin", { "aria-hidden": "true" }),
        s.hint && h("span.side-pop-hint", {}, s.hint),
      );
      btn.addEventListener("click", () => void renderNewChatClis({ key: s.key, title: s.title }, true));
      return btn;
    });
    const pickOther = h("button.side-pop-item", { type: "button", role: "menuitem" }, L("选择其他文件夹…", "Choose folder…"));
    pickOther.addEventListener("click", () => void renderNewChatClis({ key: null, title: L("选择其他文件夹", "Choose folder") }, true));
    const listBox = h("div.side-pop-list", {}, ...rows);
    pop.replaceChildren(listBox, h("div.side-pop-sep"), pickOther);
    if (dir) globalThis.ANMotion?.reveal?.(listBox, { dx: dir * 10, dy: 0, duration: 150 });
    if (newChatAnchor) positionPopover(pop, newChatAnchor);
    pop.querySelector("button")?.focus({ preventScroll: true });
  }

  async function renderNewChatClis(choice, canBack) {
    const pop = ensureNewChatPop();
    const token = ++newChatToken;
    newChatStep = "cli";
    const back = canBack && h("button.side-pop-item.side-pop-back", { type: "button", role: "menuitem" }, `‹ ${choice.title}`);
    if (back) back.addEventListener("click", () => renderNewChatProjects(-1));
    const found = typeof ChatTerminal !== "undefined" && ChatTerminal.clis
      ? await ChatTerminal.clis().catch(() => [])
      : [];
    if (token !== newChatToken || pop.hidden) return;
    const rows = found.flatMap(({ provider, accounts }) => [
      { label: h("span.ctx-cli", {}, AN.hostTile(provider), AN.cliName(provider)), provider, account: undefined },
      ...(accounts || []).map((account) => ({
        label: h("span.ctx-cli", {}, AN.hostTile(provider), L(`${AN.cliName(provider)}（${account.label}）`, `${AN.cliName(provider)} (${account.label})`)),
        provider,
        account: account.id,
      })),
    ]).map((entry) => {
      const btn = h("button.side-pop-item", { type: "button", role: "menuitem" }, entry.label);
      btn.addEventListener("click", () => {
        closeNewChat();
        void ChatTerminal?.start?.(entry.provider, choice.key, entry.account);
      });
      return btn;
    });
    const body = rows.length
      ? h("div.side-pop-list", {}, ...rows)
      : h("div.side-pop-empty", {}, L("没有检测到已安装的 CLI", "No installed CLI found"));
    pop.replaceChildren(...[back, back && h("div.side-pop-sep"), body].filter(Boolean));
    if (canBack) globalThis.ANMotion?.reveal?.(body, { dx: 10, dy: 0, duration: 150 });
    if (newChatAnchor) positionPopover(pop, newChatAnchor);
    (rows[0] || pop.querySelector("button"))?.focus({ preventScroll: true });
  }

  function openNewChat(anchor) {
    const pop = ensureNewChatPop();
    if (!pop.hidden) {
      closeNewChat();
      return;
    }
    closePopover();
    newChatAnchor = anchor;
    pop.hidden = false;
    setAttr(anchor, "aria-expanded", "true");
    renderNewChatProjects();
  }

  function openMoreMenu(anchor) {
    if (typeof Actions === "undefined" || !Actions.openMenu) return;
    closePopover();
    closeNewChat();
    const rect = anchor.getBoundingClientRect();
    const event = {
      clientX: rect.left,
      clientY: rect.bottom + 4,
      preventDefault() {},
      stopPropagation() {},
    };
    const flat = currentPrefs.view === "flat";
    Actions.openMenu(event, [
      !flat && { label: L("全部折叠", "Collapse all"), run: () => typeof setAllFolded === "function" && setAllFolded(true) },
      !flat && { label: L("全部展开", "Expand all"), run: () => typeof setAllFolded === "function" && setAllFolded(false) },
      { label: L("清理…", "Clean up…"), run: () => Actions.cleanup?.() },
    ].filter(Boolean));
  }

  // ---- drag-and-drop project reordering -------------------------------------------------
  const DRAG_EDGE_PX = 40;
  const DRAG_SCROLL_MAX = 12;
  let boundWrap = null;

  function canDrag(section) {
    return Boolean(
      currentPrefs.view === "project" &&
      !isSearching() &&
      section &&
      !section.pinned &&
      !section.other,
    );
  }

  function clearDropIndicators() {
    if (typeof state !== "undefined") {
      for (const rec of state.heads?.values?.() || []) {
        rec.el?.classList?.remove("dragging");
        setAttr(rec.el, "data-drop", null);
      }
      for (const rec of state.items?.values?.() || []) setAttr(rec.el, "data-drop", null);
      for (const rec of state.moreNodes?.values?.() || []) setAttr(rec.el, "data-drop", null);
    }
    const list = $("list");
    if (list?.querySelectorAll) {
      for (const node of list.querySelectorAll("[data-drop]")) setAttr(node, "data-drop", null);
    }
    if (dragState?.dropEl) setAttr(dragState.dropEl, "data-drop", null);
  }

  function resolveDropIndicator(rec, pos) {
    if (pos !== "after") return { el: rec.el, pos: "before" };
    const parent = rec.el.parentElement || $("list");
    const children = parent?.children ? [...parent.children] : [];
    const idx = children.indexOf(rec.el);
    if (idx < 0) return { el: rec.el, pos: "after" };
    const groupsMore = state.moreNodes?.get?.("groups:project")?.el || null;
    let lastVisible = rec.el;
    for (let i = idx + 1; i < children.length; i++) {
      const child = children[i];
      if (groupsMore && child === groupsMore) break;
      let nextHead = null;
      for (const hRec of state.heads.values()) {
        if (hRec.el === child) { nextHead = hRec; break; }
      }
      if (nextHead) {
        if (canDrag(nextHead.section) && Boolean(nextHead.section.pinnedProject) === dragState.pinned) {
          return { el: nextHead.el, pos: "before" };
        }
        break;
      }
      if (!child.hidden && child.getAttribute?.("data-folded") !== "true") {
        lastVisible = child;
      }
    }
    return { el: lastVisible, pos: "after" };
  }

  function bindWrapDragScroll() {
    const list = $("list");
    const wrap = list?.closest?.(".gl-wrap") || list?.parentElement || document.querySelector?.(".gl-wrap");
    if (!wrap || wrap === boundWrap || !wrap.addEventListener) return;
    boundWrap = wrap;
    wrap.addEventListener("dragover", (e) => {
      if (!dragState || !wrap.getBoundingClientRect) return;
      const rect = wrap.getBoundingClientRect();
      if (!rect || rect.height <= 0) return;
      const topDist = e.clientY - rect.top;
      const bottomDist = rect.bottom - e.clientY;
      let delta = 0;
      if (topDist < DRAG_EDGE_PX && topDist >= -4) {
        const dist = Math.max(0, topDist);
        delta = -Math.ceil(((DRAG_EDGE_PX - dist) / DRAG_EDGE_PX) * DRAG_SCROLL_MAX);
      } else if (bottomDist < DRAG_EDGE_PX && bottomDist >= -4) {
        const dist = Math.max(0, bottomDist);
        delta = Math.ceil(((DRAG_EDGE_PX - dist) / DRAG_EDGE_PX) * DRAG_SCROLL_MAX);
      }
      if (delta) {
        const maxTop = Number.isFinite(wrap.scrollHeight) && Number.isFinite(wrap.clientHeight)
          ? Math.max(0, wrap.scrollHeight - wrap.clientHeight)
          : Infinity;
        wrap.scrollTop = Math.min(maxTop, Math.max(0, (wrap.scrollTop || 0) + delta));
      }
    });
  }

  function computeReorderedKeys(dragKey, targetKey, pos) {
    if (typeof state === "undefined") return null;
    const sections = ChatGroups.arrange(state.groupsData || [], {
      projects: state.projects || [],
      urgent: AN.urgent,
      L,
      sidebar: state.sidebar,
      prefs: { ...currentPrefs, view: "project", filter: "all", hideEmpty: false },
      query: "",
    });
    const keys = sections.filter((s) => !s.pinned && !s.other).map((s) => s.key);
    const seen = new Set(keys);
    for (const saved of state.sidebar?.project_order || []) {
      if (saved && !seen.has(saved)) {
        keys.push(saved);
        seen.add(saved);
      }
    }
    const from = keys.indexOf(dragKey);
    if (from < 0) return null;
    keys.splice(from, 1);
    const to = keys.indexOf(targetKey);
    if (to < 0) return null;
    keys.splice(pos === "after" ? to + 1 : to, 0, dragKey);
    return keys;
  }

  function attachDrag(rec) {
    bindWrapDragScroll();
    if (!rec.dragAttached) {
      rec.dragAttached = true;
      rec.el.addEventListener("dragstart", (e) => {
        if (e.target.closest?.(".gl-grp-add") || !canDrag(rec.section)) {
          e.preventDefault();
          return;
        }
        dragState = {
          key: rec.section.key,
          pinned: Boolean(rec.section.pinnedProject),
          pos: null,
          dropEl: null,
          dropOwner: null,
        };
        rec.el.classList.add("dragging");
        if (e.dataTransfer) {
          e.dataTransfer.effectAllowed = "move";
          e.dataTransfer.setData("text/plain", rec.section.key);
        }
      });
      rec.el.addEventListener("dragover", (e) => {
        if (!dragState || !canDrag(rec.section)) return;
        if (rec.section.key === dragState.key) return;
        if (Boolean(rec.section.pinnedProject) !== dragState.pinned) return;
        e.preventDefault();
        if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        const rect = rec.el.getBoundingClientRect();
        const pos = e.clientY >= rect.top + rect.height / 2 ? "after" : "before";
        const target = resolveDropIndicator(rec, pos);
        if (dragState.dropEl && dragState.dropEl !== target.el) {
          setAttr(dragState.dropEl, "data-drop", null);
        }
        dragState.pos = pos;
        dragState.dropEl = target.el;
        dragState.dropOwner = rec;
        setAttr(target.el, "data-drop", target.pos);
      });
      rec.el.addEventListener("dragleave", (e) => {
        if (rec.el.contains?.(e.relatedTarget)) return;
        setAttr(rec.el, "data-drop", null);
        if (dragState?.dropOwner === rec && dragState.dropEl) {
          setAttr(dragState.dropEl, "data-drop", null);
          dragState.dropEl = null;
          dragState.dropOwner = null;
        }
      });
      rec.el.addEventListener("drop", (e) => {
        if (!dragState || !canDrag(rec.section)) return;
        if (rec.section.key === dragState.key || Boolean(rec.section.pinnedProject) !== dragState.pinned) return;
        e.preventDefault();
        const rect = rec.el.getBoundingClientRect();
        const pos = dragState.pos || (e.clientY >= rect.top + rect.height / 2 ? "after" : "before");
        const nextOrder = computeReorderedKeys(dragState.key, rec.section.key, pos);
        clearDropIndicators();
        dragState = null;
        if (!nextOrder) return;
        savePrefs({ ...currentPrefs, projectSort: "manual" });
        syncUI();
        if (popover && !popover.hidden) renderPopover();
        if (state.sidebar) state.sidebar = { ...state.sidebar, project_order: nextOrder };
        if (typeof renderList === "function") renderList();
        Actions?.reorderProjects?.(nextOrder);
      });
      rec.el.addEventListener("dragend", () => {
        clearDropIndicators();
        dragState = null;
      });
    }
    setAttr(rec.el, "draggable", canDrag(rec.section) ? "true" : null);
  }

  // ---- init & listeners ----------------------------------------------------------------
  function bindSearch(input, clearBtn) {
    input.addEventListener("input", () => setQuery(input.value));
    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (searchQuery) setQuery("");
        else input.blur();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        moveActive(1);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        moveActive(-1);
      } else if (e.key === "Enter" && !e.isComposing && e.keyCode !== 229) {
        e.preventDefault();
        openActive();
      }
    });
    input.addEventListener("blur", () => {
      if (!isSearching() && activeId) {
        activeId = null;
        syncActiveHighlight();
      }
    });
    clearBtn?.addEventListener("click", () => {
      setQuery("");
      input.focus({ preventScroll: true });
    });
  }

  function init() {
    if (initialized) return;
    initialized = true;
    const viewBtn = $("side-view-btn");
    const newChatBtn = $("new-chat");
    const moreBtn = $("side-more-btn");
    const input = $("side-search-input");
    const clearBtn = $("side-search-clear");
    viewBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      openPopover(viewBtn);
    });
    newChatBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      openNewChat(newChatBtn);
    });
    moreBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      openMoreMenu(moreBtn);
    });
    if (input) bindSearch(input, clearBtn);
    bindWrapDragScroll();

    document.addEventListener("pointerdown", (e) => {
      if (popover && !popover.hidden && !popover.contains(e.target) && !$("side-view-btn")?.contains(e.target)) {
        closePopover();
      }
      if (newChatPop && !newChatPop.hidden && !newChatPop.contains(e.target) && !$("new-chat")?.contains(e.target)) {
        closeNewChat();
      }
    }, true);
    window.addEventListener("blur", () => {
      closePopover();
      closeNewChat();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && newChatPop && !newChatPop.hidden) {
        e.preventDefault();
        closeNewChat();
        return;
      }
      if (e.key === "Escape" && popover && !popover.hidden) {
        e.preventDefault();
        closePopover();
        return;
      }
      if (e.key.toLowerCase() === "f" && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        if (isTerminalFocused(e)) return;
        e.preventDefault();
        focusSearch();
        return;
      }
      if (e.key === "/" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        if (isTextInput(e.target) || isTerminalFocused(e)) return;
        if (document.querySelector("dialog[open]")) return;
        e.preventDefault();
        focusSearch();
      }
    });
    syncUI();
  }

  if (typeof document !== "undefined" && document.addEventListener) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
    else init();
  }

  window.ChatSidebar = {
    init,
    prefs,
    setPrefs,
    query,
    setQuery,
    isSearching,
    syncUI,
    syncActiveHighlight,
    attachDrag,
    closePopover,
    closeNewChat,
  };
})();
