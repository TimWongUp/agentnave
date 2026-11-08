"use strict";
// The desk laptop: a small 3D model seen from behind, in MacBook proportions. The base stays put and the
// lid swings on its hinge, with the CLI mark printed on it at the desks. It is drawn only while it moves.
(function () {
  const W = 1563, D = 1103, T = 34, LID = 1060, LID_T = 18, R = 42, OPEN = 108, MOVE_MS = 420;
  const SCALE = 0.3, CX = 400, CY = 300, TOP = 120, VIEW_W = 800, VIEW_H = 600;
  // every desk is painted at the same 14° view, so seats and bosses share one camera; only the height differs
  const VIEW = { tilt: 14, depth: 11100 };
  const BOSS_TOPS = {
    plain: 34.3,
    agy: 33.0,
    claude: 33.2,
    codebuddy: 35.6,
    codex: 33.0,
    deepseek: 33.0,
    devin: 35.1,
    grok: 35.9,
    hermes: 35.3,
    pi: 33.5,
    zcode: 35.1,
  };
  const SILVER = [206, 208, 214];
  // boss laptops lean towards their desk's colour
  const TONES = {
    agy: [162, 163, 186], claude: [174, 148, 139], codebuddy: [202, 206, 217], codex: [218, 213, 211], deepseek: [168, 172, 186], devin: [105, 141, 153],
    grok: [146, 143, 151], hermes: [197, 175, 152], pi: [214, 192, 166], zcode: [150, 148, 151],
  };
  const rad = (d) => (d * Math.PI) / 180;
  const camera = ({ tilt, depth }) => (p) => {
    const t = rad(tilt);
    const y = p.y * Math.cos(t) - p.z * Math.sin(t);
    const z = -(p.y * Math.sin(t) + p.z * Math.cos(t));
    const k = depth / (depth - z);
    return [CX + (W / 2 - p.x) * SCALE * k, CY + y * SCALE * k, z];
  };
  const rounded = (w, h) => {
    const out = [];
    const arc = (cx, cy, from) => {
      for (let i = 0; i <= 6; i++) {
        const a = rad(from + 15 * i);
        out.push([cx + R * Math.cos(a), cy + R * Math.sin(a)]);
      }
    };
    arc(w - R, R, -90); arc(w - R, h - R, 0); arc(R, h - R, 90); arc(R, R, 180);
    return out;
  };
  const DECK = rounded(W, D), LID_EDGE = rounded(W, LID);
  const deckAt = (a, b, y) => ({ x: a, y, z: b });
  const lidAt = (deg, u, r, n) => {
    const a = rad(deg);
    return { x: u, y: -r * Math.sin(a) - n * Math.cos(a), z: r * Math.cos(a) - n * Math.sin(a) };
  };
  const area = (p) => p.reduce((s, [a, b], i) => {
    const [c, d] = p[(i + 1) % p.length];
    return s + a * d - c * b;
  }, 0);
  const shade = (c, l) => `rgb(${c.map((v) => Math.round(v * l)).join(",")})`;
  const path = (p, fill, line = "") => `<path d="M${p.map((v) => `${v[0].toFixed(2)} ${v[1].toFixed(2)}`).join("L")}Z" fill="${fill}"${line && ` stroke="${line}" stroke-width=".5"`}/>`;
  function walls(project, outline, at, depth, colour, facing) {
    const faces = [];
    outline.forEach(([a, b], i) => {
      const [c, d] = outline[(i + 1) % outline.length];
      const quad = [at(a, b, 0), at(c, d, 0), at(c, d, depth), at(a, b, depth)].map(project);
      if (area(quad) * facing <= 0) return;
      const fill = shade(colour, 0.62 + 0.38 * Math.abs((c - a) / (Math.hypot(d - b, c - a) || 1)));
      faces.push({ z: quad.reduce((m, p) => m + p[2], 0), d: path(quad, fill, fill) });
    });
    return faces.sort((x, y) => x.z - y.z).map((f) => f.d).join("");
  }
  // a w×h box onto any quad (top left, top right, bottom right, bottom left)
  function matrix(w, h, [p0, p1, p2, p3]) {
    const sx = p0[0] - p1[0] + p2[0] - p3[0], sy = p0[1] - p1[1] + p2[1] - p3[1];
    const dx1 = p1[0] - p2[0], dx2 = p3[0] - p2[0], dy1 = p1[1] - p2[1], dy2 = p3[1] - p2[1];
    const den = dx1 * dy2 - dx2 * dy1;
    const g = (sx * dy2 - dx2 * sy) / den, k = (dx1 * sy - sx * dy1) / den;
    const a = p1[0] - p0[0] + g * p1[0], b = p3[0] - p0[0] + k * p3[0];
    const d = p1[1] - p0[1] + g * p1[1], e = p3[1] - p0[1] + k * p3[1];
    return `matrix3d(${a / w},${d / w},0,${g / w},${b / h},${e / h},0,${k / h},0,0,1,0,${p0[0]},${p0[1]},0,1)`;
  }
  let made = 0;
  function create(lid) {
    let project = camera(VIEW);
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", `0 ${-TOP} ${VIEW_W} ${VIEW_H}`);
    const el = document.createElement("span");
    el.className = "laptop";
    el.append(svg);
    if (lid) el.append(lid);
    const id = `laptop-${++made}`;
    let colour = SILVER, deg = null, run = null;
    function draw(at) {
      if (!el.clientWidth) { run = null; return; }
      deg = at;
      const edge = (u, r, n) => lidAt(at, u, r, n);
      const grad = `<defs><linearGradient id="${id}" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${shade(colour, 1.14)}"/><stop offset="1" stop-color="${shade(colour, 0.92)}"/></linearGradient></defs>`;
      svg.innerHTML = grad
        + walls(project, DECK, deckAt, T, colour.map((v) => v * 0.72), -1)
        + path(DECK.map(([a, b]) => project(deckAt(a, b, 0))), shade(colour, 0.62))
        + path(LID_EDGE.map(([a, b]) => project(edge(a, b, -5))), "#2a2b30")
        + walls(project, LID_EDGE, (a, b, n) => edge(a, b, n), LID_T, colour.map((v) => v * 0.8), 1)
        + path(LID_EDGE.map(([a, b]) => project(edge(a, b, LID_T))), `url(#${id})`);
      const size = el.clientWidth * 0.2;
      if (!lid || !size) return;
      const unit = el.clientWidth / VIEW_W;
      const corners = [[0.655, 0.78], [0.345, 0.78], [0.345, 0.32], [0.655, 0.32]]
        .map(([u, v]) => project(edge(W * u, LID * v, LID_T + 1)))
        .map(([x, y]) => [x * unit, (y + TOP) * unit]);
      lid.style.width = lid.style.height = `${size}px`;
      lid.style.transform = matrix(size, size, corners);
    }
    function sync() {
      const target = el.closest("[data-mood]")?.dataset.mood === "working" ? OPEN : 0;
      if (!el.clientWidth) { run = null; return; }
      if (deg === null) return draw(target);
      if (run === target || (run === null && deg === target)) return;
      const from = deg, start = performance.now();
      run = target;
      const step = (now) => {
        if (run !== target) return;
        const t = Math.min(1, (now - start) / MOVE_MS);
        draw(from + (target - from) * t * t * (3 - 2 * t));
        if (t < 1) requestAnimationFrame(step);
        else run = null;
      };
      requestAnimationFrame(step);
    }
    function dress(set) {
      colour = TONES[set] || SILVER;
      el.style.top = `${BOSS_TOPS[set] || BOSS_TOPS.plain}%`;
      if (deg !== null) draw(deg);
    }
    el.laptop = { sync, dress };
    new ResizeObserver(() => {
      if (!el.clientWidth) { run = null; return; }
      if (deg !== null) draw(deg);
      sync();
    }).observe(el);
    return el;
  }
  let queued = 0;
  new MutationObserver((records) => {
    const hasLaptop = (node) => node.nodeType === 1 && (node.matches(".laptop") || node.querySelector(".laptop"));
    if (!records.some((m) => m.type === "attributes" || [...m.addedNodes].some(hasLaptop))) return;
    queued ||= requestAnimationFrame(() => {
      queued = 0;
      for (const el of document.querySelectorAll(".laptop")) el.laptop.sync();
    });
  }).observe(document.documentElement, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-mood"] });
  window.Laptop = { create };
})();
