"use strict";
// Settings and Usage borrow the office's content area, one at a time; the office, its terminals and its player stay mounted.
{
  const pages = [
    { prefix: "/settings/", cls: "office-settings", zh: "设置", en: "Settings" },
    { prefix: "/usage/", cls: "office-usage", zh: "用量", en: "Usage" },
  ].map(page => ({ ...page, frame: null, loaded: false, language: "", pendingSection: null, pendingUninstall: false }));
  const settings = pages[0];
  let receiving = false;
  const header = document.querySelector("header.bar");
  const origin = location.origin === "null" ? "*" : location.origin;
  const send = (page, data) => page.frame?.contentWindow?.postMessage({ type: "agentnave:settings", ...data }, origin);
  const broadcast = (data, except) => pages.forEach(page => { if (page !== except) send(page, data); });
  const shown = page => Boolean(page.frame && !page.frame.hidden);
  const current = () => pages.find(shown) || null;
  const frameVisible = page => Boolean(shown(page) && !document.hidden && document.documentElement?.dataset?.windowHidden !== "true");
  const syncVisible = () => pages.forEach(page => send(page, { action: "visible", visible: frameVisible(page) }));
  const refreshVoice = () => AN.voice?.refresh();
  const pageOf = path => pages.find(page => path.startsWith(page.prefix));
  function targetOf(path) {
    const url = new URL(path, location.href);
    if (url.protocol !== location.protocol || url.host !== location.host) return null;
    // Static previews carry their page and workspace in the query.
    if (url.pathname.endsWith("/preview.html") && ["settings", "chat"].includes(url.searchParams.get("page"))) {
      url.pathname = `/${url.searchParams.get("page")}/${url.searchParams.get("page") === "chat" ? encodeURIComponent(url.searchParams.get("group") || "") : ""}`;
    }
    return url;
  }
  function highlight() {
    const active = current()?.prefix || "/chat/";
    for (const link of header.querySelectorAll("nav a")) {
      if (targetOf(link.href)?.pathname.startsWith(active)) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  }
  function deliverPending(page) {
    if (page.pendingSection) { send(page, { action: "section", hash: page.pendingSection }); page.pendingSection = null; }
    if (page.pendingUninstall) { send(page, { action: "uninstall" }); page.pendingUninstall = false; }
  }
  function hide(page) {
    if (!shown(page)) return;
    send(page, { action: "visible", visible: false });
    page.frame.hidden = true;
    if (page === settings) refreshVoice();
  }
  function open(path = "/settings/") {
    const target = targetOf(path), page = target && pageOf(target.pathname);
    if (!page) return false;
    if (target.hash) page.pendingSection = target.hash;
    pages.filter(other => other !== page).forEach(hide);
    if (!page.frame) {
      page.frame = AN.h(`iframe.${page.cls}`, { title: AN.L(page.zh, page.en), hidden: true });
      const source = new URL(AN.href(target.pathname + target.search + target.hash), location.href);
      source.searchParams.set("embedded", "1");
      page.frame.src = source.href;
      document.body.append(page.frame);
    } else {
      if (target.searchParams.has("uninstall")) page.pendingUninstall = true;
      if (page.loaded) {
        if (page.language && page.language !== document.documentElement.lang) {
          page.language = document.documentElement.lang;
          send(page, { action: "language", language: page.language });
        }
        deliverPending(page);
      }
    }
    page.frame.hidden = false;
    if (page.loaded) globalThis.ANMotion?.reveal?.(page.frame, { dy: 0, duration: 150 });
    document.getElementById("app").inert = true;
    highlight();
    syncVisible();
    return true;
  }
  function close() {
    const page = current();
    if (!page) return;
    hide(page);
    document.getElementById("app").inert = false;
    highlight();
  }
  function navigate(path) {
    const target = targetOf(path);
    if (!target) return false;
    if (pageOf(target.pathname)) return open(path);
    // The update dialog opens over whichever page is showing; the overlay stays where it is.
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
    if (!target || !(pageOf(target.pathname) || target.pathname.startsWith("/chat/"))) return;
    event.preventDefault();
    navigate(link.href);
  }, true);
  window.addEventListener("message", event => {
    const page = pages.find(item => item.frame && event.source === item.frame.contentWindow);
    if (!page || event.data?.type !== "agentnave:settings") return;
    const data = event.data;
    if (data.action === "navigate") navigate(data.path);
    else if (data.action === "changed") { refreshVoice(); AN.art?.loadVideoPacks?.(); }
    else if (data.action === "loading") page.loaded = false;
    else if (data.action === "ready") {
      const firstReady = !page.loaded;
      page.loaded = true;
      deliverPending(page);
      if (typeof data.language === "string") {
        page.language = data.language;
        if (page === settings) AN.setLanguage?.(data.language);
      }
      if (typeof data.choice === "string") document.documentElement.dataset.lang = data.choice;
      send(page, { action: "preferences", general: window.ANAppearance?.preferences() });
      syncVisible();
      if (firstReady && shown(page)) globalThis.ANMotion?.reveal?.(page.frame, { dy: 0, duration: 150 });
      refreshVoice();
    } else if (data.action === "preferences") {
      receiving = true;
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: data.general }));
      receiving = false;
      broadcast({ action: "preferences", general: data.general }, page);
    } else if (data.action === "model-names") {
      receiving = true;
      AN.setFullModelNames(data.full === true);
      receiving = false;
      broadcast({ action: "model-names", full: data.full === true }, page);
    }
  });
  document.addEventListener("visibilitychange", syncVisible);
  if (typeof MutationObserver === "function") {
    new MutationObserver(syncVisible).observe(document.documentElement, { attributes: true, attributeFilter: ["data-window-hidden"] });
  }
  window.addEventListener("agentnave:preferences", event => {
    if (!receiving) broadcast({ action: "preferences", general: event.detail });
  });
  window.addEventListener("agentnave:model-names", event => {
    if (!receiving) broadcast({ action: "model-names", full: event.detail.full });
  });
  AN.officeSettings = { open, close };
  AN.navigate = navigate;
}
