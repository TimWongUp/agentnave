"use strict";
// SVG trend chart, mini sparklines, donut chart, and multi-pinning highlight sync.
// Uses createElementNS and CSS classes / data-p / data-ci only (zero inline style attributes).
window.UsageChart = (() => {
  const { h, L } = AN;
  const NS = "http://www.w3.org/2000/svg";
  const pad2 = n => String(n).padStart(2, "0");

  const el = (tag, attrs, ...kids) => {
    const node = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v != null && v !== false) node.setAttribute(k, String(v));
    }
    node.append(...kids.filter(k => k != null && k !== false));
    return node;
  };
  const setTone = (node, item) => {
    if (!item) return node;
    if (item.family) node.setAttribute("data-f", item.family);
    else if (item.provider) node.setAttribute("data-p", item.provider);
    if (item.ci != null) node.setAttribute("data-ci", String(item.ci));
    if (item.shade) node.setAttribute("data-shade", String(item.shade));
    return node;
  };

  function monotone(pts) {
    const n = pts.length;
    if (!n) return "";
    const f = v => v.toFixed(2);
    if (n === 1) return `M${f(pts[0][0])},${f(pts[0][1])}`;
    const slope = [], tan = [];
    for (let i = 0; i < n - 1; i++) {
      const dx = pts[i + 1][0] - pts[i][0];
      slope[i] = dx === 0 ? 0 : (pts[i + 1][1] - pts[i][1]) / dx;
    }
    tan[0] = slope[0];
    tan[n - 1] = slope[n - 2];
    for (let i = 1; i < n - 1; i++) tan[i] = slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2;
    for (let i = 0; i < n - 1; i++) {
      if (slope[i] === 0) { tan[i] = tan[i + 1] = 0; continue; }
      const a = tan[i] / slope[i], b = tan[i + 1] / slope[i], hyp = Math.hypot(a, b);
      if (hyp > 3) { tan[i] = 3 * a / hyp * slope[i]; tan[i + 1] = 3 * b / hyp * slope[i]; }
    }
    let d = `M${f(pts[0][0])},${f(pts[0][1])}`;
    for (let i = 0; i < n - 1; i++) {
      const [x0, y0] = pts[i], [x1, y1] = pts[i + 1], dx = (x1 - x0) / 3;
      d += `C${f(x0 + dx)},${f(y0 + tan[i] * dx)} ${f(x1 - dx)},${f(y1 - tan[i + 1] * dx)} ${f(x1)},${f(y1)}`;
    }
    return d;
  }

  function niceScale(max, isCost) {
    let topMax = max > 0 ? max : (isCost ? 1 : 1000);
    const raw = topMax / 4, mag = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 2.5, 5, 10].map(k => k * mag).find(s => s >= raw * 0.9999) || raw;
    const count = Math.max(1, Math.ceil(topMax / step - 1e-9));
    return { step, top: step * count, count };
  }

  function sparkSvg(values, w, hBox, tone) {
    const data = values.length > 1 ? values : [values[0] || 0, values[0] || 0];
    const max = Math.max(...data) || 1;
    const pts = data.map((v, i) => [i * w / (data.length - 1), hBox - 1.5 - v / max * (hBox - 3)]);
    const path = el("path", { class: `u-spark-line${tone ? " colored" : ""}`, d: monotone(pts) });
    if (tone) setTone(path, tone);
    return el("svg", { class: tone ? "u-spark" : null, viewBox: `0 0 ${w} ${hBox}`, preserveAspectRatio: "none", "aria-hidden": "true" }, path);
  }

  function tickOrder(buckets, width, grain, nowIdx) {
    const n = buckets.length, all = Array.from({ length: n }, (_, i) => i);
    const first = [nowIdx, 0, n - 1].filter(i => i != null && i >= 0 && i < n);
    if (grain === "hour") {
      const every = k => all.filter(i => buckets[i].hour != null && buckets[i].hour % k === 0);
      if (n > 48) return [...first, ...every(24), ...(width / Math.max(1, n / 24) > 90 ? every(12) : [])];
      return [...first, ...every(24), ...every(6), ...every(3)];
    }
    const step = Math.max(1, Math.ceil(n / Math.max(2, Math.floor(width / 62))));
    return [...first, ...all.filter(i => i % step === 0)];
  }

  function buildGradients(defs, list, totals, y, T, base) {
    const mk = (id, item, isTotal, vals) => {
      const peak = vals?.length ? Math.max(0, ...vals) : 0;
      const y1 = peak > 0 ? Math.min(base - 4, Math.max(T, y(peak))) : T;
      const g = el("linearGradient", { id, x1: 0, y1, x2: 0, y2: base, gradientUnits: "userSpaceOnUse" });
      const s0 = el("stop", { offset: 0, class: `u-stop-start${isTotal ? " total" : ""}` });
      const s1 = el("stop", { offset: 1, class: `u-stop-end${isTotal ? " total" : ""}` });
      if (item) { setTone(s0, item); setTone(s1, item); }
      g.append(s0, s1);
      defs.append(g);
    };
    mk("u-g-total", null, true, totals);
    for (const s of list) mk(`u-g-${s.gid}`, s, false, s.values);
  }

  const roundedBarPath = (x0, y0, w, hh, r) => r <= 0
    ? `M${x0},${y0}H${x0 + w}V${y0 + hh}H${x0}Z`
    : `M${x0},${y0 + hh}V${y0 + r}Q${x0},${y0} ${x0 + r},${y0}H${x0 + w - r}Q${x0 + w},${y0} ${x0 + w},${y0 + r}V${y0 + hh}Z`;

  function activeSelection(items, pinned, hover) {
    const validHover = hover != null && items.some(s => s.id === hover) ? hover : null;
    const validPinned = new Set([...pinned].filter(id => items.some(s => s.id === id)));
    return { validPinned, validHover, hasPinned: validPinned.size > 0 };
  }

  function drawnSet(items, mode, pinned, hover) {
    const set = new Set(items.slice(0, mode === "total" ? 5 : 6).map(s => s.id));
    for (const id of pinned) set.add(id);
    if (hover != null) set.add(hover);
    return set;
  }

  function drawAxesAndGuides(nodes, geo, ctx) {
    const { W, Hh, Lpad, Rpad, Tpad, base, band, bars, x, y, sc } = geo;
    const { buckets, grain, nowIdx, midnightIdx } = ctx;
    const n = buckets.length;
    if (nowIdx != null && nowIdx >= 0 && nowIdx < n) {
      const nx = bars ? x(nowIdx) + band / 2 : x(nowIdx);
      nodes.push(el("rect", { class: "u-future-band", x: nx, y: Tpad, width: Math.max(0, W - Rpad - nx), height: base - Tpad }));
    }
    for (let i = 0; i <= sc.count; i++) {
      const v = sc.step * i;
      nodes.push(el("line", { class: "u-grid", x1: Lpad, x2: W - Rpad, y1: y(v), y2: y(v) }));
      nodes.push(el("text", { class: "u-axis", x: Lpad - 6, y: y(v) + 3.5, "text-anchor": "end" }, ctx.fmtAxis(v)));
    }
    const placed = [];
    for (const idx of tickOrder(buckets, W - Lpad - Rpad, grain, nowIdx)) {
      const label = String(buckets[idx]?.axis || "");
      if (!label) continue;
      const wEst = [...label].reduce((a, ch) => a + (ch.charCodeAt(0) > 255 ? 10 : 5.6), 0);
      const cx = Math.min(W - wEst / 2, Math.max(Lpad - 6 + wEst / 2, x(idx)));
      if (placed.some(p => Math.abs(p[0] - cx) < (p[1] + wEst) / 2 + 16)) continue;
      placed.push([cx, wEst]);
      nodes.push(el("text", { class: `u-axis${nowIdx === idx ? " now" : ""}`, x: cx, y: Hh - 5, "text-anchor": "middle" }, label));
    }
    if (midnightIdx != null && midnightIdx > 0 && midnightIdx < n) {
      const mx = bars ? x(midnightIdx) - band / 2 : x(midnightIdx);
      nodes.push(el("line", { class: "u-ref-line", x1: mx, x2: mx, y1: Tpad - 8, y2: base }),
        el("text", { class: "u-axis u-halo", x: mx + 4, y: Tpad - 2 }, L("今日 00:00", "Today 00:00")));
    }
    if (nowIdx != null && nowIdx >= 0 && nowIdx < n) {
      const nx = bars ? x(nowIdx) + band / 2 : x(nowIdx);
      nodes.push(el("line", { class: "u-ref-line", x1: nx, x2: nx, y1: Tpad - 8, y2: base }),
        el("text", { class: "u-axis now u-halo", x: nx + 4, y: Tpad - 2 }, buckets[nowIdx]?.axis || `${pad2(new Date().getHours())}:00`));
    }
  }

  function drawBars(nodes, barNodes, geo, ctx) {
    const { Tpad, base, band, x, sc } = geo;
    const barW = Math.max(1, Math.min(band * (band > 8 ? 0.66 : 0.8), 34));
    const stack = [...ctx.items].sort((a, b) => a.order - b.order);
    ctx.buckets.forEach((b, i) => {
      if (!b.has) return;
      const parts = stack.map(s => [s, (base - Tpad) * (s.values[i] || 0) / sc.top]).filter(p => p[1] > 0.05);
      let bottom = base;
      parts.forEach(([s, barH], k) => {
        const topMost = k === parts.length - 1, gap = !topMost && barH > 2.5 ? 1.5 : 0;
        const x0 = x(i) - barW / 2, y0 = bottom - barH + gap, hh = Math.max(0.5, barH - gap);
        const path = el("path", { class: "u-seg-bar", "data-id": s.id, d: roundedBarPath(x0, y0, barW, hh, topMost ? Math.min(4, barW / 2, hh) : 0) });
        setTone(path, s);
        barNodes.push(path);
        nodes.push(path);
        bottom -= barH;
      });
    });
  }

  function drawLines(nodes, seriesNodes, totalNodes, geo, ctx) {
    const { x, y, base, validIndices, first, last, totalMode } = geo;
    const ptsFor = vals => validIndices.map(i => [x(i), y(vals[i] || 0)]);
    const areaFor = vals => {
      const pts = ptsFor(vals);
      return pts.length ? `${monotone(pts)}L${x(last)},${base}L${x(first)},${base}Z` : "";
    };
    if (totalMode && validIndices.length > 0) {
      const gTotal = el("g", { class: "u-total" },
        el("path", { d: areaFor(ctx.totals), fill: "url(#u-g-total)" }),
        el("path", { class: "u-total-line", d: monotone(ptsFor(ctx.totals)) }));
      if (validIndices.length === 1) gTotal.append(el("circle", { class: "u-total-dot", cx: x(first), cy: y(ctx.totals[first]), r: 4 }));
      totalNodes.push(gTotal);
      nodes.push(gTotal);
    }
    const shown = drawnSet(ctx.items, ctx.mode, ctx.pinned, ctx.hover);
    for (const s of [...ctx.items].reverse()) {
      if (!shown.has(s.id)) continue;
      const gSeries = setTone(el("g", { class: "u-series", "data-id": s.id }), s);
      if (validIndices.length > 0) {
        gSeries.append(
          el("path", { class: "ar", d: areaFor(s.values), fill: `url(#u-g-${s.gid})` }),
          el("path", { class: "ln", d: monotone(ptsFor(s.values)), "stroke-dasharray": s.dash ? "5 4" : null }));
        if (validIndices.length === 1) gSeries.append(el("circle", { class: "dot-pt", cx: x(first), cy: y(s.values[first]), r: 3 }));
      }
      seriesNodes.push(gSeries);
      nodes.push(gSeries);
    }
  }

  function drawMeanAndPeak(nodes, totalNodes, geo, ctx) {
    const { W, Lpad, Rpad, x, y, validIndices, bars } = geo;
    const activeVals = validIndices.map(i => ctx.totals[i]);
    const peak = validIndices[activeVals.indexOf(Math.max(...activeVals))];
    const mean = activeVals.reduce((a, b) => a + b, 0) / activeVals.length;
    const g = el("g", { class: "u-total", "pointer-events": "none" },
      el("line", { class: "u-mean-line", x1: Lpad, x2: W - Rpad, y1: y(mean), y2: y(mean) }),
      el("text", { class: "u-axis u-halo", x: W - Rpad, y: y(mean) - 5, "text-anchor": "end" }, L(`均值 ${ctx.fmt(mean)}`, `Avg ${ctx.fmt(mean)}`)));
    const label = ctx.fmt(ctx.totals[peak] || 0), bw = label.length * 6.4 + 12;
    const bx = Math.min(W - bw, Math.max(Lpad, x(peak) - bw / 2)), by = Math.max(0, y(ctx.totals[peak] || 0) - 25);
    if (!bars) g.append(el("circle", { class: "u-peak-dot", cx: x(peak), cy: y(ctx.totals[peak] || 0), r: 4 }));
    g.append(el("rect", { class: "u-peak-box", x: bx, y: by, width: bw, height: 17, rx: 5 }),
      el("text", { class: "u-peak-text", x: bx + bw / 2, y: by + 12, "text-anchor": "middle" }, label));
    totalNodes.push(g);
    nodes.push(g);
  }

  function drawTrend(ctx) {
    const { plot, svg, tip, items, buckets, totals, mode, shape, metric, pinned, hover, title } = ctx;
    const W = Math.max(240, plot.clientWidth || 640), Hh = 248, Lpad = 42, Rpad = 10, Tpad = 24, Bpad = 22;
    const n = buckets.length;
    if (!n) { svg.replaceChildren(); return null; }

    const validIndices = buckets.map((b, i) => b.has ? i : -1).filter(i => i >= 0);
    const first = validIndices[0] ?? 0, last = validIndices.at(-1) ?? n - 1;
    const bars = shape === "bar", totalMode = bars || mode === "total";
    const { validPinned, hasPinned } = activeSelection(items, pinned, hover);
    const scaleItems = !totalMode && hasPinned ? items.filter(s => validPinned.has(s.id)) : items;
    const yMax = totalMode ? Math.max(0, ...totals) : Math.max(0, ...(scaleItems.length ? scaleItems : items).flatMap(s => s.values));
    const sc = niceScale(yMax, metric === "cost");
    const inset = n <= 7 ? 14 : 4, band = (W - Lpad - Rpad) / n, base = Hh - Bpad;
    const x = bars ? i => Lpad + band * (i + 0.5)
      : n === 1 ? () => Lpad + (W - Lpad - Rpad) / 2 : i => Lpad + inset + i * (W - Lpad - Rpad - inset * 2) / (n - 1);
    const y = v => Tpad + (1 - v / sc.top) * (Hh - Tpad - Bpad);
    const step = n > 1 ? x(1) - x(0) : W - Lpad - Rpad;
    const at = px => Math.max(0, Math.min(n - 1, bars ? Math.floor((px - Lpad) / band) : Math.round((px - x(0)) / step)));

    plot.classList.toggle("mode-series", !totalMode);
    plot.classList.toggle("mode-total", totalMode);
    const defs = el("defs");
    buildGradients(defs, items, totals, y, Tpad, base);
    const bandLayer = el("g", { id: "u-band" }), crossLayer = el("g", { id: "u-cross" });
    const nodes = [defs, bandLayer], seriesNodes = [], barNodes = [], totalNodes = [];
    const geo = { W, Hh, Lpad, Rpad, Tpad, base, band, bars, totalMode, x, y, sc, validIndices, first, last, totals, at, svg, crossLayer, bandLayer, tip, seriesNodes, barNodes, totalNodes };

    const emptyData = !totals.some(v => v > 0);
    if (!emptyData && bars) drawBars(nodes, barNodes, geo, ctx);
    else if (!emptyData) drawLines(nodes, seriesNodes, totalNodes, geo, ctx);
    if (!emptyData && totalMode && validIndices.length > 0) drawMeanAndPeak(nodes, totalNodes, geo, ctx);
    if (emptyData) nodes.push(el("text", { class: "u-axis", x: Lpad + (W - Lpad - Rpad) / 2, y: Tpad + (base - Tpad) / 2, "text-anchor": "middle" }, L("暂无数据", "No data")));
    drawAxesAndGuides(nodes, geo, ctx);
    nodes.push(crossLayer);

    svg.setAttribute("viewBox", `0 0 ${W} ${Hh}`);
    svg.setAttribute("aria-label", title);
    svg.replaceChildren(...nodes);
    return geo;
  }

  function drawCursor(geo, ctx) {
    if (!geo) return;
    const { cursor, buckets, items, grain, mode, pinned, hover, dim, fmt, hitExact, hitOfBucket } = ctx;
    const { x, y, Tpad, base, totals, W, band, bars, crossLayer, bandLayer, tip } = geo;
    const bucket = cursor == null ? null : buckets[cursor];
    ctx.plot.classList.toggle("drill", grain === "day" && Boolean(bucket?.has));
    if (!bucket || !bucket.has) { crossLayer.replaceChildren(); bandLayer.replaceChildren(); tip.hidden = true; return; }

    const { validPinned, validHover, hasPinned } = activeSelection(items, pinned, hover);
    const shown = drawnSet(items, mode, validPinned, validHover);
    if (bars) {
      bandLayer.replaceChildren(el("rect", { class: "u-cross-band", x: x(cursor) - band / 2, y: Tpad, width: band, height: base - Tpad, rx: 4 }));
      crossLayer.replaceChildren();
    } else {
      const marks = [el("line", { class: "u-cross-line", x1: x(cursor), x2: x(cursor), y1: Tpad, y2: base })];
      for (const s of items) {
        if (hasPinned ? (validPinned.has(s.id) || validHover === s.id) : (validHover ? validHover === s.id : shown.has(s.id))) {
          marks.push(setTone(el("circle", { class: "u-cross-dot", cx: x(cursor), cy: y(s.values[cursor] || 0), r: 4 }), s));
        }
      }
      if (mode === "total") marks.push(el("circle", { class: "u-cross-dot u-total-tone", cx: x(cursor), cy: y(totals[cursor] || 0), r: 4 }));
      crossLayer.replaceChildren(...marks);
      bandLayer.replaceChildren();
    }

    const wasHidden = tip.hidden;
    const rows = items.filter(s => (s.values[cursor] || 0) > 0).sort((a, b) => (b.values[cursor] || 0) - (a.values[cursor] || 0));
    tip.className = `u-tip${dim === "model" ? " u-mono" : ""}`;
    tip.replaceChildren(
      h("div.u-tip-time", {}, bucket.label || ""),
      h("div.u-tip-row.sum", {}, h("i.u-key.u-total-tone"), h("span", {}, L("总计", "Total")), h("b", {}, fmt(totals[cursor] || 0))),
      h("div.u-tip-row.rate", {}, h("i"), h("span", {}, L("命中率", "Hit rate")), h("b", {}, hitExact(hitOfBucket(bucket)))),
      ...rows.map(s => {
        const isEm = hasPinned ? validPinned.has(s.id) : validHover === s.id;
        const isDim = hasPinned ? (!validPinned.has(s.id) && validHover !== s.id) : (validHover != null && validHover !== s.id);
        const keyLine = setTone(h(`i.u-key${s.dash ? ".dash" : ""}`), s);
        return h(`div.u-tip-row${isEm ? ".em" : isDim ? ".dimmed" : ""}`, {}, keyLine, h("span.u-tip-name", {}, s.name), h("b", {}, fmt(s.values[cursor] || 0)));
      }));
    const gap = bars ? band / 2 + 8 : 14;
    const cx = x(cursor);
    if (wasHidden) tip.setAttribute?.("data-snap", "1");
    // Clamp against actual tooltip width (isolated via contain: layout paint on .u-tip) and move via translate3d.
    const tipW = Math.min(W, tip.offsetWidth || 168);
    const rawLeft = cx > W / 2 ? cx - gap - tipW : cx + gap;
    const left = W <= tipW + 8 ? Math.max(0, (W - tipW) / 2) : Math.max(0, Math.min(W - tipW, rawLeft));
    tip.style.transform = `translate3d(${left.toFixed(1)}px, 0, 0)`;
    tip.hidden = false;
    if (wasHidden && typeof requestAnimationFrame === "function") {
      requestAnimationFrame(() => tip.removeAttribute?.("data-snap"));
    }
  }

  function applyHighlight({ geo, items, pinned, hover, shape, mode, dim, total, metricLabel, fmt, chips, rankRows, arcs, donutCenter }) {
    const { validPinned, validHover, hasPinned } = activeSelection(items, pinned, hover);
    const shown = drawnSet(items, mode, validPinned, validHover);
    const classify = id => hasPinned
      ? { hi: validPinned.has(id), preview: !validPinned.has(id) && validHover === id, lo: !validPinned.has(id) && validHover !== id, pressed: validPinned.has(id) }
      : { hi: validHover === id, preview: false, lo: validHover != null && validHover !== id, pressed: false };

    for (const g of geo?.seriesNodes || []) {
      const id = g.getAttribute?.("data-id"), st = classify(id);
      g.classList.toggle("is-hi", st.hi);
      g.classList.toggle("is-preview", st.preview);
      g.classList.toggle("is-lo", st.lo && shown.has(id));
      g.classList.toggle("is-off", !st.hi && !st.preview && !shown.has(id));
      if (st.hi && geo.svg && geo.crossLayer) geo.svg.insertBefore?.(g, geo.crossLayer);
    }
    for (const bar of geo?.barNodes || []) {
      const st = classify(bar.getAttribute?.("data-id"));
      bar.classList.toggle("is-lo", st.lo);
      bar.classList.toggle("is-preview", st.preview);
    }
    for (const tot of geo?.totalNodes || []) tot.classList.toggle("is-lo", (hasPinned || validHover != null) && shape === "line");
    for (const node of [...(chips || []), ...(rankRows || []), ...(arcs || [])]) {
      const id = node.getAttribute?.("data-id") ?? node.dataset?.id, st = classify(id);
      node.classList.toggle("is-hi", st.hi);
      node.classList.toggle("is-preview", st.preview);
      node.classList.toggle("is-lo", st.lo);
      if (node.getAttribute?.("aria-pressed") != null) node.setAttribute("aria-pressed", String(st.pressed));
      if (chips?.includes(node)) node.classList.toggle("is-off", shape === "line" && !shown.has(id));
    }
    if (!donutCenter) return;
    const sel = hasPinned ? items.filter(s => validPinned.has(s.id)) : (validHover ? items.filter(s => s.id === validHover) : []);
    const pctStr = v => `${(total > 0 ? v / total * 100 : 0).toFixed(1)}%`;
    if (sel.length === 1) {
      donutCenter.replaceChildren(h(`span${dim === "model" ? ".model" : ""}`, {}, sel[0].name), h("strong", {}, fmt(sel[0].value)), h("span", {}, pctStr(sel[0].value)));
    } else if (sel.length >= 2) {
      const sumVal = sel.reduce((a, s) => a + s.value, 0);
      donutCenter.replaceChildren(h("span", {}, L(`已选 ${sel.length} 项`, `${sel.length} selected`)), h("strong", {}, fmt(sumVal)), h("span", {}, pctStr(sumVal)));
    } else {
      donutCenter.replaceChildren(h("span", {}, L("总计", "Total")), h("strong", {}, fmt(total)), h("span", {}, metricLabel));
    }
  }

  function drawDonut(container, items, total, fmt) {
    const radius = 62, C = 2 * Math.PI * radius, gap = 2, arcs = [];
    let at = 0;
    const svg = el("svg", { viewBox: "0 0 148 148", role: "img", "aria-label": L("占比", "Share") });
    if (total > 0) {
      // Group holds rotate(-90 74 74) so CSS transform on each .u-arc translates cleanly without overriding rotation.
      const ring = el("g", { transform: "rotate(-90 74 74)" });
      for (const s of items) {
        if (!(s.value > 0)) continue;
        const len = s.value / total * C;
        const theta = ((at + len / 2) / C) * 2 * Math.PI;
        const arc = setTone(el("circle", {
          class: "u-arc", "data-id": s.id, cx: 74, cy: 74, r: radius,
          "stroke-dasharray": `${Math.max(0.8, len - gap).toFixed(2)} ${C.toFixed(2)}`,
          "stroke-dashoffset": (-at).toFixed(2),
        }, el("title", {}, `${s.name} ${fmt(s.value)}`)), s);
        arc.style?.setProperty?.("--dx", `${(Math.cos(theta) * 3).toFixed(2)}px`);
        arc.style?.setProperty?.("--dy", `${(Math.sin(theta) * 3).toFixed(2)}px`);
        arcs.push(arc);
        ring.append(arc);
        at += len;
      }
      svg.append(ring);
    } else {
      svg.append(el("circle", { class: "u-donut-ring-empty", cx: 74, cy: 74, r: radius }));
    }
    const center = h("div.u-donut-center", { id: "u-donut-center" });
    container.replaceChildren(svg, center);
    return { arcs, center };
  }

  return { monotone, niceScale, sparkSvg, drawTrend, drawCursor, applyHighlight, drawDonut };
})();
