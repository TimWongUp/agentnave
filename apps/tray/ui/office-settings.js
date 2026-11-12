"use strict";
// Settings borrows the office's content area; the office and its player stay mounted.
{
  let frame = null, receiving = false, loaded = false, pendingSection = null;
  const header = document.querySelector("header.bar");
  const origin = location.origin === "null" ? "*" : location.origin;
  const send = data => frame?.contentWindow?.postMessage({ type: "agentnave:settings", ...data }, origin);
  const frameVisible = () => Boolean(frame && !frame.hidden && !document.hidden && document.documentElement?.dataset?.windowHidden !== "true");
  const syncVisible = () => send({ action: "visible", visible: frameVisible() });
  const refreshVoice = () => AN.voice?.refresh();
  function targetOf(path) {
    const url = new URL(path, location.href);
    if (url.protocol !== location.protocol || url.host !== location.host) return null;
    // Static previews carry their page and workspace in the query.
    if (url.pathname.endsWith("/preview.html") && ["settings", "chat"].includes(url.searchParams.get("page"))) {
      url.pathname = `/${url.searchParams.get("page")}/${url.searchParams.get("page") === "chat" ? encodeURIComponent(url.searchParams.get("group") || "") : ""}`;
    }
    return url;
  }
  function highlight(open) {
    for (const link of header.querySelectorAll("nav a")) {
      const settings = targetOf(link.href)?.pathname.startsWith("/settings/");
      if (settings === open) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  }
  function open(path = "/settings/") {
    const target = targetOf(path);
    if (!target?.pathname.startsWith("/settings/")) return false;
    if (target.hash) pendingSection = target.hash;
    if (!frame) {
      frame = AN.h("iframe.office-settings", { title: AN.L("设置", "Settings"), hidden: true });
      const source = new URL(AN.href(target.pathname + target.search + target.hash), location.href);
      source.searchParams.set("embedded", "1");
      frame.src = source.href;
      document.body.append(frame);
    } else if (loaded && pendingSection) { send({ action: "section", hash: pendingSection }); pendingSection = null; }
    frame.hidden = false;
    document.getElementById("app").inert = true;
    highlight(true);
    syncVisible();
    return true;
  }
  function close() {
    if (!frame || frame.hidden) return;
    send({ action: "visible", visible: false });
    frame.hidden = true;
    document.getElementById("app").inert = false;
    highlight(false);
    refreshVoice();
  }
  function navigate(path) {
    const target = targetOf(path);
    if (!target) return false;
    if (target.pathname.startsWith("/settings/")) return open(path);
    // The update dialog opens over whichever page is showing; settings stays where it is.
    if (target.pathname.startsWith("/chat/") && target.searchParams.has("check-update")) { AN.updateFlow?.(); return true; }
    close();
    if (target.pathname.startsWith("/chat/")) {
      if (target.pathname === "/chat/" && !target.hash && !target.search) return true;
      if (typeof go === "function") go(target.pathname + target.search + target.hash);
      return true;
    }
    location.assign(target.href);
    return true;
  }
  document.addEventListener("click", event => {
    const link = event.target.closest("a[href]");
    if (!link || event.button) return;
    const target = targetOf(link.href);
    if (!target || !["/settings/", "/chat/"].some(prefix => target.pathname.startsWith(prefix))) return;
    event.preventDefault();
    navigate(link.href);
  }, true);
  window.addEventListener("message", event => {
    if (event.source !== frame?.contentWindow || event.data?.type !== "agentnave:settings") return;
    const data = event.data;
    if (data.action === "navigate") navigate(data.path);
    else if (data.action === "changed") { refreshVoice(); AN.art?.loadVideoPacks?.(); }
    else if (data.action === "loading") loaded = false;
    else if (data.action === "ready") {
      loaded = true;
      if (pendingSection) { send({ action: "section", hash: pendingSection }); pendingSection = null; }
      if (typeof data.language === "string") AN.setLanguage?.(data.language);
      if (typeof data.choice === "string") document.documentElement.dataset.lang = data.choice;
      send({ action: "preferences", general: window.ANAppearance?.preferences() });
      syncVisible();
      refreshVoice();
    } else if (data.action === "preferences") {
      receiving = true;
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: data.general }));
      receiving = false;
    } else if (data.action === "model-names") {
      receiving = true;
      AN.setFullModelNames(data.full === true);
      receiving = false;
    }
  });
  document.addEventListener("visibilitychange", syncVisible);
  if (typeof MutationObserver === "function") {
    new MutationObserver(syncVisible).observe(document.documentElement, { attributes: true, attributeFilter: ["data-window-hidden"] });
  }
  window.addEventListener("agentnave:preferences", event => {
    if (!receiving) send({ action: "preferences", general: event.detail });
  });
  window.addEventListener("agentnave:model-names", event => {
    if (!receiving) send({ action: "model-names", full: event.detail.full });
  });
  AN.officeSettings = { open, close };
  AN.navigate = navigate;
}
