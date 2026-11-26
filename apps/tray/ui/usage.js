"use strict";
// Usage page controller: fetches /app/usage, coordinates KPIs, trend chart, ranking, donut, composition, and projects.
// Zero inline style attributes; all dynamic values use CSSOM properties or classes.
{
  const { h, L } = AN;
  const NS = "http://www.w3.org/2000/svg";
  const KEY = "agentnave.usage";
  const DAY_MS = 86400000;
  const PRESET_RANGES = new Set(["24h", "today", "7d", "30d"]);
  const RANGE_DAYS = { "24h": 1, today: 1, "7d": 7, "30d": 30, "90d": 90 };
  const KNOWN_PROVIDERS = new Set([
    "claude", "claude-desktop", "codex", "antigravity", "codebuddy", "workbuddy",
    "grok", "deepseek", "pi", "zcode", "hermes", "opencode", "kimi", "minimax", "devin",
  ]);
  const KNOWN_FAMILIES = new Set([
    "claude", "gemini", "gpt", "grok", "deepseek", "glm", "kimi",
    "minimax", "qwen", "hunyuan", "doubao", "llama", "mistral", "unknown",
  ]);

  const state = {
    view: "overall", range: "7d", presetRange: "7d", custom: null, granularity: null,
    dim: "provider", metric: "cost", cache: "with", mode: "total", shape: "line",
    sort: { key: "value", dir: -1 }, open: new Set(), openProjects: new Set(),
    pinned: new Set(), hover: null, cursor: null, back: null, legendAll: false,
    data: null, loadedReqKey: "", error: false, manual: false, grace: 0,
    sig: "", redraw: null, drawnWidth: 0, picker: null,
  };
  let timer = 0, seq = 0;

  // ---- formats & helpers -------------------------------------------------------------
  const num = v => Number(v) || 0;
  const pad = v => String(v).padStart(2, "0");
  const trim = s => s.replace(/\.0+$/, "").replace(/(\.\d*[1-9])0+$/, "$1");
  const compactFormat = new Intl.NumberFormat("en-US", { notation: "compact", maximumSignificantDigits: 3 });
  const compact = v => compactFormat.format(num(v));
  const count = v => Math.round(num(v)).toLocaleString("en-US");
  const money = v => {
    const n = num(v);
    return n > 0 && n < 0.01 ? "<$0.01" : `$${n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  };
  const fraction = r => Number(r) > 1 ? Number(r) / 100 : Number(r);
  function pct(rate) {
    if (rate == null || !Number.isFinite(Number(rate))) return "—";
    const p = fraction(rate) * 100;
    if (p > 0 && p < 1) return "<1%";
    return p < 100 ? `${Math.min(99, Math.round(p))}%` : "100%";
  }
  const hitExact = rate => rate == null || !Number.isFinite(Number(rate)) ? "—" : `${(fraction(rate) * 100).toFixed(2)}%`;
  const unpricedAll = item => num(item?.total_tokens) > 0 && num(item?.unpriced_tokens) >= num(item?.total_tokens);
  const costText = item => unpricedAll(item) ? "—" : `${num(item?.unpriced_tokens) > 0 ? "≈ " : ""}${money(item?.cost_usd)}`;
  const writes = item => num(item?.cache_write ?? num(item?.cache_write_5m) + num(item?.cache_write_1h));
  const netTokens = item => item?.net_tokens != null ? num(item.net_tokens) : Math.max(0, num(item?.total_tokens) - num(item?.cache_read));
  const isNet = () => state.metric === "tokens" && state.cache === "net";
  const tokValue = item => isNet() ? netTokens(item) : num(item?.total_tokens);
  const metricValue = item => state.metric === "cost" ? num(item?.cost_usd) : tokValue(item);
  const fmtValue = v => state.metric === "cost" ? money(v) : compact(v);
  const fmtAxis = v => state.metric === "cost" ? (v > 0 && v < 0.01 ? "<$0.01" : `$${trim(num(v).toFixed(2))}`) : compact(v);
  const pricePerM = item => (!item || item.priced === false || unpricedAll(item) || num(item.total_tokens) <= 0)
    ? null : (num(item.cost_usd) / num(item.total_tokens)) * 1e6;
  const fmtPrice = p => p == null ? "—" : `$${p.toFixed(2)}`;
  const change = (cur, prev) => prev != null && num(prev) > 0 ? num(cur) / num(prev) - 1 : null;
  const providerName = p => p === "other" ? L("其他", "Other") : AN.cliName(p);
  const modelLabel = m => (m && String(m).trim()) ? AN.modelName(m) : L("未知模型", "Unknown model");
  const projectName = p => p.key === "other" ? L("其他", "Other") : p.label || p.key;
  function baseModelKey(raw) {
    let s = String(raw || "").trim();
    if (!s) return "";
    if (s.includes("/")) s = s.split("/").pop() || s;
    s = s.replace(/(?:\[[^\]]+\]|@\d+[km]?)$/i, "").trim().toLowerCase();
    let prev;
    do {
      prev = s;
      s = s.replace(/-20\d{6}$/, "")
        .replace(/-tiered-(?:xhigh|high|medium|low|minimal|none)$/, "")
        .replace(/-(?:xhigh|high|medium|low|minimal|none|thinking)$/, "");
    } while (s !== prev);
    return s;
  }
  function modelFamilyId(base) {
    if (!base) return "unknown";
    if (/^claude/.test(base)) return "claude";
    if (/^gemini/.test(base)) return "gemini";
    if (/^(?:gpt|o[134](?:-|$)|codex|chatgpt)/.test(base)) return "gpt";
    if (/^grok/.test(base)) return "grok";
    if (/^deepseek/.test(base)) return "deepseek";
    if (/^(?:glm|chatglm)/.test(base)) return "glm";
    if (/^(?:kimi|moonshot)/.test(base)) return "kimi";
    if (/^(?:minimax|abab)/.test(base)) return "minimax";
    if (/^(?:qwen|qwq)/.test(base)) return "qwen";
    if (/^(?:hy\d|hunyuan)/.test(base)) return "hunyuan";
    if (/^(?:doubao|seed-)/.test(base)) return "doubao";
    if (/^llama/.test(base)) return "llama";
    if (/^(?:mistral|mixtral|codestral|devstral)/.test(base)) return "mistral";
    return "other";
  }
  const familyName = id => ({
    claude: "Claude", gemini: "Gemini", gpt: "GPT", grok: "Grok",
    deepseek: "DeepSeek", glm: "GLM", kimi: "Kimi", minimax: "MiniMax",
    qwen: "Qwen", hunyuan: L("混元", "Hunyuan"), doubao: L("豆包", "Doubao"),
    llama: "Llama", mistral: "Mistral", unknown: L("未知模型", "Unknown model"),
    other: L("其他", "Other"),
  }[id] || id);

  const svgEl = (tag, attrs, ...kids) => {
    const node = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs || {})) if (v != null) node.setAttribute(k, String(v));
    node.append(...kids);
    return node;
  };
  const chevronSvg = () => svgEl("svg", { width: 8, height: 8, viewBox: "0 0 8 8", "aria-hidden": "true" },
    svgEl("path", { d: "M2 .8 6 4 2 7.2", fill: "none", stroke: "currentColor", "stroke-width": "1.5", "stroke-linecap": "round", "stroke-linejoin": "round" }));
  const lineIconSvg = () => svgEl("svg", { width: 15, height: 13, viewBox: "0 0 15 13", "aria-hidden": "true" },
    svgEl("path", { d: "M1 10C3.5 10 4 4 6.5 4S9 8 11 8s2-5 3-5.5", fill: "none", stroke: "currentColor", "stroke-width": "1.6", "stroke-linecap": "round" }));
  const barIconSvg = () => svgEl("svg", { width: 15, height: 13, viewBox: "0 0 15 13", "aria-hidden": "true" },
    svgEl("path", { d: "M1.5 12V7.5a1.3 1.3 0 0 1 2.6 0V12zM6.2 12V2.3a1.3 1.3 0 0 1 2.6 0V12zM10.9 12V5.5a1.3 1.3 0 0 1 2.6 0V12z", fill: "currentColor" }));

  const span = (cls, text, title) => h(`span${cls ? `.${cls}` : ""}`, title ? { title } : {}, text);
  const tile = p => h("span.u-tile", { "aria-hidden": "true" }, AN.hostTile(p, providerName(p)));
  const fill = (node, f) => { node.style.width = `${Math.max(0, Math.min(1, f)) * 100}%`; return node; };
  const flex = (node, v) => { node.style.flex = `${Math.max(0, num(v))} 1 0px`; return node; };
  const empty = text => h("div.u-empty", {}, text);
  const metricLabel = () => state.metric === "cost" ? L("折算花费", "Cost") : isNet() ? L("净 Token", "Net tokens") : "Token";
  const dataAttr = (node, key) => node?.dataset?.[key] ?? node?.getAttribute?.(`data-${key}`);
  const toneProps = (p, shade = 0, ci = null, family = null) => ({
    ...(family ? { "data-f": family } : p ? { "data-p": p } : {}),
    ...(ci != null ? { "data-ci": String(ci) } : {}),
    ...(shade ? { "data-shade": String(shade) } : {}),
  });
  const applyTone = (node, item) => {
    if (item.family) node.setAttribute("data-f", item.family);
    else if (item.provider) node.setAttribute("data-p", item.provider);
    if (item.ci != null) node.setAttribute("data-ci", String(item.ci));
    if (item.shade) node.setAttribute("data-shade", String(item.shade));
    return node;
  };
  const arrowNode = (d, tone) => h(`i.u-arrow.${tone === "neutral" ? "neutral" : ((d >= 0) === (tone === "lowerBetter")) ? "up" : "down"}`, {}, d >= 0 ? "↑" : "↓");
  const deltaNodes = (d, tone) => d == null ? ["—"] : (Math.round(d * 100) ? [arrowNode(d, tone), `${Math.abs(Math.round(d * 100))}%`] : ["0%"]);
  const metricTone = () => state.metric === "cost" ? "lowerBetter" : "neutral";

  // ---- preferences & loading ---------------------------------------------------------
  function loadPrefs() {
    try {
      const s = JSON.parse(localStorage.getItem(KEY) || "{}");
      if (["overall", "projects"].includes(s.view)) state.view = s.view;
      if (PRESET_RANGES.has(s.range)) state.range = state.presetRange = s.range;
      if (["provider", "family", "model"].includes(s.dim)) state.dim = s.dim;
      if (["cost", "tokens"].includes(s.metric)) state.metric = s.metric;
      if (["with", "net"].includes(s.cache)) state.cache = s.cache;
      if (["total", "series"].includes(s.mode)) state.mode = s.mode;
      if (["line", "bar"].includes(s.shape)) state.shape = s.shape;
    } catch { /* Defaults apply without storage. */ }
  }
  function savePrefs() {
    try {
      const range = PRESET_RANGES.has(state.range) ? state.range : state.presetRange;
      localStorage.setItem(KEY, JSON.stringify({ view: state.view, range, dim: state.dim, metric: state.metric, cache: state.cache, mode: state.mode, shape: state.shape }));
    } catch { /* Ignore storage quota errors. */ }
  }
  function requestPath() {
    let base = `/app/usage?range=${state.range}`;
    if (state.range === "custom") {
      base = state.custom?.kind === "preset90d" ? "/app/usage?range=90d"
        : state.custom?.fromMs != null && state.custom?.toMs != null
          ? `/app/usage?range=custom&from=${Math.round(state.custom.fromMs)}&to=${Math.round(state.custom.toMs)}`
          : `/app/usage?range=${state.presetRange}`;
    }
    return state.granularity ? `${base}&granularity=${state.granularity}` : base;
  }
  const switching = () => Boolean(state.data && state.loadedReqKey && state.loadedReqKey !== requestPath());

  async function fetchUsage() {
    clearTimeout(timer);
    const mine = ++seq, path = requestPath();
    try {
      const data = await AN.load(path);
      if (mine !== seq) return;
      state.data = data; state.loadedReqKey = path; state.error = false;
      state.grace = data.scanning ? 0 : Math.max(0, state.grace - 1);
      if (!data.scanning && state.grace === 0) state.manual = false;
    } catch {
      if (mine !== seq) return;
      if (switching()) state.data = null;
      state.error = true; state.manual = false;
    }
    render();
    schedule();
  }
  const hasUsageData = d => Boolean(d && (num(d.overall?.total_tokens) > 0 || (d.providers || []).length > 0));
  const showScanning = () => Boolean(state.data?.scanning && (state.manual || !hasUsageData(state.data)));
  function schedule() {
    clearTimeout(timer);
    if (AN.isHidden()) return;
    timer = setTimeout(fetchUsage, (showScanning() || (state.manual && state.grace > 0)) ? 1500 : 60000);
  }
  async function refresh() {
    const btn = document.getElementById("u-refresh");
    if (btn.dataset?.busy === "true" || btn.getAttribute?.("data-busy") === "true") return;
    state.manual = true;
    btn.setAttribute("data-busy", "true");
    if (btn.dataset) btn.dataset.busy = "true";
    try { await AN.post("/app/usage/refresh"); state.grace = 4; }
    catch { state.error = true; state.manual = false; }
    await fetchUsage();
  }

  // ---- bucket & series normalization -------------------------------------------------
  const effectiveGrain = d => d?.granularity || state.granularity || (d?.range === "today" || d?.range === "24h" ? "hour" : "day");
  const weekdayShort = dt => [L("周日", "Sun"), L("周一", "Mon"), L("周二", "Tue"), L("周三", "Wed"), L("周四", "Thu"), L("周五", "Fri"), L("周六", "Sat")][dt.getDay()];
  function dayPrefix(dt, nowMs) {
    const nowDt = new Date(nowMs);
    const diff = Math.round((Date.UTC(nowDt.getFullYear(), nowDt.getMonth(), nowDt.getDate()) - Date.UTC(dt.getFullYear(), dt.getMonth(), dt.getDate())) / DAY_MS);
    return diff === 0 ? L("今天", "Today") : diff === 1 ? L("昨天", "Yesterday") : `${pad(dt.getMonth() + 1)}/${pad(dt.getDate())}`;
  }
  function normalizeBuckets(data) {
    const grain = effectiveGrain(data), daily = data.daily || [], nowMs = num(data.now_ms) || Date.now();
    if (daily.length > 0 && daily[0].start_ms != null) {
      const singleDay = grain === "hour" && new Date(daily[0].start_ms).toDateString() === new Date(daily.at(-1).start_ms).toDateString();
      const buckets = daily.map(b => {
        const dt = new Date(num(b.start_ms)), endDt = new Date(Math.max(num(b.start_ms), num(b.end_ms) - 1)), hr = dt.getHours();
        const mdStr = `${pad(dt.getMonth() + 1)}/${pad(dt.getDate())}`;
        let axis = mdStr, label = `${mdStr} ${weekdayShort(dt)}`;
        if (grain === "hour") {
          const time = `${pad(hr)}:00`;
          axis = !singleDay && hr === 0 ? (dayPrefix(dt, nowMs) === L("今天", "Today") ? L("今日 00:00", "Today 00:00") : mdStr) : time;
          label = `${singleDay ? "" : `${dayPrefix(dt, nowMs)} `}${time}–${pad(hr + 1)}:00`;
        } else if (grain === "week") {
          const endMd = `${pad(endDt.getMonth() + 1)}/${pad(endDt.getDate())}`;
          label = mdStr === endMd ? mdStr : `${mdStr}–${endMd}`;
        }
        return { ...b, has: !b.future, hour: hr, axis, label };
      });
      const firstFuture = buckets.findIndex(b => b.future);
      const nowIdx = grain === "hour" && firstFuture > 0 ? firstFuture - 1 : null;
      const midnightIdx = grain === "hour" && nowIdx == null ? (() => { const i = buckets.findIndex((b, idx) => idx > 0 && b.hour === 0); return i > 0 ? i : null; })() : null;
      return { grain, buckets, nowIdx, midnightIdx };
    }
    const hourly = grain === "hour", known = new Map(daily.map(d => [hourly ? String(d.date).slice(0, 5) : String(d.date).slice(0, 10), d]));
    let slots;
    if (hourly) slots = Array.from({ length: 24 }, (_, hr) => `${pad(hr)}:00`);
    else {
      const now = new Date(nowMs), todayStr = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
      const [y, m, d] = [todayStr, ...daily.map(x => String(x.date).slice(0, 10))].sort().at(-1).split("-").map(Number);
      const n = RANGE_DAYS[data.range] || 7;
      slots = Array.from({ length: n }, (_, i) => new Date(Date.UTC(y, m - 1, d - (n - 1 - i))).toISOString().slice(0, 10));
    }
    const buckets = slots.map((slot, idx) => {
      const raw = known.get(slot) || {};
      const axis = hourly ? slot : `${pad(Number(slot.split("-")[1]))}/${pad(Number(slot.split("-")[2]))}`;
      const label = hourly ? (Number.isFinite(Number(slot.slice(0, 2))) ? `${pad(Number(slot.slice(0, 2)))}:00–${pad(Number(slot.slice(0, 2)) + 1)}:00` : slot)
        : L(`${Number(slot.split("-")[1])}/${Number(slot.split("-")[2])}`, new Date(Date.UTC(2000, Number(slot.split("-")[1]) - 1, Number(slot.split("-")[2]))).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" }));
      return { date: slot, ...raw, has: !raw.future, hour: hourly ? idx % 24 : null, axis, label };
    });
    return { grain, buckets, nowIdx: null, midnightIdx: null };
  }

  function mergeAgg(entries) {
    let cost = 0, unpriced = 0, total = 0, net = 0, calls = 0, kp = 0, kr = 0, wHit = 0, wTok = 0;
    let allUnpriced = entries.length > 0;
    for (const e of entries) {
      cost += num(e.cost_usd);
      const t = num(e.total_tokens);
      const u = e.unpriced_tokens != null ? num(e.unpriced_tokens) : (e.priced === false ? t : 0);
      unpriced += u; total += t; net += netTokens(e); calls += num(e.calls);
      if (e.priced !== false && (t === 0 || u < t)) allUnpriced = false;
      if (num(e.cache_known_prompt) > 0) { kp += num(e.cache_known_prompt); kr += num(e.cache_known_read); }
      else if (e.cache_hit_rate != null && Number.isFinite(Number(e.cache_hit_rate)) && t > 0) { wHit += fraction(e.cache_hit_rate) * t; wTok += t; }
    }
    return {
      cost_usd: cost, unpriced_tokens: unpriced, total_tokens: total, net_tokens: net, calls,
      priced: !allUnpriced,
      cache_hit_rate: kp > 0 ? kr / kp : (wTok > 0 ? wHit / wTok : null),
    };
  }

  function prevGroupMap(data, keyOf) {
    const out = new Map();
    const add = (k, c, t, n) => {
      const g = out.get(k) || { cost_usd: 0, total_tokens: 0, net_tokens: 0 };
      g.cost_usd += num(c); g.total_tokens += num(t); g.net_tokens += num(n);
      out.set(k, g);
    };
    if (Array.isArray(data.prev_by_model)) {
      for (const e of data.prev_by_model) add(keyOf(e.model), e.cost_usd, e.total_tokens, e.net_tokens);
      return out;
    }
    let hasAny = false;
    for (const p of data.providers || []) for (const m of p.models || []) {
      if (m.prev_cost_usd != null || m.prev_total_tokens != null || m.prev_net_tokens != null) {
        hasAny = true;
        add(keyOf(m.model), m.prev_cost_usd, m.prev_total_tokens, m.prev_net_tokens ?? m.prev_total_tokens);
      }
    }
    return hasAny ? out : null;
  }

  function collectModelRows(data, buckets) {
    const byKey = new Map();
    for (const p of data.providers || []) for (const m of p.models || []) {
      const prov = m.provider || p.provider, key = m.key || `${prov}::${m.model}`;
      byKey.set(key, { ...m, provider: prov, key, base: baseModelKey(m.model) });
    }
    for (const b of buckets) for (const [key, m] of Object.entries(b.by_model || {})) {
      if (!byKey.has(key)) {
        const model = m.model ?? key.split("::")[1] ?? "";
        byKey.set(key, { key, provider: m.provider || key.split("::")[0], model, base: baseModelKey(model) });
      }
    }
    return [...byKey.values()].map(r => ({ ...r, family: modelFamilyId(r.base) }));
  }

  function buildGroupedModels(data, buckets, mVal, famColorIdx) {
    const rows = collectModelRows(data, buckets), isFam = state.dim === "family";
    const groupKey = r => isFam ? r.family : (r.base || "__unknown__");
    const prevMap = prevGroupMap(data, m => isFam ? modelFamilyId(baseModelKey(m)) : (baseModelKey(m) || "__unknown__"));
    const prevVal = k => !prevMap ? null : (state.metric === "cost" ? num(prevMap.get(k)?.cost_usd) : (isNet() ? num(prevMap.get(k)?.net_tokens) : num(prevMap.get(k)?.total_tokens)));
    const groups = new Map();
    for (const r of rows) {
      const k = groupKey(r);
      if (!groups.has(k)) groups.set(k, { id: k, base: r.base, family: r.family, entries: [] });
      groups.get(k).entries.push(r);
    }
    const bucketSums = buckets.map(b => {
      const map = new Map();
      for (const [key, m] of Object.entries(b.by_model || {})) {
        const base = baseModelKey(m.model ?? key.split("::")[1] ?? "");
        const gk = isFam ? modelFamilyId(base) : (base || "__unknown__");
        map.set(gk, (map.get(gk) || 0) + mVal(m));
      }
      return map;
    });
    const items = [...groups.values()].map(g => {
      const raw = mergeAgg(g.entries), values = bucketSums.map(bm => bm.get(g.id) || 0);
      const value = g.entries.some(e => e.total_tokens != null || e.cost_usd != null) ? mVal(raw) : values.reduce((a, b) => a + b, 0);
      const subMap = new Map();
      for (const e of g.entries) {
        const sk = isFam ? (e.base || "__unknown__") : e.provider;
        if (!subMap.has(sk)) subMap.set(sk, []);
        subMap.get(sk).push(e);
      }
      const subs = [...subMap.entries()].map(([sk, list]) => ({
        ...mergeAgg(list), kind: isFam ? "model" : "tool",
        provider: isFam ? null : sk, family: g.family,
        name: isFam ? (sk === "__unknown__" ? L("未知模型", "Unknown model") : sk) : providerName(sk),
      }));
      const provNames = [...new Set(g.entries.map(e => providerName(e.provider)).filter(Boolean))];
      const vendor = isFam ? "" : provNames.length > 1
        ? `${familyName(g.family)} · ${L(`${provNames.length} 工具`, `${provNames.length} tools`)}`
        : (provNames[0] ? `${familyName(g.family)} · ${provNames[0]}` : familyName(g.family));
      return {
        id: g.id, gid: `${isFam ? "f" : "m"}_${g.id.replace(/[^a-z0-9]/gi, "_")}`,
        family: g.family, ci: famColorIdx(g.family),
        name: isFam ? familyName(g.family) : (g.base || L("未知模型", "Unknown model")),
        vendor, raw, values, value, price: pricePerM(raw), delta: change(value, prevVal(g.id)),
        hit: raw.cache_hit_rate, calls: raw.calls, subs,
      };
    }).sort((a, b) => b.value - a.value || a.name.localeCompare(b.name));
    if (isFam) return items.map((s, order) => ({ ...s, order, shade: 0, dash: false }));
    const seen = new Map();
    return items.map((s, order) => { const r = seen.get(s.family) || 0; seen.set(s.family, r + 1); return { ...s, order, shade: Math.min(2, r), dash: r > 0 }; });
  }

  function buildSeriesItems(data, buckets) {
    const unknownIdx = new Map();
    const colorIdx = (k, known) => (known.has(k) || k === "other") ? null : (unknownIdx.has(k) ? unknownIdx.get(k) : (unknownIdx.set(k, unknownIdx.size % 8), unknownIdx.get(k)));
    const mVal = item => state.metric === "cost" ? num(item?.cost_usd) : (isNet() ? netTokens(item) : num(item?.total_tokens));
    const pVal = item => !item ? null : (state.metric === "cost" ? item.prev_cost_usd : (isNet() ? (item.prev_net_tokens ?? item.prev_total_tokens) : item.prev_total_tokens));

    if (state.dim !== "provider") return buildGroupedModels(data, buckets, mVal, f => colorIdx(f, KNOWN_FAMILIES));
    const byProv = new Map((data.providers || []).map(p => [p.provider, p]));
    const keys = [...new Set([...byProv.keys(), ...buckets.flatMap(b => Object.keys(b.by_provider || {}))])];
    return keys.map(p => {
      const raw = byProv.get(p) || { provider: p, models: [] }, values = buckets.map(b => mVal(b.by_provider?.[p]));
      const value = byProv.has(p) ? mVal(raw) : values.reduce((a, b) => a + b, 0);
      const subs = (raw.models || []).map(m => ({ ...m, kind: "model", name: modelLabel(m.model) }));
      return {
        id: p, gid: p.replace(/[^a-z0-9]/gi, "_"), provider: p, ci: colorIdx(p, KNOWN_PROVIDERS), shade: 0, dash: false,
        name: providerName(p), raw, values, value, price: pricePerM(raw),
        delta: change(value, pVal(raw)), hit: raw.cache_hit_rate ?? null, calls: num(raw.calls), subs,
      };
    }).sort((a, b) => b.value - a.value || a.name.localeCompare(b.name)).map((s, order) => ({ ...s, order }));
  }

  function trendTitleText(data, grain) {
    const map = { "24h": L("24 小时走势", "24h trend"), today: L("今日走势", "Today's trend"), "7d": L("7 日走势", "7-day trend"), "30d": L("30 日走势", "30-day trend") };
    if (map[state.range]) return map[state.range];
    if (state.range === "custom" && state.custom) {
      const c = state.custom;
      if (c.kind === "preset90d") return L("90 日走势", "90-day trend");
      if (c.kind === "hours") return L(`${c.n} 小时走势`, `${c.n}h trend`);
      if (c.presetId === "yesterday") return L("昨日走势", "Yesterday's trend");
      if (c.presetId === "this-month") return L("本月走势", "This month's trend");
      if (c.presetId === "last-month") return L("上月走势", "Last month's trend");
      if (c.fromDay === c.toDay && window.UsagePicker) return L(`${window.UsagePicker.formatCustomLabel(c)} 走势`, `${window.UsagePicker.formatCustomLabel(c)} trend`);
    }
    return grain === "hour" ? L(`${(data.daily || []).length || 24} 小时走势`, `${(data.daily || []).length || 24}h trend`) : L("走势对比", "Usage trend");
  }

  // ---- KPIs, ranking, composition -----------------------------------------------------
  function kpisSection(data, buckets, totals) {
    const o = data.overall || {}, p = data.prev_overall || null;
    const spark = fn => window.UsageChart ? window.UsageChart.sparkSvg(buckets.filter(b => b.has).map(fn), 52, 22, null) : null;
    const un = num(o.unpriced_tokens), totTok = num(o.total_tokens);
    const costSub = [h("span.u-pill", {}, ...deltaNodes(p ? change(o.cost_usd, p.cost_usd) : null, "lowerBetter"))];
    if (un > 0 && totTok > 0) costSub.push(span("warn", L(`未定价 ${pct(un / totTok)}`, `${pct(un / totTok)} unpriced`)));
    const curTok = tokValue(o), prevTok = p ? tokValue(p) : null;
    const hitKnown = o.cache_hit_rate != null && Number.isFinite(Number(o.cache_hit_rate));
    const prevHit = p?.cache_hit_rate != null && Number.isFinite(Number(p.cache_hit_rate)) ? fraction(p.cache_hit_rate) : null;
    const pp = hitKnown && prevHit != null ? (fraction(o.cache_hit_rate) - prevHit) * 100 : null;
    let peakVal = 0, peakLabel = "—";
    buckets.forEach((b, i) => { if (b.has && totals[i] > peakVal) { peakVal = totals[i]; peakLabel = b.label || b.date || "—"; } });

    return h("section.u-card.u-kpis", { "aria-label": L("汇总", "Summary") },
      h("div.u-kpi", {}, span("u-kpi-label", L("折算花费", "Estimated cost")),
        h("div.u-kpi-main", {}, h(`div.u-kpi-value${unpricedAll(o) ? ".none" : ""}`, {}, costText(o)), spark(b => num(b.cost_usd))),
        h("div.u-kpi-sub", {}, ...costSub)),
      h("div.u-kpi", {}, span("u-kpi-label", isNet() ? L("净 Token", "Net tokens") : L("总 Token", "Tokens")),
        h("div.u-kpi-main", {}, h("div.u-kpi-value", {}, compact(curTok)), spark(b => tokValue(b))),
        h("div.u-kpi-sub", {}, h("span.u-pill", {}, ...deltaNodes(prevTok != null ? change(curTok, prevTok) : null, "neutral")), span("", L(`${count(o.total_calls)} 次调用`, `${count(o.total_calls)} calls`)))),
      h("div.u-kpi", {}, span("u-kpi-label", L("缓存命中率", "Cache hit rate")),
        h("div.u-kpi-main", {}, h(`div.u-kpi-value${hitKnown ? "" : ".none"}`, { title: hitExact(o.cache_hit_rate) }, pct(o.cache_hit_rate)),
          h("div.u-meter", { "aria-hidden": "true" }, hitKnown ? fill(h("i"), fraction(o.cache_hit_rate)) : null)),
        h("div.u-kpi-sub", {}, h("span.u-pill", {}, ...(pp == null ? ["—"] : [arrowNode(pp, "higherBetter"), `${Math.abs(pp).toFixed(1)} pp`])))),
      h("div.u-kpi", {}, span("u-kpi-label", L("峰值", "Peak")),
        h("div.u-kpi-main", {}, h(`div.u-kpi-value${peakVal > 0 ? "" : ".none"}`, {}, peakVal > 0 ? fmtValue(peakVal) : "—")),
        h("div.u-kpi-sub", {}, span("", peakLabel))));
  }

  function rankSubRows(s, total) {
    return [...(s.subs || [])].sort((a, b) => metricValue(b) - metricValue(a)).map((m, idx) => {
      const mVal = metricValue(m), mPrice = pricePerM(m), isTool = m.kind === "tool";
      const lead = isTool ? tile(m.provider) : h("i.u-dot", toneProps(s.provider, Math.min(2, idx), s.ci, s.family));
      return h(`div.u-rrow.sub${isTool ? ".sub-tool" : ""}`, { title: `${m.name} · ${(total > 0 ? mVal / total * 100 : 0).toFixed(1)}%` },
        h("span.u-rname", {}, lead, h("em", {}, m.name)),
        h("span.u-spark-cell"),
        h("span.num.u-val", {}, state.metric === "cost" && (m.priced === false || unpricedAll(m)) ? h("span.u-tag", {}, L("未定价", "Unpriced")) : (state.metric === "cost" ? costText(m) : fmtValue(mVal))),
        span(`num u-price${mPrice == null ? " dim" : ""}`, fmtPrice(mPrice)),
        h("span.num"), span("num u-hit", pct(m.cache_hit_rate), hitExact(m.cache_hit_rate)));
    });
  }

  function rankRowNode(s, dim, total, buckets, redrawAll) {
    const isProv = dim === "provider", isModel = dim === "model";
    const hasSubs = Boolean(s.subs?.length), open = hasSubs && state.open.has(s.id);
    const subTip = isModel ? L(`${s.name} 工具明细`, `${s.name} by tool`) : L(`${s.name} 模型明细`, `${s.name} models`);
    const twist = hasSubs ? h("button.u-twist", { type: "button", "data-open": s.id, "aria-expanded": String(open), "aria-label": subTip }, chevronSvg()) : null;
    twist?.addEventListener("click", e => { e.stopPropagation(); state.open[state.open.has(s.id) ? "delete" : "add"](s.id); redrawAll(); });
    const nameBlock = isProv ? h("span.u-rname", {}, tile(s.provider), h("em", {}, s.name))
      : h("span.u-rname", {}, h("i.u-dot", toneProps(s.provider, s.shade, s.ci, s.family)), h("em", {}, s.name));
    const under = h("span.u-under", {}, isModel && s.vendor ? h("span.u-vendor", {}, h("i.u-dot", toneProps(s.provider, 0, s.ci, s.family)), span("", s.vendor)) : null, h("span.u-track", {}, fill(h("i"), total > 0 ? s.value / total : 0)));
    const spark = window.UsageChart ? window.UsageChart.sparkSvg(buckets.map((b, i) => b.has ? s.values[i] : null).filter(v => v != null), 42, 16, s) : null;
    const valNode = span(`num u-val${state.metric === "cost" && unpricedAll(s.raw) ? " dim" : ""}`, state.metric === "cost" ? costText(s.raw) : fmtValue(s.value));
    const share = total > 0 ? s.value / total : 0;
    const row = applyTone(h("div.u-rrow.item", {
      role: "button", tabindex: "0", "aria-pressed": String(state.pinned.has(s.id)), "data-id": s.id,
      title: `${s.name} · ${(share * 100).toFixed(1)}% · ${count(s.calls)} ${L("次调用", "calls")}`,
    }, h(`span.u-lead${hasSubs ? "" : ".flat"}`, {}, twist, h("span.u-who", {}, nameBlock, under)),
    h("span.u-spark-cell", {}, spark), valNode, span(`num u-price${s.price == null ? " dim" : ""}`, fmtPrice(s.price)),
    h("span.num", {}, h("span.u-delta", {}, ...deltaNodes(s.delta, metricTone()))), span("num u-hit", pct(s.hit), hitExact(s.hit))), s);
    return { row, subs: open ? rankSubRows(s, total) : [] };
  }

  function rankCard(items, total, buckets, redrawAll) {
    const dim = state.dim;
    const firstCol = dim === "provider" ? L("工具", "Tool") : dim === "family" ? L("模型系列", "Family") : L("模型", "Model");
    const cols = [["name", firstCol], ["value", state.metric === "cost" ? L("花费", "Cost") : "Token"], ["price", "$/1M"], ["delta", L("环比", "Change")], ["hit", L("命中", "Hit")]];
    const mkHead = ([k, lbl]) => {
      const on = state.sort.key === k;
      return h(`button${on ? ".on" : ""}`, { type: "button", "data-sort": k }, `${lbl}${on ? (state.sort.dir < 0 ? " ↓" : " ↑") : ""}`);
    };
    const head = h("div.u-rrow.head", {}, mkHead(cols[0]), span("u-spark-cell", ""), ...cols.slice(1).map(mkHead));
    head.addEventListener("click", e => {
      const btn = e.target?.closest?.("[data-sort]");
      if (!btn) return;
      const key = dataAttr(btn, "sort");
      state.sort = state.sort.key === key ? { key, dir: -state.sort.dir } : { key, dir: key === "name" ? 1 : -1 };
      redrawAll();
    });
    const sorted = [...items].sort((a, b) => state.sort.key === "name" ? state.sort.dir * a.name.localeCompare(b.name)
      : (a[state.sort.key] == null || b[state.sort.key] == null) ? (a[state.sort.key] == null) - (b[state.sort.key] == null)
      : state.sort.dir * (a[state.sort.key] - b[state.sort.key]) || b.value - a.value);
    const rankRows = [], bodyNodes = [];
    for (const s of sorted) { const built = rankRowNode(s, dim, total, buckets, redrawAll); rankRows.push(built.row); bodyNodes.push(built.row, ...built.subs); }
    const card = h(`section.u-card.u-rank${dim === "model" ? ".u-mono" : ""}`, { "aria-label": L("排行", "Ranking") }, head, ...bodyNodes);
    return { card, rankRows };
  }

  function compSection(data) {
    const o = data.overall || {}, p = data.prev_overall || null, net = isNet(), base = net ? netTokens(o) : num(o.total_tokens);
    const parts = [
      { cls: "u-comp-fresh", label: L("新输入", "Input"), v: num(o.fresh_input), prev: p?.fresh_input, cut: false },
      { cls: "u-comp-read", label: L("缓存读", "Cache read"), v: num(o.cache_read), prev: p?.cache_read, cut: net },
      { cls: "u-comp-write", label: L("缓存写", "Cache write"), v: writes(o), prev: p ? writes(p) : null, cut: false },
      { cls: "u-comp-out", label: L("输出", "Output"), v: num(o.output), prev: p?.output, cut: false },
    ];
    return h(`div.u-comp${net ? ".net" : ""}`, {},
      h("div.u-comp-head", {}, span("", net ? L("净 Token", "Net tokens") : L("Token 构成", "Token mix")), h("b", {}, compact(base))),
      h("div.u-comp-bar", {}, ...parts.map(pt => flex(h(`i.${pt.cls}${pt.cut ? ".is-cut" : ""}`, { title: `${pt.label} ${compact(pt.v)}` }), pt.v))),
      h("div.u-comp-grid", {}, ...parts.map(pt => h(`div.u-comp-item${pt.cut ? ".is-cut" : ""}`, {},
        h("div.lab", {}, h(`i.${pt.cls}`), span("", pt.label)),
        h("div", {}, h("strong", {}, compact(pt.v))),
        h("div", {}, h("small", {}, pt.cut ? "—" : `${(base > 0 ? pt.v / base * 100 : 0).toFixed(1)}%`), h("small", {}, ...deltaNodes(pt.prev != null ? change(pt.v, pt.prev) : null, "neutral")))))));
  }

  // ---- overall view ------------------------------------------------------------------
  function mkToolSeg(label, cls, opts, cur, onPick) {
    return h(`div.u-seg.seg-sm${cls ? `.${cls}` : ""}`, { role: "radiogroup", "aria-label": label },
      ...opts.map(([val, lbl, tip]) => {
        const b = h("button", { type: "button", role: "radio", "aria-checked": String(cur === val), ...(tip ? { title: tip, "aria-label": tip } : {}) }, lbl);
        b.addEventListener("click", () => { if (cur !== val) onPick(val); });
        return b;
      }));
  }

  function wirePlotEvents(plot, buckets, grain, getGeo, syncHighlight, drillAt) {
    const indexAt = e => getGeo()?.at((e.clientX || 0) - (plot.getBoundingClientRect?.()?.left || 0)) ?? 0;
    const move = e => {
      if (!getGeo()) return;
      const i = indexAt(e), next = buckets[i]?.has ? i : null;
      if (next !== state.cursor) { state.cursor = next; syncHighlight(); }
    };
    plot.addEventListener("pointermove", move);
    plot.addEventListener("pointerdown", move);
    plot.addEventListener("pointerleave", e => { if (e.pointerType !== "touch") { state.cursor = null; syncHighlight(); } });
    plot.addEventListener("click", e => { if (grain === "day" && getGeo()) drillAt(indexAt(e)); });
    plot.addEventListener("blur", () => { state.cursor = null; syncHighlight(); });
    plot.addEventListener("keydown", e => {
      if (e.key === "Enter" && state.cursor != null) { e.preventDefault(); drillAt(state.cursor); return; }
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      e.preventDefault();
      const valid = buckets.map((b, i) => b.has ? i : -1).filter(i => i >= 0);
      if (!valid.length) return;
      const next = state.cursor == null ? (e.key === "ArrowLeft" ? valid.at(-1) : valid[0]) : state.cursor + (e.key === "ArrowLeft" ? -1 : 1);
      state.cursor = Math.max(valid[0], Math.min(valid.at(-1), next));
      syncHighlight();
    });
  }

  function wirePinGroup(container, onPin, onHover) {
    const hit = e => e.target?.closest?.(".u-chip,.u-rrow.item,.u-arc");
    container.addEventListener("click", e => { if (!e.target?.closest?.(".u-twist") && hit(e)) onPin(dataAttr(hit(e), "id")); });
    container.addEventListener("pointerover", e => { if (e.pointerType !== "touch") onHover(hit(e) ? dataAttr(hit(e), "id") : null); });
    container.addEventListener("pointerleave", () => onHover(null));
    container.addEventListener("keydown", e => {
      if ((e.key === "Enter" || e.key === " ") && e.target?.classList?.contains?.("item")) { e.preventDefault(); onPin(dataAttr(e.target, "id")); }
    });
  }

  const LEGEND_CAP = 10;
  function buildLegend(legendEl, items, clearBtn, onToggleMore) {
    const ordered = [...items].sort((a, b) => a.order - b.order);
    const nameCounts = new Map();
    if (state.dim === "model") for (const s of ordered) nameCounts.set(s.name, (nameCounts.get(s.name) || 0) + 1);
    const needCap = ordered.length > LEGEND_CAP && !state.legendAll;
    const shown = needCap ? ordered.filter((s, idx) => idx < LEGEND_CAP || state.pinned.has(s.id)) : ordered;
    const chips = shown.map(s => {
      const dup = state.dim === "model" && (nameCounts.get(s.name) || 0) > 1 && s.vendor;
      return applyTone(h(`button.u-chip${state.dim === "model" ? ".model" : ""}`, {
        type: "button", "data-id": s.id, "aria-pressed": String(state.pinned.has(s.id)),
        ...(state.dim === "model" && s.vendor ? { title: `${s.name} · ${s.vendor}` } : {}),
      }, h(`i.u-key${s.dash ? ".dash" : ""}`, toneProps(s.provider, s.shade, s.ci, s.family)), s.name,
      dup ? h("span.u-chip-sub", {}, s.vendor) : null), s);
    });
    const hiddenCount = ordered.length - shown.length;
    const moreBtn = ordered.length > LEGEND_CAP ? h("button.u-more-chips", { type: "button" },
      state.legendAll ? L("收起", "Less") : L(`其余 ${hiddenCount} 项`, `+${hiddenCount} more`)) : null;
    moreBtn?.addEventListener("click", onToggleMore);
    legendEl.replaceChildren(...chips, ...(moreBtn ? [moreBtn] : []), clearBtn);
    return chips;
  }

  function overall(data) {
    const o = data.overall || {};
    if (!num(o.total_tokens) && !num(o.total_calls)) return [empty(data.scanning ? L("正在统计，请稍候", "Counting, please wait") : L("这段时间没有用量记录", "No usage in this period"))];
    const { grain, buckets, nowIdx, midnightIdx } = normalizeBuckets(data), items = buildSeriesItems(data, buckets);
    const totals = buckets.map(b => state.metric === "cost" ? num(b.cost_usd) : tokValue(b)), total = metricValue(o);

    const backBtn = h("button.u-back", { type: "button", id: "u-back", hidden: !state.back }, L("← 返回", "← Back"));
    backBtn.addEventListener("click", () => {
      if (!state.back) return;
      Object.assign(state, state.back, { back: null, cursor: null, hover: null });
      savePrefs(); render(); fetchUsage();
    });
    const availGrains = Array.isArray(data.available_granularities) ? data.available_granularities : [];
    const modeSeg = mkToolSeg(L("曲线模式", "Line mode"), "", [["total", L("总计 + 对比", "Total + compare")], ["series", L("仅各系列", "Series only")]], state.mode, v => { state.mode = v; savePrefs(); renderBody(true); });
    modeSeg.hidden = state.shape === "bar";
    const gMap = { hour: L("时", "Hour"), day: L("天", "Day"), week: L("周", "Week") };
    const grainSeg = mkToolSeg(L("分桶粒度", "Granularity"), "", availGrains.map(g => [g, gMap[g] || g]), grain, g => { state.granularity = g; state.cursor = state.hover = null; render(); fetchUsage(); });
    grainSeg.hidden = availGrains.length < 2;
    const shapeSeg = mkToolSeg(L("图表形态", "Chart shape"), "seg-shape", [["line", lineIconSvg(), L("折线图", "Line chart")], ["bar", barIconSvg(), L("柱状图", "Bar chart")]], state.shape, v => { state.shape = v; savePrefs(); renderBody(true); });

    const plot = h("div.u-plot", { tabindex: "0", "aria-label": L("走势图，左右方向键查看各时间点", "Trend chart, use arrow keys to inspect slots") });
    const svg = svgEl("svg", { id: "u-chart", role: "img" }), tip = h("div.u-tip", { hidden: true, role: "status" });
    plot.append(svg, tip);
    const clearBtn = h("button.u-clear-pins", { type: "button", hidden: state.pinned.size < 2 }, L("清除", "Clear"));
    const legendEl = h("div.u-legend");
    let chips = [];

    const split = h(`div.u-split${state.open.size > 0 ? ".grown" : ""}`), donutBox = h("div.u-donut");
    const donutRefs = window.UsageChart ? window.UsageChart.drawDonut(donutBox, items, total, fmtValue) : { arcs: [], center: null };
    const sideCard = h("section.u-card.u-side", {}, donutBox, compSection(data));

    let geo = null, rankRefs = { card: h("section.u-card.u-rank"), rankRows: [] };
    const redrawChart = () => {
      state.drawnWidth = plot.clientWidth || 0;
      if (window.UsageChart) geo = window.UsageChart.drawTrend({ plot, svg, tip, items, buckets, totals, grain, mode: state.mode, shape: state.shape, metric: state.metric, pinned: state.pinned, hover: state.hover, nowIdx, midnightIdx, title: trendTitleText(data, grain), fmt: fmtValue, fmtAxis });
    };
    const syncHighlight = () => {
      clearBtn.hidden = state.pinned.size < 2;
      if (!window.UsageChart) return;
      window.UsageChart.applyHighlight({ geo, items, pinned: state.pinned, hover: state.hover, shape: state.shape, mode: state.mode, dim: state.dim, total, metricLabel: metricLabel(), fmt: fmtValue, chips, rankRows: rankRefs.rankRows, arcs: donutRefs.arcs, donutCenter: donutRefs.center });
      window.UsageChart.drawCursor(geo, { plot, cursor: state.cursor, buckets, items, grain, mode: state.mode, pinned: state.pinned, hover: state.hover, dim: state.dim, fmt: fmtValue, hitExact, hitOfBucket: b => b.cache_hit_rate ?? null });
    };
    const visibleLegendKey = () => {
      if (items.length <= LEGEND_CAP || state.legendAll) return "all";
      return items.filter((s, idx) => idx < LEGEND_CAP || state.pinned.has(s.id)).map(s => s.id).join("\0");
    };
    let legendKey = "";
    const rebuildLegend = (force = false) => {
      const nextKey = visibleLegendKey();
      if (!force && nextKey === legendKey) return;
      legendKey = nextKey;
      chips = buildLegend(legendEl, items, clearBtn, () => { state.legendAll = !state.legendAll; rebuildLegend(true); syncHighlight(); });
    };
    const syncVisuals = (redrawTrend = false) => { if (redrawTrend) redrawChart(); syncHighlight(); };
    const rebuildRank = () => {
      rankRefs = rankCard(items, total, buckets, () => { rebuildRank(); split.classList.toggle("grown", state.open.size > 0); syncHighlight(); });
      split.replaceChildren(rankRefs.card, sideCard);
    };
    rebuildLegend(true);
    rebuildRank();

    clearBtn.addEventListener("click", () => { state.pinned.clear(); state.hover = null; rebuildLegend(); syncVisuals(true); });
    const togglePin = id => { state.pinned[state.pinned.has(id) ? "delete" : "add"](id); rebuildLegend(); syncVisuals(state.shape === "line"); };
    const setHover = id => { if (state.hover !== id) { state.hover = id; syncHighlight(); } };
    wirePinGroup(legendEl, togglePin, setHover);
    wirePinGroup(split, togglePin, setHover);
    wirePlotEvents(plot, buckets, grain, () => geo, syncHighlight, idx => {
      const b = buckets[idx];
      if (grain !== "day" || !b?.has) return;
      let fromMs = b.start_ms != null ? num(b.start_ms) : null, toMs = b.end_ms != null ? num(b.end_ms) : null;
      if (fromMs == null || toMs == null) {
        const [y, m, d] = String(b.date || "").slice(0, 10).split("-").map(Number);
        if (!y || !m || !d) return;
        fromMs = new Date(y, m - 1, d, 0, 0, 0, 0).getTime(); toMs = fromMs + DAY_MS;
      }
      state.back = { range: state.range, presetRange: state.presetRange, custom: state.custom, granularity: state.granularity };
      const dk = window.UsagePicker ? window.UsagePicker.toDayKey(fromMs) : 0;
      state.range = "custom"; state.custom = { kind: "days", fromDay: dk, toDay: dk, startHour: 0, endHour: 24, fromMs, toMs };
      state.granularity = "hour"; state.cursor = state.hover = null;
      render(); fetchUsage();
    });

    const trendCard = h("section.u-card.u-trend", {},
      h("div.u-trend-head", {}, h("div.u-trend-title", {}, backBtn, h("h2", { id: "u-trend-title" }, trendTitleText(data, grain))), h("div.u-trend-tools", {}, modeSeg, grainSeg, shapeSeg)),
      plot, legendEl);
    state.redraw = () => syncVisuals(true);
    state._escClear = () => { state.pinned.clear(); state.hover = state.cursor = null; rebuildLegend(); syncVisuals(true); };
    queueMicrotask(() => syncVisuals(true));
    return [kpisSection(data, buckets, totals), trendCard, split];
  }

  // ---- projects view -----------------------------------------------------------------
  function splitBar(bossVal, workerVal, nativeVal, showAgents, showNative) {
    const b = showAgents ? num(bossVal) : 0, w = showAgents ? num(workerVal) : 0, n = showNative ? num(nativeVal) : 0, sum = b + w + n;
    const parts = [], tips = [];
    if (showAgents) { parts.push(flex(h("i.boss"), b), flex(h("i.worker"), w)); tips.push(`Boss ${pct(sum ? b / sum : 0)}`, `${L("员工", "Employees")} ${pct(sum ? w / sum : 0)}`); }
    if (showNative) { parts.push(flex(h("i.native"), n)); tips.push(`${L("原生", "Native")} ${pct(sum ? n / sum : 0)}`); }
    return h("div.u-bw-bar", { title: tips.join(" · ") }, ...parts);
  }
  function projectGroup(label, stats, marker) {
    const s = stats || {}, primary = state.metric === "cost" ? costText(s) : compact(isNet() ? netTokens(s) : s.total_tokens);
    const sec = state.metric === "cost" ? h("span", {}, `${isNet() ? L("净 Token", "Net") : "Token"} `, h("b", {}, compact(isNet() ? netTokens(s) : s.total_tokens)))
      : h("span", {}, `${L("花费", "Cost")} `, h("b", {}, costText(s)));
    return h("div.u-group", {}, h("span.u-lab.lab", {}, marker ? h(`i.u-sq.${marker}`) : null, label),
      h(`strong${state.metric === "cost" && unpricedAll(s) ? ".dim" : ""}`, {}, primary),
      h("small", {}, sec, h("span", { title: hitExact(s.cache_hit_rate) }, `${L("命中", "Hit")} `, h("b", {}, pct(s.cache_hit_rate)))));
  }
  function projectDetail(project, showAgents, showNative) {
    const cls = showAgents && showNative ? "" : showAgents ? ".cols-6" : ".cols-5";
    const head = h(`div.u-prow.head${cls}`, {}, span("", ""), span("", ""), span("num", L("折算花费", "Cost")), span("num", isNet() ? L("净 Token", "Net") : "Token"),
      showAgents ? span("num extra", "Boss") : null, showAgents ? span("num extra", L("员工", "Employees")) : null, showNative ? span("num extra", L("原生", "Native")) : null);
    const rows = [...(project.by_provider || [])].sort((a, b) => num(b.cost_usd) - num(a.cost_usd) || num(b.total_tokens) - num(a.total_tokens)).map(p =>
      h(`div.u-prow${cls}`, {}, h("span.u-rname", {}, tile(p.provider), h("em", {}, providerName(p.provider))),
        splitBar(p.boss_tokens, p.worker_tokens, p.native_tokens, showAgents, showNative),
        span(`num${unpricedAll(p) ? " dim" : ""}`, costText(p)), span("num", compact(isNet() ? netTokens(p) : p.total_tokens)),
        showAgents ? span("num extra", state.metric === "cost" && p.boss_cost != null ? money(p.boss_cost) : compact(p.boss_tokens)) : null,
        showAgents ? span("num extra", state.metric === "cost" && p.worker_cost != null ? money(p.worker_cost) : compact(p.worker_tokens)) : null,
        showNative ? span("num extra", state.metric === "cost" && p.native_cost != null ? money(p.native_cost) : compact(p.native_tokens)) : null));
    return h("div.u-pdetail", {}, head, ...rows);
  }
  function projects(data) {
    const list = [...(data.projects || [])].sort((a, b) => (a.key === "other") - (b.key === "other") || num(b.total?.cost_usd) - num(a.total?.cost_usd) || num(b.total?.total_tokens) - num(a.total?.total_tokens));
    if (!list.length) return [empty(data.scanning ? L("正在首次扫描，请稍候", "Scanning for the first time, please wait") : L("还没有项目用量记录", "No project usage yet"))];
    const hasAgents = list.some(p => num(p.boss?.total_tokens) > 0 || num(p.workers?.total_tokens) > 0 || num(p.boss?.calls) > 0 || num(p.workers?.calls) > 0);
    const hasNative = list.some(p => p.native != null || (p.by_provider || []).some(bp => bp.native_tokens != null));
    const showAgents = hasAgents || !hasNative, showNative = hasNative, allTotal = list.reduce((a, p) => a + metricValue(p.total), 0);
    return list.map(project => {
      const total = project.total || {}, boss = project.boss || {}, workers = project.workers || {}, nat = project.native || {}, key = `p:${project.key}`, open = state.openProjects.has(key);
      const shareStr = allTotal > 0 ? `${(metricValue(total) / allTotal * 100).toFixed(1)}%` : "";
      const btn = h("button.u-pbtn", { type: "button", "aria-expanded": String(open) },
        h("div.u-ptitle", {}, chevronSvg(), h("b", {}, projectName(project)), project.key !== "other" && project.path ? h("span.u-path", { title: project.path }, project.path) : null, shareStr ? span("u-pshare", shareStr) : null),
        h(`div.u-groups${showAgents && showNative ? "" : showAgents ? ".cols-3" : ".cols-2"}`, {},
          projectGroup(L("合计", "Total"), total), showAgents ? projectGroup("Boss", boss, "boss") : null,
          showAgents ? projectGroup(L("员工", "Employees"), workers, "worker") : null, showNative ? projectGroup(L("原生", "Native"), nat, "native") : null),
        splitBar(boss.total_tokens, workers.total_tokens, nat.total_tokens, showAgents, showNative));
      const detail = projectDetail(project, showAgents, showNative);
      detail.hidden = !open;
      btn.addEventListener("click", () => {
        const next = btn.getAttribute("aria-expanded") !== "true";
        btn.setAttribute("aria-expanded", String(next));
        detail.hidden = !next;
        state.openProjects[next ? "add" : "delete"](key);
      });
      return h("section.u-card.u-project", {}, btn, detail);
    });
  }

  // ---- page render & wiring ----------------------------------------------------------
  function renderControls() {
    for (const b of document.getElementById("u-view")?.children || []) b.setAttribute("aria-checked", String(dataAttr(b, "view") === state.view));
    for (const b of document.getElementById("u-range")?.children || []) {
      const r = dataAttr(b, "range");
      b.setAttribute("aria-checked", String(r === state.range));
      if (r === "custom") {
        const lbl = state.range === "custom" && window.UsagePicker ? window.UsagePicker.formatCustomLabel(state.custom) : L("自定义", "Custom");
        b.textContent = lbl; b.setAttribute("title", lbl);
      }
    }
    const subbar = document.getElementById("u-subbar"), cacheEl = document.getElementById("u-cache");
    if (subbar) subbar.hidden = state.view !== "overall";
    if (cacheEl) cacheEl.hidden = state.metric !== "tokens";
    for (const b of document.getElementById("u-dim")?.children || []) b.setAttribute("aria-checked", String(dataAttr(b, "dim") === state.dim));
    for (const b of cacheEl?.children || []) b.setAttribute("aria-checked", String(dataAttr(b, "cache") === state.cache));
    for (const b of document.getElementById("u-metric")?.children || []) b.setAttribute("aria-checked", String(dataAttr(b, "metric") === state.metric));
  }
  function renderSync() {
    const data = state.data, label = document.getElementById("u-updated"), button = document.getElementById("u-refresh");
    const busy = showScanning() || (state.manual && state.grace > 0);
    button.setAttribute("data-busy", String(busy));
    if (button.dataset) button.dataset.busy = String(busy);
    label.classList.toggle("error", state.error);
    if (state.error) label.textContent = L("读取失败，稍后重试", "Couldn't read usage, retrying");
    else if (switching()) label.textContent = L("读取中…", "Loading…");
    else if (busy) label.textContent = L("扫描中…", "Scanning…");
    else if (data?.updated_at) label.textContent = L(`更新于 ${AN.relTime(new Date(data.updated_at).toISOString())}`, `Updated ${AN.relTime(new Date(data.updated_at).toISOString())}`);
    else label.textContent = "";
  }
  function renderFoot() {
    const foot = document.getElementById("u-foot"), names = (state.data?.uncovered_installed || []).map(providerName);
    foot.hidden = !names.length;
    foot.textContent = names.length ? L(`暂未统计：${names.join("、")}`, `Not tracked yet: ${names.join(", ")}`) : "";
  }
  function renderBody(force = false) {
    if (switching() && !force) return;
    const data = state.data, body = document.getElementById("u-body"), lang = String(document.documentElement?.lang || "zh");
    const sig = `${state.view}|${state.range}|${state.dim}|${state.metric}|${state.cache}|${state.mode}|${state.shape}|${state.sort.key}:${state.sort.dir}|${lang}|${Boolean(AN.fullModelNames?.())}|${!data && state.error}|${JSON.stringify(data)}`;
    if (!force && sig === state.sig) return;
    state.sig = sig; state.redraw = null; state._escClear = null;
    if (!data) return body.replaceChildren(empty(state.error ? L("暂时读不到用量，稍后会自动重试", "Usage is unavailable, retrying shortly") : ""));
    body.replaceChildren(...(state.view === "projects" ? projects(data) : overall(data)));
  }
  function render() {
    renderControls(); renderSync();
    document.getElementById("u-body").setAttribute("aria-busy", String(switching()));
    renderBody(); renderFoot();
  }
  function pickRange(nextRange, customSpec = null) {
    state.range = nextRange;
    if (PRESET_RANGES.has(nextRange)) state.presetRange = nextRange;
    state.custom = customSpec; state.granularity = null; state.back = null;
    state.cursor = state.hover = null; state.error = false;
    savePrefs(); render(); fetchUsage();
  }

  loadPrefs();
  const rangeEl = document.getElementById("u-range"), popEl = document.getElementById("u-pop"), appEl = document.getElementById("u-app");
  const customBtn = [...(rangeEl?.children || [])].find(b => dataAttr(b, "range") === "custom") || null;
  if (window.UsagePicker && popEl) {
    state.picker = window.UsagePicker.create({
      pop: popEl, app: appEl, anchor: rangeEl, customBtn,
      getNowMs: () => state.data?.now_ms || Date.now(),
      getMinTsMs: () => state.data?.min_ts_ms ?? null,
      getCurrent: () => ({ range: state.range, custom: state.custom, fromMs: state.data?.from_ms, toMs: state.data?.to_ms }),
      onApply: spec => pickRange("custom", spec),
    });
  }
  document.getElementById("u-view").addEventListener("click", e => {
    const v = dataAttr(e.target?.closest?.("button[data-view]") || e.target?.closest?.("button"), "view");
    if (v && v !== state.view) { state.view = v; savePrefs(); render(); }
  });
  rangeEl.addEventListener("click", e => {
    const r = dataAttr(e.target?.closest?.("button[data-range]") || e.target?.closest?.("button"), "range");
    if (!r) return;
    if (r === "custom") { if (state.picker?.isOpen()) state.picker.close(false); else state.picker?.open(); return; }
    state.picker?.close(false);
    if (r !== state.range || state.back) pickRange(r);
  });
  document.getElementById("u-dim")?.addEventListener("click", e => {
    const d = dataAttr(e.target?.closest?.("button[data-dim]") || e.target?.closest?.("button"), "dim");
    if (d && d !== state.dim) { state.dim = d; state.pinned.clear(); state.open.clear(); state.legendAll = false; state.hover = state.cursor = null; savePrefs(); render(); }
  });
  document.getElementById("u-cache")?.addEventListener("click", e => {
    const c = dataAttr(e.target?.closest?.("button[data-cache]") || e.target?.closest?.("button"), "cache");
    if (c && c !== state.cache) { state.cache = c; savePrefs(); render(); }
  });
  document.getElementById("u-metric")?.addEventListener("click", e => {
    const m = dataAttr(e.target?.closest?.("button[data-metric]") || e.target?.closest?.("button"), "metric");
    if (m && m !== state.metric) { state.metric = m; savePrefs(); render(); }
  });
  document.getElementById("u-refresh").addEventListener("click", refresh);
  document.addEventListener("pointerdown", e => {
    if (state.picker?.isOpen() && !popEl.contains?.(e.target) && !e.target?.closest?.('[data-range="custom"]')) state.picker.close(false);
  });
  document.addEventListener("keydown", e => {
    if (e.key !== "Escape") return;
    if (state.picker?.isOpen()) { state.picker.close(true); return; }
    if (state.view === "overall") state._escClear?.();
  });
  window.addEventListener("message", event => {
    if (window.parent !== window && event.source === window.parent && event.data?.type === "agentnave:settings" && event.data.action === "language") {
      AN.setLanguage(event.data.language); state.sig = ""; render();
      if (state.picker?.isOpen()) state.picker.open();
    }
  });
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(() => {
      state.picker?.place();
      const box = document.querySelector(".u-plot");
      if (box && state.redraw && box.clientWidth !== state.drawnWidth) state.redraw();
    }).observe(document.getElementById("u-body"));
  }
  AN.onVisibility(() => { if (AN.isHidden()) clearTimeout(timer); else fetchUsage(); });
  window.addEventListener("agentnave:model-names", () => { state.sig = ""; renderBody(); });

  render();
  fetchUsage();
}
