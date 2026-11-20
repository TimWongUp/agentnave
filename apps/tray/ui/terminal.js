"use strict";
// The office's terminal frame (ADR-0103): the CLI named by ?session, shown beside the floor in
// place of the messages. The office tells it when it is shown and when preferences change.
document.addEventListener("DOMContentLoaded", () => {
  const id = Number(new URLSearchParams(location.search).get("session"));
  window.addEventListener("focus", () => {
    if (window.parent && window.parent !== window) {
      const origin = !location.origin || location.origin === "null" ? "*" : location.origin;
      window.parent.postMessage({ type: "agentnave:terminal", action: "focus" }, origin);
    }
  });
  const view = new TerminalView(id, (session, code) => {
    if (window.parent && window.parent !== window) {
      const origin = !location.origin || location.origin === "null" ? "*" : location.origin;
      window.parent.postMessage({ type: "agentnave:terminal", action: "exited", session, code }, origin);
    }
  });
  AN.$("term").append(view.element);
  view.open();
  new MutationObserver(() => view.restyle()).observe(document.documentElement, {
    attributes: true, attributeFilter: ["data-theme"],
  });
  window.addEventListener("message", (event) => {
    if (event.source !== window.parent || event.data?.type !== "agentnave:terminal") return;
    if (event.data.action === "shown") {
      view.layout();
      view.focus();
    } else if (event.data.action === "preferences") {
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: event.data.general }));
    }
  });
});
