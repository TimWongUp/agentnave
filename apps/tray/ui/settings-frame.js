"use strict";
// Only the same-origin office parent can host this retained settings page.
if (window.parent !== window && new URLSearchParams(location.search).get("embedded") === "1") {
  document.documentElement.dataset.embedded = "true";
  document.documentElement.dataset.artPaused = "true";
  const origin = location.origin === "null" ? "*" : location.origin;
  const send = data => window.parent.postMessage({ type: "agentnave:settings", ...data }, origin);
  let receiving = false;
  function navigate(path) {
    const target = new URL(path, location.href);
    if (target.protocol !== location.protocol || target.host !== location.host) return false;
    send({ action: "navigate", path: target.href });
    return true;
  }
  function section(hash) {
    location.hash = hash === "#voice-settings" ? "#voice" : hash;
  }
  window.AgentNaveSettingsFrame = { navigate, changed: () => send({ action: "changed" }) };
  document.addEventListener("click", event => {
    const link = event.target.closest("a[href]");
    if (!link || event.button || link.getAttribute("href").startsWith("#")) return;
    const target = new URL(link.href, location.href);
    if (target.protocol === location.protocol && target.host === location.host) {
      event.preventDefault(); event.stopImmediatePropagation();
      navigate(link.href);
    }
  }, true);
  window.addEventListener("message", event => {
    if (event.source !== window.parent || event.data?.type !== "agentnave:settings") return;
    const data = event.data;
    if (data.action === "section") section(data.hash);
    else if (data.action === "visible") {
      document.documentElement.dataset.artPaused = String(!data.visible);
      document.documentElement.dataset.windowHidden = String(!data.visible);
      window.dispatchEvent(new Event("agentnave:visibility"));
      window.dispatchEvent(new Event(data.visible ? "focus" : "agentnave:settings-hidden"));
    }
    else if (data.action === "preferences") {
      receiving = true;
      window.dispatchEvent(new CustomEvent("agentnave:preferences", { detail: data.general }));
      receiving = false;
    }
    else if (data.action === "model-names") {
      receiving = true;
      AN.setFullModelNames(data.full === true);
      receiving = false;
    }
  });
  window.addEventListener("agentnave:preferences", event => {
    if (!receiving) send({ action: "preferences", general: event.detail });
  });
  // The choice also travels by message, so the office needs no working storage event to follow it.
  window.addEventListener("agentnave:model-names", event => {
    if (!receiving) send({ action: "model-names", full: event.detail.full });
  });
  window.addEventListener("pagehide", () => send({ action: "loading" }));
  window.addEventListener("DOMContentLoaded", () => {
    if (location.hash === "#voice-settings") section(location.hash);
    send({ action: "ready", language: document.documentElement.lang, choice: document.documentElement.dataset.lang });
  });
}
