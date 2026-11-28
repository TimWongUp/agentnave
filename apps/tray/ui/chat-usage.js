"use strict";
window.ChatUsage = (() => {
  const num = (value) => Number(value) || 0;
  const compactFormat = new Intl.NumberFormat("en-US", { notation: "compact", maximumSignificantDigits: 3 });
  const compact = (value) => compactFormat.format(num(value));
  const money = (value) => {
    const v = num(value);
    return v > 0 && v < 0.01 ? "<$0.01" : `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  };
  const unpricedAll = (item) => num(item.total_tokens) > 0 && num(item.unpriced_tokens) >= num(item.total_tokens);
  const costText = (item) => unpricedAll(item) ? "—" : `${num(item.unpriced_tokens) > 0 ? "≈ " : ""}${money(item.cost_usd)}`;
  function pct(rate) {
    if (rate == null || !Number.isFinite(Number(rate))) return "—";
    const p = (Number(rate) > 1 ? Number(rate) / 100 : Number(rate)) * 100;
    if (p > 0 && p < 1) return "<1%";
    return p < 100 ? `${Math.min(99, Math.round(p))}%` : "100%";
  }

  let root, costEl, tokensEl, hitEl, refreshBtn;
  let group = "";
  let data = null;
  let lang = "";
  let seq = 0;

  function icon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 20 20");
    svg.setAttribute("aria-hidden", "true");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "M16.5 10a6.5 6.5 0 1 1-2-4.7M16.5 3.5v3.3h-3.3");
    svg.append(path);
    return svg;
  }
  function hide() {
    if (root) root.hidden = true;
  }
  function paint(item) {
    const L = AN.L;
    lang = document.documentElement.lang;
    costEl.textContent = costText(item.total);
    tokensEl.textContent = compact(item.total.total_tokens);
    const hit = pct(item.total.cache_hit_rate);
    hitEl.textContent = L(`命中 ${hit}`, `${hit} hit`);
    const workers = item.workers || {};
    const meaning = L("花费 · token · 缓存命中率", "cost · tokens · cache hit rate");
    if (num(workers.calls) > 0) {
      const boss = item.boss || {};
      root.title = `${meaning}\nBoss ${costText(boss)} · ${compact(boss.total_tokens)} · ${L("员工", "Employees")} ${costText(workers)} · ${compact(workers.total_tokens)}`;
    } else {
      root.title = meaning;
    }
    refreshBtn.setAttribute("aria-label", L("刷新", "Refresh"));
    refreshBtn.setAttribute("title", L("刷新", "Refresh"));
    root.hidden = false;
  }
  async function load() {
    const wanted = group;
    if (!wanted) return;
    const mine = ++seq;
    try {
      const next = await AN.load(`/app/usage/workspace?group=${encodeURIComponent(wanted)}`);
      if (mine !== seq || group !== wanted) return;
      data = next;
      if (!next?.has_data) hide();
      else paint(next);
    } catch {
      if (mine !== seq) return;
      if (!data) hide();
    }
  }
  function mount() {
    const h = AN.h;
    const L = AN.L;
    costEl = h("span.boss-usage-cost");
    tokensEl = h("span.boss-usage-tokens");
    hitEl = h("span.boss-usage-hit");
    refreshBtn = h("button.boss-usage-refresh", { type: "button", "aria-label": L("刷新", "Refresh"), title: L("刷新", "Refresh") }, icon());
    root = h("div.boss-usage", { hidden: true }, costEl, "·", tokensEl, "·", hitEl, refreshBtn);
    root.addEventListener("click", (e) => e.stopPropagation());
    refreshBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      load();
    });
    group = "";
    data = null;
    seq += 1;
    return root;
  }
  function show(next) {
    if (!root) return;
    if (!next) {
      group = "";
      data = null;
      seq += 1;
      hide();
      return;
    }
    if (next === group) {
      if (data?.has_data && document.documentElement.lang !== lang) paint(data);
      return;
    }
    group = next;
    data = null;
    hide();
    load();
  }
  if (typeof MutationObserver === "function") {
    new MutationObserver(() => {
      if (data && root && !root.hidden) paint(data);
    }).observe(document.documentElement, { attributes: true, attributeFilter: ["lang"] });
  }
  return { mount, show };
})();
