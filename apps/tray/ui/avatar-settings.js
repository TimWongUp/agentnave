"use strict";
(function () {
  const { h, art } = AN;
  const root = document.getElementById("avatar-settings");
  const status = h("p", { role: "status" });
  // Upload order the tray expects: s and m carry every frame, heads (h) only the seven moods.
  const FRAMES = [
    ...["s", "m"].flatMap((size) => [...art.STATES, ...art.EXTRA].map((state) => `${size}/${state}`)),
    ...art.STATES.map((state) => `h/${state}`),
  ];
  let busy = false, replaced = {};
  const refreshers = [];
  // A chosen folder holds s/, m/ and h/ directly, or one level further down.
  async function pack(files) {
    const found = new Map();
    for (const file of files) {
      const parts = file.webkitRelativePath.split("/");
      const frame = parts.slice(-2).join("/").replace(/\.webp$/i, "");
      if (FRAMES.includes(frame) && /\.webp$/i.test(file.name)) found.set(frame, file);
    }
    const missing = FRAMES.filter((frame) => !found.has(frame));
    if (missing.length)
      throw new Error(`缺少 ${missing.length} 张图：${missing.slice(0, 4).map((f) => `${f}.webp`).join("、")}${missing.length > 4 ? " 等" : ""}`);
    const chunks = [];
    for (const frame of FRAMES) {
      // Decode each frame once, so a broken or wrongly sized image never replaces a working set.
      const edge = { s: 266, m: 320, h: 96 }[frame[0]];
      const bitmap = await createImageBitmap(found.get(frame)).catch(() => null);
      const fits = bitmap && bitmap.width === edge && bitmap.height === edge;
      bitmap?.close();
      if (!fits) throw new Error(`${frame}.webp 必须是能打开的 ${edge}×${edge} WebP 图片`);
      const bytes = new Uint8Array(await found.get(frame).arrayBuffer());
      const head = new Uint8Array(4);
      new DataView(head.buffer).setUint32(0, bytes.length);
      chunks.push(head, bytes);
    }
    return new Blob(chunks);
  }
  async function send(family, body) {
    const response = await fetch(`/app/characters/${family}`, {
      method: "POST", headers: { "X-AgentNave-Action": "1" }, body,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || "保存小人失败");
    replaced = data;
    await art.charactersChanged();
  }
  function editor(family, label) {
    const row = h("div.avatar-editor");
    const face = h("span.char-preview");
    const input = h("input", { type: "file", webkitdirectory: "", multiple: true, hidden: true, "aria-label": `选择${label}小人文件夹` });
    const upload = h("button", { type: "button" }, "更换小人");
    const reset = h("button", { type: "button" }, "恢复默认");
    function update() {
      face.replaceChildren(art.figure(art.character("", family), "m"));
      upload.disabled = busy;
      reset.disabled = busy || !replaced[family];
    }
    async function run(action, done) {
      busy = true; refreshers.forEach((f) => f());
      try { await action(); status.textContent = `${done}，办公室和菜单栏随后更新`; }
      catch (error) { status.textContent = error.message; }
      finally { busy = false; input.value = ""; refreshers.forEach((f) => f()); }
    }
    upload.addEventListener("click", () => input.click());
    input.addEventListener("change", () => { if (input.files.length) run(async () => send(family, await pack(input.files)), `${label} 的小人已更换`); });
    reset.addEventListener("click", () => run(() => send(family, new Blob([])), `${label} 已恢复默认小人`));
    refreshers.push(update);
    update();
    row.append(face, h("div.avatar-side", {},
      h("div.avatar-name", {}, Avatars.create("family", family, ""), h("span.avatar-label", {}, label)),
      h("div.avatar-actions", {}, upload, reset)), input);
    return row;
  }
  const grid = h("div.avatar-grid");
  root.append(status, h("h2", {}, "模型家族"), grid);
  fetch("/app/characters", { cache: "no-store" }).then((r) => (r.ok ? r.json() : {})).catch(() => ({})).then((data) => {
    replaced = data;
    for (const [id, [label]] of Object.entries(Avatars.families)) grid.append(editor(id, label));
  });
})();
