"use strict";
// How the workspace list is grouped. The projection names each workspace's project and boss; the
// order is decided here: groups by their latest activity, "other" last, and inside a group the
// work going on first, then the most recent. A long list is capped the same way for both shapes.
(() => {
  const OTHER = "other";
  const MODES = ["project", "boss", "none"];
  // How much of the list is shown before the reader asks for more, and how much one ask adds.
  const CAPS = { groups: 10, groupStep: 10, items: 5, itemStep: 5, flat: 30, flatStep: 30 };
  const when = (g) => Date.parse(g.updated) || 0;
  const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

  function arrange(groups, mode, { urgent, L }) {
    const keyOf = mode === "boss" ? (g) => g.boss_key || `${g.host}|${g.label}` : (g) => g.project_key || OTHER;
    const buckets = new Map();
    for (const g of groups) {
      const key = keyOf(g);
      if (!buckets.has(key)) buckets.set(key, []);
      buckets.get(key).push(g);
    }
    const sections = [...buckets].map(([key, items]) => {
      items.sort((a, b) => (urgent(a.status) ? 0 : 1) - (urgent(b.status) ? 0 : 1) || when(b) - when(a) || byText(a.group, b.group));
      const other = mode === "project" && key === OTHER;
      const named = items.find((g) => g.project_label);
      return {
        key,
        items,
        other,
        latest: Math.max(...items.map(when)),
        title: other ? L("其他", "Other") : mode === "boss" ? items[0].label : named?.project_label || key,
        path: other || mode === "boss" ? "" : items.find((g) => g.project_path)?.project_path || "",
      };
    });
    sections.sort((a, b) => Number(a.other) - Number(b.other) || b.latest - a.latest || byText(a.key, b.key));
    // Two places with one name are told apart by where they are.
    const names = new Map();
    for (const s of sections) names.set(s.title, (names.get(s.title) || 0) + 1);
    for (const s of sections) s.hint = names.get(s.title) > 1 ? s.path : "";
    return sections;
  }

  // The entries a cap keeps from one list: the first `limit`, plus every entry `must` marks as one
  // the cap may never hide (work waiting on the reader, or the workspace already open).
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
    let hiddenGroups = 0;
    for (const [i, section] of sections.entries()) {
      if (i >= groupLimit && !section.items.some(must)) {
        hiddenGroups++;
        continue;
      }
      const shown = capped(section.items, limitOf(section), must);
      kept.push({ ...section, shown, hidden: section.items.length - shown.length });
    }
    return { sections: kept, hiddenGroups };
  }
  // The same cap for the ungrouped list, which has no groups to keep.
  function capFlat(items, { limit, must }) {
    const shown = capped(items, limit, must);
    return { items: shown, hidden: items.length - shown.length };
  }

  window.ChatGroups = { arrange, capSections, capFlat, caps: CAPS, modes: MODES, other: OTHER };
})();
