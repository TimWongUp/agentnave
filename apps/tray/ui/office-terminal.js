"use strict";
// The terminal borrows the office's content area like settings does; the office, its player and
// the terminal page all stay loaded, so switching back and forth keeps the CLI's screen.
{
  let frame = null;
  const header = document.querySelector("header.bar");
  const origin = location.origin === "null" ? "*" : location.origin;
  const send = data => frame?.contentWindow?.postMessage({ type: "agentnave:terminal", ...data }, origin);
  const pathOf = href => {
    const url = new URL(href, location.href);
    return url.protocol === location.protocol && url.host === location.host ? url.pathname : null;
  };

  function highlight(prefix) {
    for (const link of header.querySelectorAll("nav a")) {
      if (pathOf(link.href)?.startsWith(prefix)) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  }

  function open() {
    AN.officeSettings?.close();
    if (!frame) {
      frame = AN.h("iframe.office-terminal", { title: AN.L("终端", "Terminal"), src: "/terminal/?embedded=1" });
      document.body.append(frame);
    }
    frame.hidden = false;
    document.getElementById("app").inert = true;
    highlight("/terminal/");
    send({ action: "shown" });
    frame.focus();
  }

  function close(path) {
    if (frame) frame.hidden = true;
    // Settings marks its own link; going back to the office marks the office alone, since
    // closing settings marks every link that is not settings.
    if (!path.startsWith("/settings/")) {
      document.getElementById("app").inert = false;
      highlight("/chat/");
    }
  }

  // Runs beside the settings handler, which takes office and settings links.
  document.addEventListener("click", event => {
    const link = event.target.closest("a[href]");
    if (!link || event.button) return;
    const path = pathOf(link.href);
    if (!path) return;
    if (path.startsWith("/terminal/")) {
      event.preventDefault();
      open();
    } else if (path.startsWith("/chat/") || path.startsWith("/settings/")) close(path);
  }, true);
  window.addEventListener("message", event => {
    if (event.source !== frame?.contentWindow || event.data?.type !== "agentnave:terminal") return;
    if (event.data.action === "ready") send({ action: "preferences", general: window.ANAppearance?.preferences() });
  });
  window.addEventListener("agentnave:preferences", event => send({ action: "preferences", general: event.detail }));
}
