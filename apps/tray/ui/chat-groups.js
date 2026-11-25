"use strict";
// How the sidebar list is grouped, filtered, sorted, and searched: by project (with pending
// handovers pinned at top and "other" last) or flat by time. Manual projects also appear as empty
// sections until they gain an agent, unless hidden by preferences or search.
(() => {
  const OTHER = "other";
  const HANDOVER = ":handover";
  // How much of the list is shown before the reader asks for more, and how much one ask adds.
  const CAPS = { groups: 10, groupStep: 10, items: 5, itemStep: 5 };
  const DEFAULT_PREFS = {
    view: "project",
    projectSort: "recent",
    itemSort: "updated",
    urgentFirst: true,
    filter: "all",
    hideEmpty: false,
  };
  const when = (g) => Date.parse(g.updated) || 0;
  const createdAt = (g) => (typeof g.created === "number" ? g.created : Date.parse(g.created)) || when(g);
  const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  const compareNames = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" }) || byText(a, b);

  function normalizeSidebar(raw) {
    const src = raw && typeof raw === "object" ? raw : {};
    const aliases = {};
    if (src.project_aliases && typeof src.project_aliases === "object") {
      for (const [k, v] of Object.entries(src.project_aliases)) {
        if (typeof v === "string" && v.trim()) aliases[k] = v.trim();
      }
    }
    return {
      project_aliases: aliases,
      pinned_projects: Array.isArray(src.pinned_projects) ? [...src.pinned_projects] : [],
      pinned_groups: Array.isArray(src.pinned_groups) ? [...src.pinned_groups] : [],
      project_order: Array.isArray(src.project_order) ? [...src.project_order] : [],
    };
  }

  function normalizePrefs(raw) {
    const src = raw && typeof raw === "object" ? raw : {};
    return {
      view: src.view === "flat" ? "flat" : "project",
      projectSort: ["recent", "name", "manual"].includes(src.projectSort) ? src.projectSort : "recent",
      itemSort: src.itemSort === "created" ? "created" : "updated",
      urgentFirst: typeof src.urgentFirst === "boolean" ? src.urgentFirst : true,
      filter: src.filter === "active" ? "active" : "all",
      hideEmpty: Boolean(src.hideEmpty),
    };
  }

  function isDefaultPrefs(raw) {
    const p = normalizePrefs(raw);
    return Object.keys(DEFAULT_PREFS).every((k) => p[k] === DEFAULT_PREFS[k]);
  }

  function isActiveGroup(g) {
    return g.status === "running" || g.status === "review" || g.handover === "pending" || Boolean(g.live) || Boolean(g.derived_live);
  }

  function parseQuery(query) {
    return String(query || "").trim().toLowerCase().split(/\s+/).filter(Boolean);
  }

  function matchesTerms(fields, terms) {
    if (!terms.length) return true;
    const hay = fields.filter(Boolean).join("\n").toLowerCase();
    return terms.every((t) => hay.includes(t));
  }

  function itemMatches(g, terms, { aliases, manualByKey }) {
    if (!terms.length) return true;
    const key = g.project_key || "";
    const manual = key ? manualByKey.get(key) : null;
    return matchesTerms([
      key && aliases[key],
      g.project_label,
      manual?.label,
      g.project_path,
      manual?.path,
      key !== OTHER ? key : "",
      g.display_title,
      g.title,
      g.derived_titles,
      g.host,
      g.app,
      g.label,
      g.preview,
      g.group,
    ], terms);
  }

  function foldedTone(items) {
    let hasBad = false;
    let hasRun = false;
    for (const g of items || []) {
      if (g.handover === "pending" || g.status === "review") return "review";
      if (g.status === "failed") hasBad = true;
      else if (g.status === "running" || g.live) hasRun = true;
    }
    if (hasBad) return "bad";
    if (hasRun) return "run";
    return "";
  }

  function compareItems(a, b, { urgent, pinnedGroups, prefs }) {
    if (pinnedGroups.size) {
      const pinDiff = (pinnedGroups.has(a.group) ? 0 : 1) - (pinnedGroups.has(b.group) ? 0 : 1);
      if (pinDiff) return pinDiff;
    }
    if (prefs.urgentFirst) {
      const urgDiff = (urgent(a.status) ? 0 : 1) - (urgent(b.status) ? 0 : 1);
      if (urgDiff) return urgDiff;
    }
    if (prefs.itemSort === "created") {
      const createdDiff = createdAt(b) - createdAt(a);
      if (createdDiff) return createdDiff;
    }
    return when(b) - when(a) || byText(a.group, b.group);
  }

  function sortSections(sections, { projectSort, orderMap }) {
    sections.sort((a, b) => {
      const otherDiff = Number(a.other) - Number(b.other);
      if (otherDiff) return otherDiff;
      const pinDiff = Number(Boolean(b.pinnedProject)) - Number(Boolean(a.pinnedProject));
      if (pinDiff) return pinDiff;
      if (projectSort === "name") return compareNames(a.title, b.title) || byText(a.key, b.key);
      if (projectSort === "manual") {
        const ia = orderMap.has(a.key) ? orderMap.get(a.key) : Infinity;
        const ib = orderMap.has(b.key) ? orderMap.get(b.key) : Infinity;
        if (ia !== ib) return ia - ib;
      }
      return b.latest - a.latest || byText(a.key, b.key);
    });
  }

  function buildContext(projects, sidebar, prefs, query) {
    const sb = normalizeSidebar(sidebar);
    const pf = normalizePrefs(prefs);
    const manualByKey = new Map();
    for (const project of projects || []) {
      if (project?.key && project.key !== OTHER) manualByKey.set(project.key, project);
    }
    return {
      sb,
      pf,
      terms: parseQuery(query),
      manualByKey,
      aliases: sb.project_aliases,
      pinnedProjects: new Set(sb.pinned_projects),
      pinnedGroups: new Set(sb.pinned_groups),
      orderMap: new Map(sb.project_order.map((k, i) => [k, i])),
    };
  }

  function arrange(groups, { projects = [], urgent, L, sidebar = null, prefs = null, query = "" }) {
    const ctx = buildContext(projects, sidebar, prefs, query);
    const pending = [];
    const buckets = new Map();
    for (const key of ctx.manualByKey.keys()) buckets.set(key, []);
    for (const g of groups) {
      if (ctx.pf.filter === "active" && !isActiveGroup(g)) continue;
      if (!itemMatches(g, ctx.terms, ctx)) continue;
      if (g.handover === "pending") {
        pending.push(g);
        continue;
      }
      const key = g.project_key || OTHER;
      if (!buckets.has(key)) buckets.set(key, []);
      buckets.get(key).push(g);
    }
    const sections = [];
    for (const [key, items] of buckets) {
      const other = key === OTHER;
      const manual = ctx.manualByKey.get(key) || null;
      const named = items.find((g) => g.project_label);
      const rawTitle = other ? L("其他", "Other") : named?.project_label || manual?.label || key;
      const alias = other ? "" : ctx.aliases[key] || "";
      const title = alias || rawTitle;
      const path = other ? "" : items.find((g) => g.project_path)?.project_path || manual?.path || "";
      if (ctx.pf.hideEmpty && !items.length) continue;
      if (ctx.terms.length && !items.length && !matchesTerms([alias, rawTitle, path, key], ctx.terms)) continue;
      items.sort((a, b) => compareItems(a, b, { urgent, pinnedGroups: ctx.pinnedGroups, prefs: ctx.pf }));
      const latest = items.length ? Math.max(...items.map(when)) : Number(manual?.added_at) || 0;
      const section = {
        key,
        items,
        other,
        manual: Boolean(manual),
        latest,
        title,
        path,
      };
      if (alias) section.rawTitle = rawTitle;
      if (!other && ctx.pinnedProjects.has(key)) section.pinnedProject = true;
      sections.push(section);
    }
    sortSections(sections, { projectSort: ctx.pf.projectSort, orderMap: ctx.orderMap });
    // Two places with one name are told apart by where they are.
    const names = new Map();
    for (const s of sections) names.set(s.title, (names.get(s.title) || 0) + 1);
    for (const s of sections) s.hint = names.get(s.title) > 1 ? s.path : "";
    if (pending.length) {
      pending.sort((a, b) => {
        if (ctx.pinnedGroups.size) {
          const pinDiff = (ctx.pinnedGroups.has(a.group) ? 0 : 1) - (ctx.pinnedGroups.has(b.group) ? 0 : 1);
          if (pinDiff) return pinDiff;
        }
        if (ctx.pf.itemSort === "created") {
          const createdDiff = createdAt(b) - createdAt(a);
          if (createdDiff) return createdDiff;
        }
        return when(b) - when(a) || byText(a.group, b.group);
      });
      sections.unshift({
        key: HANDOVER,
        pinned: true,
        items: pending,
        other: false,
        manual: false,
        latest: Math.max(...pending.map(when)),
        title: L("待接手", "Awaiting handover"),
        path: "",
        hint: "",
      });
    }
    return sections;
  }

  function arrangeFlat(groups, { projects = [], urgent, sidebar = null, prefs = null, query = "" }) {
    const ctx = buildContext(projects, sidebar, prefs, query);
    const items = [];
    for (const g of groups) {
      if (ctx.pf.filter === "active" && !isActiveGroup(g)) continue;
      if (!itemMatches(g, ctx.terms, ctx)) continue;
      items.push(g);
    }
    items.sort((a, b) => {
      const handDiff = (a.handover === "pending" ? 0 : 1) - (b.handover === "pending" ? 0 : 1);
      if (handDiff) return handDiff;
      return compareItems(a, b, { urgent, pinnedGroups: ctx.pinnedGroups, prefs: ctx.pf });
    });
    return items;
  }

  // The entries a cap keeps from one list: the first `limit`, plus every entry `must` marks as one
  // the cap may never hide (work waiting on the reader, or the agent already open).
  function capped(items, limit, must) {
    return items.filter((g, i) => i < limit || must(g));
  }
  // Which groups and entries a long list shows. A group past `groupLimit` is dropped unless it
  // holds an entry the cap may not hide, and then shows only those plus its first `itemLimit`
  // entries; every shown group hides the rest of its entries the same way. `items` stays the whole
  // group so the header can count it, `shown` is what goes into the list. `itemLimit` is a number
  // or a function of the group, because each group remembers how much the reader expanded it.
  function capSections(sections, { groupLimit, itemLimit, must }) {
    const limitOf = (s) => (typeof itemLimit === "function" ? itemLimit(s) : itemLimit);
    const kept = [];
    let seen = 0;
    let hiddenGroups = 0;
    for (const section of sections) {
      if (!section.pinned && seen++ >= groupLimit && !section.items.some(must)) {
        hiddenGroups++;
        continue;
      }
      const shown = capped(section.items, limitOf(section), must);
      kept.push({ ...section, shown, hidden: section.items.length - shown.length });
    }
    return { sections: kept, hiddenGroups };
  }

  window.ChatGroups = {
    arrange,
    arrangeFlat,
    capSections,
    capped,
    foldedTone,
    normalizeSidebar,
    normalizePrefs,
    isDefaultPrefs,
    isActiveGroup,
    defaultPrefs: DEFAULT_PREFS,
    caps: CAPS,
    other: OTHER,
    handover: HANDOVER,
  };
})();
