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
    if (typeof state !== "undefined" && typeof narrow !== "undefined" && state.sideCollapsed && !narrow.matches) {
      if (typeof setPeek === "function") setPeek("side", true);
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
    popover.replaceChildren();
    const btn = $("side-view-btn");
    if (btn) setAttr(btn, "aria-expanded", "false");
  }

  function popoverRow(label, checked, onClick) {
    const mark = h("span.side-pop-mark", { "aria-hidden": "true" }, checked ? "✓" : "");
    const btn = h("button.side-pop-item", { type: "button", role: "menuitemcheckbox", "aria-checked": checked ? "true" : "false" }, mark, label);
    btn.addEventListener("click", () => onClick());
    return btn;
  }

  function renderPopover() {
    if (!popover) return;
    const p = currentPrefs;
    const flat = p.view === "flat";
    const nodes = [
      h("div.side-pop-sec", {}, L("视图", "View")),
      popoverRow(L("按项目分组", "By project"), p.view === "project", () => setPrefs({ view: "project" })),
      popoverRow(L("按时间平铺", "Flat timeline"), p.view === "flat", () => setPrefs({ view: "flat" })),
      !flat && h("div.side-pop-sep"),
      !flat && h("div.side-pop-sec", {}, L("项目排序", "Project order")),
      !flat && popoverRow(L("最近活动", "Recent activity"), p.projectSort === "recent", () => setPrefs({ projectSort: "recent" })),
      !flat && popoverRow(L("名称 A–Z", "Name A–Z"), p.projectSort === "name", () => setPrefs({ projectSort: "name" })),
      !flat && popoverRow(L("手动顺序", "Manual order"), p.projectSort === "manual", () => setPrefs({ projectSort: "manual" })),
      h("div.side-pop-sep"),
      h("div.side-pop-sec", {}, L("Agent 排序", "Agent order")),
      popoverRow(L("最近活动", "Recent activity"), p.itemSort === "updated", () => setPrefs({ itemSort: "updated" })),
      popoverRow(L("创建时间", "Created time"), p.itemSort === "created", () => setPrefs({ itemSort: "created" })),
      h("div.side-pop-sep"),
      h("div.side-pop-sec", {}, L("筛选与选项", "Filter & options")),
      popoverRow(L("全部", "All"), p.filter === "all", () => setPrefs({ filter: "all" })),
      popoverRow(L("仅活跃", "Active only"), p.filter === "active", () => setPrefs({ filter: "active" })),
      h("div.side-pop-sep"),
      popoverRow(L("运行中/待审置顶", "Urgent first"), p.urgentFirst, () => setPrefs({ urgentFirst: !p.urgentFirst })),
      !flat && popoverRow(L("隐藏空项目", "Hide empty projects"), p.hideEmpty, () => setPrefs({ hideEmpty: !p.hideEmpty })),
    ].filter(Boolean);
    popover.replaceChildren(...nodes);
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
    renderPopover();
    popover.hidden = false;
    setAttr(anchor, "aria-expanded", "true");
    const rect = anchor.getBoundingClientRect();
    const box = popover.getBoundingClientRect();
    const left = Math.max(6, Math.min(rect.left, innerWidth - box.width - 6));
    const top = Math.min(rect.bottom + 4, innerHeight - box.height - 6);
    popover.style.left = `${left}px`;
    popover.style.top = `${top}px`;
    popover.querySelector("button")?.focus({ preventScroll: true });
  }

  function openMoreMenu(anchor) {
    if (typeof Actions === "undefined" || !Actions.openMenu) return;
    closePopover();
    const rect = anchor.getBoundingClientRect();
    const event = {
      clientX: rect.left,
      clientY: rect.bottom + 4,
      preventDefault() {},
      stopPropagation() {},
    };
    Actions.openMenu(event, [
      { label: L("全部折叠", "Collapse all"), run: () => typeof setAllFolded === "function" && setAllFolded(true) },
      { label: L("全部展开", "Expand all"), run: () => typeof setAllFolded === "function" && setAllFolded(false) },
      { label: L("清理…", "Clean up…"), run: () => Actions.cleanup?.() },
    ]);
  }

  // ---- drag-and-drop project reordering -------------------------------------------------
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
    if (typeof state === "undefined" || !state.heads) return;
    for (const rec of state.heads.values()) {
      rec.el?.classList?.remove("dragging");
      setAttr(rec.el, "data-drop", null);
    }
  }

  function computeReorderedKeys(dragKey, targetKey, pos) {
    if (typeof state === "undefined") return null;
    const sections = ChatGroups.arrange(state.groupsData || [], {
      projects: state.projects || [],
      urgent: AN.urgent,
      L,
      sidebar: state.sidebar,
      prefs: currentPrefs,
      query: "",
    });
    const keys = sections.filter((s) => !s.pinned && !s.other).map((s) => s.key);
    const from = keys.indexOf(dragKey);
    if (from < 0) return null;
    keys.splice(from, 1);
    const to = keys.indexOf(targetKey);
    if (to < 0) return null;
    keys.splice(pos === "after" ? to + 1 : to, 0, dragKey);
    return keys;
  }

  function attachDrag(rec) {
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
        for (const other of state.heads.values()) {
          if (other !== rec) setAttr(other.el, "data-drop", null);
        }
        dragState.pos = pos;
        setAttr(rec.el, "data-drop", pos);
      });
      rec.el.addEventListener("dragleave", (e) => {
        if (!rec.el.contains?.(e.relatedTarget)) setAttr(rec.el, "data-drop", null);
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
        dragState = null;
        clearDropIndicators();
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
    const moreBtn = $("side-more-btn");
    const input = $("side-search-input");
    const clearBtn = $("side-search-clear");
    viewBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      openPopover(viewBtn);
    });
    moreBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      openMoreMenu(moreBtn);
    });
    if (input) bindSearch(input, clearBtn);

    document.addEventListener("pointerdown", (e) => {
      if (!popover || popover.hidden) return;
      if (!popover.contains(e.target) && !$("side-view-btn")?.contains(e.target)) closePopover();
    }, true);
    window.addEventListener("blur", closePopover);
    document.addEventListener("keydown", (e) => {
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
  };
})();
