"use strict";
// A section still reading its data shows a walking character instead of a half-empty page. Each
// section names the box its data fills. A section that answers within WAIT_MS never shows the
// character. Otherwise the character walks down from the upper middle and stays at least SHOW_MS,
// so it never flashes by; it leaves once that box has content or the page reports an error,
// fading out while the section fades in after it. GIVE_UP_MS only keeps a section that never
// answers from hiding its page for good.
{
  const { h, L, art } = AN;
  const CONTENT = {
    general: "general-main", hosts: "host-cards", members: "cli-cards", orchestration: "rule-files",
    avatars: "avatar-settings", voice: "voice-settings", storage: "storage-settings", advanced: "skill-list",
  };
  const CHARACTER = "deepseek";
  // The walk frames come apart into layers; the tail sits behind the body, the rest in front.
  const LAYERS = ["tail-", "", "lop-l-", "lop-r-", "tuft-"];
  const WAIT_MS = 150, SHOW_MS = 400, GIVE_UP_MS = 60000, FADE_MS = 260;
  const waiting = new Set(); // panes still within WAIT_MS
  const filled = (box) => box.childElementCount > 0 || box.textContent.trim() !== "";
  const frame = (n) => h(`span.loader-frame.f${n}`, {},
    ...LAYERS.map((layer) => h("img", { src: art.url(`s/${CHARACTER}/${layer}walk-${n}.webp`), alt: "", draggable: "false" })));

  function show(tab) {
    const pane = document.getElementById(`pane-${tab}`), box = document.getElementById(CONTENT[tab]);
    if (!pane || !box || filled(box) || waiting.has(pane) || pane.classList.contains("is-loading")) return;
    const notice = document.getElementById("notice");
    const arrived = () => filled(box) || notice?.classList.contains("error");
    waiting.add(pane);
    setTimeout(() => {
      waiting.delete(pane);
      if (!arrived()) mount(pane, box, notice, arrived);
    }, WAIT_MS);
  }

  function mount(pane, box, notice, arrived) {
    const node = h("div.pane-loader", { role: "status", "aria-label": L("正在读取…", "Loading…") },
      h("div.loader-walk", {}, h("div.loader-fig", {}, frame(1), frame(2)), h("i.loader-shadow"),
        h("div.loader-dots", { "aria-hidden": "true" }, h("i"), h("i"), h("i"))));
    pane.classList.remove("is-entering");
    pane.classList.add("is-loading");
    pane.append(node);
    setTimeout(() => node.classList.add("shown"), 20); // after the first paint, so it fades in
    let stayed = false, left = false;
    const watcher = new MutationObserver(() => { if (stayed && arrived()) leave(); });
    const stay = setTimeout(() => { stayed = true; if (arrived()) leave(); }, SHOW_MS);
    const giveUp = setTimeout(leave, GIVE_UP_MS);
    function leave() {
      if (left) return;
      left = true;
      watcher.disconnect();
      clearTimeout(stay);
      clearTimeout(giveUp);
      node.classList.remove("shown");
      setTimeout(() => {
        node.remove();
        pane.classList.remove("is-loading");
        pane.classList.add("is-entering");
        setTimeout(() => pane.classList.remove("is-entering"), 500);
      }, FADE_MS);
    }
    watcher.observe(box, { childList: true, subtree: true, characterData: true });
    if (notice) watcher.observe(notice, { attributes: true, attributeFilter: ["class"] });
  }
  window.AgentNavePaneLoader = { show };
}
