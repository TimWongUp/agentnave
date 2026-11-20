"use strict";
// Usage page: reads /app/usage and draws it with plain DOM and SVG. Nothing here writes
// styles into markup; widths go through CSSOM, which the page's CSP allows.
{
  const { h, L } = AN;
  const NS = "http://www.w3.org/2000/svg";
  const KEY = "agentnave.usage";
  const DAYS = { today: 1, "7d": 7, "30d": 30 };
  const state = { view: "overall", range: "7d", metric: "cost", data: null, error: false, manual: false, grace: 0, sig: "", open: new Set(), redraw: null, drawnWidth: 0 };
  let timer = 0, seq = 0;

  // ---- formats -----------------------------------------------------------------------
  const num = value => Number(value) || 0;
  const compactFormat = new Intl.NumberFormat("en-US", { notation: "compact", maximumSignificantDigits: 3 });
  const compact = value => compactFormat.format(num(value));
  const count = value => num(value).toLocaleString("en-US");
  const money = value => {
    const v = num(value);
    return v > 0 && v < 0.01 ? "<$0.01" : `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  };
  // A share is a fraction of one; a rate that arrives already as a percentage is taken as is.
  // Never print 0% or 100% for a value that is not exactly there.
  function pct(rate) {
    if (rate == null || !Number.isFinite(Number(rate))) return "—";
    const p = fraction(rate) * 100;
    if (p > 0 && p < 1) return "<1%";
    return p < 100 ? `${Math.min(99, Math.round(p))}%` : "100%";
  }
  const fraction = rate => Number(rate) > 1 ? Number(rate) / 100 : Number(rate);
  const unpricedAll = item => num(item.total_tokens) > 0 && num(item.unpriced_tokens) >= num(item.total_tokens);
  const costText = item => unpricedAll(item) ? "—" : `${num(item.unpriced_tokens) > 0 ? "≈ " : ""}${money(item.cost_usd)}`;
  const writes = item => num(item.cache_write ?? num(item.cache_write_5m) + num(item.cache_write_1h));
  const providerName = provider => provider === "other" ? L("其他", "Other") : AN.cliName(provider);
  const projectName = project => project.key === "other" ? L("其他", "Other") : project.label || project.key;
  const el = (tag, attrs, ...kids) => {
    const node = document.createElementNS(NS, tag);
    for (const [name, value] of Object.entries(attrs || {})) node.setAttribute(name, value);
    node.append(...kids);
    return node;
  };
  const span = (cls, text) => h(`span${cls ? `.${cls}` : ""}`, {}, text);
  const labelled = (label, value) => h("span", {}, `${label} `, h("b", {}, value));
  const dot = provider => h("span.u-dot", { "data-p": provider, "aria-hidden": "true" });
  const fill = (node, fraction) => { node.style.width = `${Math.max(0, Math.min(1, fraction)) * 100}%`; return node; };

  // ---- preferences -------------------------------------------------------------------
  function loadPrefs() {
    try {
      const saved = JSON.parse(localStorage.getItem(KEY) || "{}");
      if (["overall", "projects"].includes(saved.view)) state.view = saved.view;
      if (saved.range in DAYS) state.range = saved.range;
      if (["cost", "tokens"].includes(saved.metric)) state.metric = saved.metric;
    } catch { /* A viewer without storage starts from the defaults. */ }
  }
  function savePrefs() {
    try { localStorage.setItem(KEY, JSON.stringify({ view: state.view, range: state.range, metric: state.metric })); } catch { /* Not worth failing over. */ }
  }

  // ---- loading -----------------------------------------------------------------------
  async function fetchUsage() {
    clearTimeout(timer);
    const mine = ++seq, range = state.range;
    try {
      const data = await AN.load(`/app/usage?range=${range}`);
      if (mine !== seq) return;
      state.data = data;
      state.error = false;
      state.grace = data.scanning ? 0 : Math.max(0, state.grace - 1);
      if (!data.scanning && state.grace === 0) state.manual = false;
    } catch {
      if (mine !== seq) return;
      state.error = true;
      state.manual = false;
    }
    render();
    schedule();
  }
  const hasUsageData = data => Boolean(data && (num(data.overall?.total_tokens) > 0 || (data.providers || []).length > 0));
  const showScanning = () => Boolean(state.data?.scanning && (state.manual || !hasUsageData(state.data)));
  function schedule() {
    clearTimeout(timer);
    if (AN.isHidden()) return;
    const quick = showScanning() || (state.manual && state.grace > 0);
    timer = setTimeout(fetchUsage, quick ? 1500 : 60000);
  }
  async function refresh() {
    const button = document.getElementById("u-refresh");
    if (button.dataset.busy === "true") return;
    state.manual = true;
    button.dataset.busy = "true";
    try {
      await AN.post("/app/usage/refresh");
      state.grace = 4;
    } catch { state.error = true; state.manual = false; }
    await fetchUsage();
  }

  // ---- shared parts ------------------------------------------------------------------
  const providerOrder = data => [...new Set([...(data.providers || []).map(p => p.provider), ...(data.daily || []).flatMap(d => Object.keys(d.by_provider || {}))])];
  const tile = provider => h("span.u-tile", { "aria-hidden": "true" }, AN.hostTile(provider, providerName(provider)));
  const share = (fraction, provider) => h("div.u-sharecell", { "data-p": provider }, h("div.u-share", {}, fill(h("i"), fraction)), span("u-pct", pct(fraction)));
  function expander(row, detail, key) {
    const open = state.open.has(key);
    row.setAttribute("aria-expanded", String(open));
    detail.hidden = !open;
    row.addEventListener("click", () => {
      const next = row.getAttribute("aria-expanded") !== "true";
      row.setAttribute("aria-expanded", String(next));
      detail.hidden = !next;
      state.open[next ? "add" : "delete"](key);
    });
  }
  const empty = text => h("div.u-empty", {}, text);

  // ---- overall view ------------------------------------------------------------------
  function kpiCost(o) {
    const un = num(o.unpriced_tokens), total = num(o.total_tokens);
    const none = unpricedAll(o), calls = num(o.total_calls);
    const sub = [span("", L(`${count(calls)} 次调用`, `${count(calls)} calls`))];
    if (un > 0) sub.push(span("warn", L(`未定价 ${pct(un / total)}`, `${pct(un / total)} unpriced`)));
    return h("section.u-card.u-kpi", {},
      span("u-label", L("折算花费", "Estimated cost")),
      h(`div.u-value${none ? ".none" : ""}`, {}, costText(o)),
      h("div.u-sub", {}, ...sub));
  }
  function kpiTokens(o) {
    return h("section.u-card.u-kpi", {},
      span("u-label", L("Token 用量", "Tokens")),
      h("div.u-value", {}, compact(o.total_tokens)),
      h("div.u-sub", {},
        labelled(L("新输入", "Input"), compact(o.fresh_input)),
        labelled(L("缓存读", "Cache read"), compact(o.cache_read)),
        labelled(L("缓存写", "Cache write"), compact(writes(o))),
        labelled(L("输出", "Output"), compact(o.output))));
  }
  function kpiHit(o) {
    const known = o.cache_hit_rate != null;
    return h("section.u-card.u-kpi", {},
      span("u-label", L("缓存命中率", "Cache hit rate")),
      h(`div.u-value${known ? "" : ".none"}`, {}, pct(o.cache_hit_rate)),
      h("div.u-meter", {}, known ? fill(h("i"), fraction(o.cache_hit_rate)) : null));
  }
  function modelRow(model) {
    return h("li", {}, h("div.u-row", {},
      h("span.u-name", {}, span("", AN.modelName(model.model))),
      h("span", {}),
      h("span.num", {}, model.priced === false ? span("u-tag", L("未定价", "Unpriced")) : money(model.cost_usd)),
      h("span.num", {}, compact(model.total_tokens)),
      h("span.num", {}, pct(model.cache_hit_rate))));
  }
  function providerItem(p, base, key) {
    const metric = base.cost ? num(p.cost_usd) / base.cost : base.tokens ? num(p.total_tokens) / base.tokens : 0;
    const row = h("button.u-row", { type: "button" },
      h("span.u-name", {}, h("i.u-chev"), tile(p.provider), span("", providerName(p.provider))),
      share(metric, p.provider),
      h(`span.num${unpricedAll(p) ? ".u-num-dim" : ""}`, {}, costText(p)),
      h("span.num", {}, compact(p.total_tokens)),
      h("span.num", {}, pct(p.cache_hit_rate)));
    const models = h("ul.u-models", {}, ...(p.models || []).map(modelRow));
    expander(row, models, key);
    return h("li.u-item", {}, row, models);
  }
  function ranking(data) {
    const list = [...(data.providers || [])].sort((a, b) => num(b.cost_usd) - num(a.cost_usd) || num(b.total_tokens) - num(a.total_tokens));
    const base = { cost: list.reduce((sum, p) => sum + num(p.cost_usd), 0), tokens: list.reduce((sum, p) => sum + num(p.total_tokens), 0) };
    return h("section.u-card", {},
      h("div.u-head", {}, h("h2", {}, L("按 CLI", "By CLI"))),
      h("div.u-cols", {}, span("", ""), span("", ""), span("", L("折算花费", "Cost")), span("", "Token"), span("", L("命中率", "Hit rate"))),
      h("ul.u-list", {}, ...list.map(p => providerItem(p, base, `c:${p.provider}`))));
  }
  function overall(data) {
    const o = data.overall || {};
    if (!num(o.total_tokens) && !num(o.total_calls)) {
      return [empty(data.scanning ? L("正在统计，请稍候", "Counting, please wait") : L("这段时间没有用量记录", "No usage in this period"))];
    }
    return [h("div.u-kpis", {}, kpiCost(o), kpiTokens(o), kpiHit(o)), trendCard(data), ranking(data)];
  }

  // ---- daily trend -------------------------------------------------------------------
  const pad = value => String(value).padStart(2, "0");
  function dayList(range, daily) {
    const now = new Date(), today = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
    const last = [today, ...daily.map(d => String(d.date).slice(0, 10))].sort().at(-1);
    const [y, m, d] = last.split("-").map(Number), n = DAYS[range] || 7;
    return Array.from({ length: n }, (_, i) => new Date(Date.UTC(y, m - 1, d - (n - 1 - i))).toISOString().slice(0, 10));
  }
  function niceMax(value) {
    if (value <= 0) return 1;
    const unit = 10 ** Math.floor(Math.log10(value)), n = value / unit;
    return unit * (n <= 1 ? 1 : n <= 2 ? 2 : n <= 2.5 ? 2.5 : n <= 5 ? 5 : 10);
  }
  const dayText = date => {
    const [, m, d] = date.split("-").map(Number);
    return L(`${m}/${d}`, new Date(Date.UTC(2000, m - 1, d)).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" }));
  };
  const metricKey = () => state.metric === "cost" ? "cost_usd" : "total_tokens";
  const axisText = value => state.metric === "cost" ? `$${compact(value)}` : compact(value);
  function tipRows(day, order) {
    const rows = [h("b", {}, dayText(day.date)),
      h("div", {}, span("", L("折算花费", "Cost")), span("", costText(day))),
      h("div", {}, span("", "Token"), span("", compact(day.total_tokens))),
      h("div", {}, span("", L("缓存命中率", "Cache hit rate")), span("", pct(day.cache_hit_rate)))];
    const parts = order.filter(p => num(day.by_provider?.[p]?.[metricKey()]) > 0);
    if (parts.length > 1) {
      for (const p of parts) {
        const value = day.by_provider[p][metricKey()];
        rows.push(h("div", {}, h("span", {}, dot(p), providerName(p)), span("", state.metric === "cost" ? money(value) : compact(value))));
      }
    }
    return rows;
  }
  function drawChart(box, tip, data) {
    const width = Math.max(320, box.clientWidth), height = 210, left = 46, top = 8, bottom = 24;
    state.drawnWidth = box.clientWidth;
    const order = providerOrder(data), known = new Map((data.daily || []).map(d => [String(d.date).slice(0, 10), d]));
    const days = dayList(data.range, data.daily || []).map(date => ({ date, ...(known.get(date) || {}), empty: !known.has(date) }));
    const key = metricKey();
    const stack = day => order.map(p => [p, num(day.by_provider?.[p]?.[key])]).filter(([, v]) => v > 0);
    const totalOf = day => stack(day).reduce((sum, [, v]) => sum + v, 0) || num(day[key]);
    const max = niceMax(Math.max(0, ...days.map(totalOf)));
    const plot = height - top - bottom, band = (width - left) / days.length, barW = Math.min(band * 0.6, 44);
    const y = value => top + plot * (1 - value / max);
    const svg = el("svg", { viewBox: `0 0 ${width} ${height}`, width, height, role: "img", "aria-label": L("每日趋势", "Daily trend") });
    for (const value of [0, max / 2, max]) {
      svg.append(el("line", { class: "u-grid", x1: left, x2: width, y1: y(value), y2: y(value) }));
      const text = el("text", { class: "u-tick", x: left - 8, y: y(value) + 4, "text-anchor": "end" });
      text.textContent = axisText(value);
      svg.append(text);
    }
    const every = Math.max(1, Math.ceil(days.length / Math.floor((width - left) / 60)));
    days.forEach((day, i) => {
      const cx = left + band * (i + 0.5);
      let base = y(0);
      const parts = stack(day);
      for (const [p, v] of parts.length ? parts : [["", totalOf(day)]]) {
        const bar = Math.max(v > 0 ? 1 : 0, plot * v / max);
        base -= bar;
        if (bar > 0) svg.append(el("rect", { class: "u-bar", "data-p": p, x: cx - barW / 2, y: base, width: barW, height: bar }));
      }
      if (i % every === 0 || days.length === 1) {
        const text = el("text", { class: "u-tick", x: cx, y: height - 6, "text-anchor": "middle" });
        text.textContent = dayText(day.date);
        svg.append(text);
      }
      const column = el("rect", { class: "u-col", x: left + band * i, y: top, width: band, height: plot });
      const show = () => {
        tip.replaceChildren(...tipRows(day, order));
        tip.hidden = false;
        const room = box.clientWidth - tip.offsetWidth - 4;
        tip.style.left = `${Math.max(0, Math.min(cx + 12, room))}px`;
        tip.style.top = `${top}px`;
        column.classList.add("on");
      };
      column.addEventListener("pointerenter", show);
      column.addEventListener("pointerdown", show);
      column.addEventListener("pointerleave", () => { tip.hidden = true; column.classList.remove("on"); });
      svg.append(column);
    });
    box.replaceChildren(svg, tip);
  }
  function metricSwitch(onPick) {
    const group = h("div.u-seg.small", { role: "radiogroup", "aria-label": L("指标", "Metric") });
    for (const [value, label] of [["cost", L("花费", "Cost")], ["tokens", "Token"]]) {
      const button = h("button", { type: "button", role: "radio", "aria-checked": String(state.metric === value) }, label);
      button.addEventListener("click", () => {
        state.metric = value;
        savePrefs();
        group.querySelectorAll("button").forEach(item => item.setAttribute("aria-checked", String(item === button)));
        onPick();
      });
      group.append(button);
    }
    return group;
  }
  function trendCard(data) {
    const box = h("div.u-chart"), tip = h("div.u-tip", { hidden: true, role: "status" });
    const draw = () => drawChart(box, tip, data);
    state.redraw = draw;
    queueMicrotask(draw);
    const legend = h("div.u-legend", {}, ...providerOrder(data).map(p => h("span", {}, dot(p), providerName(p))));
    return h("section.u-card", {}, h("div.u-head", {}, h("h2", {}, L("每日趋势", "Daily trend")), metricSwitch(draw)), box, legend);
  }

  // ---- projects view -----------------------------------------------------------------
  function group(label, stats, marker) {
    return h("div.u-group", {},
      h("span.u-label", {}, marker ? h(`span.u-dot.${marker}`) : null, label),
      h(`div.cost${unpricedAll(stats) ? ".u-num-dim" : ""}`, {}, costText(stats)),
      h("div.u-sub", {}, labelled("Token", compact(stats.total_tokens)), labelled(L("命中", "Hit"), pct(stats.cache_hit_rate))));
  }
  function splitBar(boss, worker) {
    const sum = num(boss) + num(worker);
    const part = (cls, value) => {
      const node = h(`i.${cls}`);
      node.style.flex = `${num(value)} 1 0px`;
      return node;
    };
    return h("div.u-split", { title: `Boss ${pct(sum ? num(boss) / sum : 0)} · ${L("员工", "Employees")} ${pct(sum ? num(worker) / sum : 0)}` },
      part("boss", boss), part("worker", worker));
  }
  function projectDetail(project) {
    const head = h("div.u-prow.head", {}, span("", ""), span("", ""), span("", L("折算花费", "Cost")), span("", "Token"), span("", "Boss"), span("", L("员工", "Employees")));
    const rows = [...(project.by_provider || [])].sort((a, b) => num(b.cost_usd) - num(a.cost_usd) || num(b.total_tokens) - num(a.total_tokens)).map(p =>
      h("div.u-prow", {}, h("span.u-name", {}, tile(p.provider), span("", providerName(p.provider))),
        splitBar(p.boss_tokens, p.worker_tokens),
        h("span.num", {}, costText({ ...p, unpriced_tokens: 0 })), h("span.num", {}, compact(p.total_tokens)),
        h("span.num", {}, compact(p.boss_tokens)), h("span.num", {}, compact(p.worker_tokens))));
    return h("div.u-pdetail", {}, head, ...rows);
  }
  function projectCard(project) {
    const total = project.total || {}, boss = project.boss || {}, workers = project.workers || {};
    const title = h("div.u-ptitle", {}, h("b", {}, projectName(project)),
      project.key !== "other" && project.path ? h("span.u-path", { title: project.path }, project.path) : null);
    const button = h("button.u-pbtn", { type: "button" }, title,
      h("div.u-groups", {}, group(L("合计", "Total"), total), group("Boss", boss, "boss"), group(L("员工", "Employees"), workers, "worker")),
      splitBar(boss.total_tokens, workers.total_tokens));
    const detail = projectDetail(project);
    expander(button, detail, `p:${project.key}`);
    return h("section.u-card.u-project", {}, button, detail);
  }
  function projects(data) {
    const list = [...(data.projects || [])].sort((a, b) => num(b.total?.cost_usd) - num(a.total?.cost_usd) || num(b.total?.total_tokens) - num(a.total?.total_tokens));
    if (!list.length) return [empty(data.scanning ? L("正在首次扫描，请稍候", "Scanning for the first time, please wait") : L("还没有项目用量记录", "No project usage yet"))];
    return list.map(projectCard);
  }

  // ---- page --------------------------------------------------------------------------
  function renderSync() {
    const data = state.data, label = document.getElementById("u-updated"), button = document.getElementById("u-refresh");
    const busy = showScanning() || (state.manual && state.grace > 0);
    button.dataset.busy = String(busy);
    label.classList.toggle("error", state.error);
    if (state.error) label.textContent = L("读取失败，稍后重试", "Couldn't read usage, retrying");
    else if (busy) label.textContent = L("扫描中…", "Scanning…");
    else if (data?.updated_at) label.textContent = L(`更新于 ${AN.relTime(new Date(data.updated_at).toISOString())}`, `Updated ${AN.relTime(new Date(data.updated_at).toISOString())}`);
    else label.textContent = "";
  }
  function renderFoot() {
    const foot = document.getElementById("u-foot"), names = (state.data?.uncovered_installed || []).map(providerName);
    foot.hidden = !names.length;
    foot.textContent = names.length ? L(`暂未统计：${names.join("、")}`, `Not tracked yet: ${names.join(", ")}`) : "";
  }
  function renderBody() {
    const data = state.data, body = document.getElementById("u-body");
    const sig = `${state.view}|${state.range}|${document.documentElement.lang}|${JSON.stringify(data)}`;
    if (sig === state.sig) return;
    state.sig = sig;
    state.redraw = null;
    if (!data) return body.replaceChildren(empty(state.error ? L("暂时读不到用量，稍后会自动重试", "Usage is unavailable, retrying shortly") : ""));
    body.replaceChildren(...(state.view === "projects" ? projects(data) : overall(data)));
  }
  function render() {
    for (const button of document.querySelectorAll("#u-view button")) button.setAttribute("aria-checked", String(button.dataset.view === state.view));
    for (const button of document.querySelectorAll("#u-range button")) button.setAttribute("aria-checked", String(button.dataset.range === state.range));
    renderSync();
    renderBody();
    renderFoot();
  }
  function pick(next) {
    Object.assign(state, next);
    savePrefs();
    if ("range" in next) { state.data = null; state.sig = ""; }
    render();
    if ("range" in next) fetchUsage();
  }

  loadPrefs();
  document.getElementById("u-view").addEventListener("click", event => {
    const button = event.target.closest("button[data-view]");
    if (button && button.dataset.view !== state.view) pick({ view: button.dataset.view });
  });
  document.getElementById("u-range").addEventListener("click", event => {
    const button = event.target.closest("button[data-range]");
    if (button && button.dataset.range !== state.range) pick({ range: button.dataset.range });
  });
  document.getElementById("u-refresh").addEventListener("click", refresh);
  // The office hands over the interface language when this page was loaded in another one.
  window.addEventListener("message", event => {
    if (window.parent !== window && event.source === window.parent && event.data?.type === "agentnave:settings" && event.data.action === "language") {
      AN.setLanguage(event.data.language);
      state.sig = "";
      render();
    }
  });
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      const box = document.querySelector(".u-chart");
      if (box && state.redraw && box.clientWidth !== state.drawnWidth) state.redraw();
    }).observe(document.getElementById("u-body"));
  }
  AN.onVisibility(() => { if (AN.isHidden()) clearTimeout(timer); else fetchUsage(); });
  render();
  fetchUsage();
}
