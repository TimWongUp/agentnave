"use strict";
// The office's terminal frame (ADR-0103): the CLI named by ?session, shown beside the floor in
// place of the messages. The office tells it when it is shown and when preferences change.
document.addEventListener("DOMContentLoaded", () => {
  const id = Number(new URLSearchParams(location.search).get("session"));
  const view = new TerminalView(id);
  AN.$("term").append(view.element);
  view.open();
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
