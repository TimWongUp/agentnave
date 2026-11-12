"use strict";
// A section still reading its data shows a walking character instead of a half-empty page. Each
// section names the box its data fills; the character leaves once that box has content, the page
// reports an error, or the wait runs out.
{
  const { h, L, art } = AN;
  const CONTENT = {
    general: "general-prefs", hosts: "host-cards", members: "cli-cards", orchestration: "orchestration-effect",
    avatars: "avatar-settings", voice: "voice-settings", storage: "storage-settings", advanced: "skill-list",
  };
  // Characters whose walk needs no separate parts.
  const CHARACTERS = ["claude", "default", "doubao", "kimi", "minimax", "muse"];
  const GIVE_UP_MS = 12000, SHOW_AFTER_MS = 150;
  const filled = (box) => box.childElementCount > 0 || box.textContent.trim() !== "";

  function show(tab) {
    const pane = document.getElementById(`pane-${tab}`), box = document.getElementById(CONTENT[tab]);
    if (!pane || !box || filled(box) || pane.classList.contains("is-loading")) return;
    const name = CHARACTERS[Math.floor(Math.random() * CHARACTERS.length)];
    const frame = (n) => h("img", { src: art.url(`s/${name}/walk-${n}.webp`), alt: "", draggable: "false" });
    const node = h("div.pane-loader", { role: "status", "aria-label": L("正在读取…", "Loading…") },
      h("div.loader-walk", {}, h("div.loader-fig", {}, frame(1), frame(2)), h("i.loader-shadow"),
        h("div.loader-dots", { "aria-hidden": "true" }, h("i"), h("i"), h("i"))));
    pane.classList.add("is-loading");
    pane.append(node);
    const notice = document.getElementById("notice");
    const watcher = new MutationObserver(() => { if (filled(box) || notice?.classList.contains("error")) done(); });
    // A fast answer shows no character at all.
    const reveal = setTimeout(() => node.classList.add("shown"), SHOW_AFTER_MS);
    const giveUp = setTimeout(done, GIVE_UP_MS);
    function done() {
      watcher.disconnect();
      clearTimeout(reveal);
      clearTimeout(giveUp);
      node.remove();
      pane.classList.remove("is-loading");
    }
    watcher.observe(box, { childList: true, subtree: true, characterData: true });
    if (notice) watcher.observe(notice, { attributes: true, attributeFilter: ["class"] });
  }
  window.AgentNavePaneLoader = { show };
}
