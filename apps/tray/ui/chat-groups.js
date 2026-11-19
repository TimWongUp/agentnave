"use strict";
// How the sidebar list is grouped: always by project, with pending handovers pinned at top and
// "other" last. Manual projects also appear as empty sections until they gain an agent.
(() => {
  const OTHER = "other";
  const HANDOVER = ":handover";
  // How much of the list is shown before the reader asks for more, and how much one ask adds.
  const CAPS = { groups: 10, groupStep: 10, items: 5, itemStep: 5 };
  const when = (g) => Date.parse(g.updated) || 0;
  const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

  function arrange(groups, { projects = [], urgent, L }) {
    const pending = [];
    const buckets = new Map();
    const manualByKey = new Map();
    for (const project of projects || []) {
      if (!project?.key || project.key === OTHER) continue;
      manualByKey.set(project.key, project);
      if (!buckets.has(project.key)) buckets.set(project.key, []);
    }
    for (const g of groups) {
      if (g.handover === "pending") {
        pending.push(g);
        continue;
      }
      const key = g.project_key || OTHER;
      if (!buckets.has(key)) buckets.set(key, []);
      buckets.get(key).push(g);
    }
    const sections = [...buckets].map(([key, items]) => {
      items.sort((a, b) => (urgent(a.status) ? 0 : 1) - (urgent(b.status) ? 0 : 1) || when(b) - when(a) || byText(a.group, b.group));
      const other = key === OTHER;
      const named = items.find((g) => g.project_label);
      const manual = manualByKey.get(key) || null;
      const latest = items.length ? Math.max(...items.map(when)) : Number(manual?.added_at) || 0;
      return {
        key,
        items,
        other,
        manual: Boolean(manual),
        latest,
        title: other ? L("其他", "Other") : named?.project_label || manual?.label || key,
        path: other ? "" : items.find((g) => g.project_path)?.project_path || manual?.path || "",
      };
    });
    sections.sort((a, b) => Number(a.other) - Number(b.other) || b.latest - a.latest || byText(a.key, b.key));
    // Two places with one name are told apart by where they are.
    const names = new Map();
    for (const s of sections) names.set(s.title, (names.get(s.title) || 0) + 1);
    for (const s of sections) s.hint = names.get(s.title) > 1 ? s.path : "";
    if (pending.length) {
      pending.sort((a, b) => when(b) - when(a) || byText(a.group, b.group));
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

  window.ChatGroups = { arrange, capSections, caps: CAPS, other: OTHER, handover: HANDOVER };
})();
