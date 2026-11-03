"use strict";
// Static-only entry: serve ui/ and open preview.html?mock&page=chat&scene=live.
// Neither this loader nor mock.js is embedded in the App.
(async () => {
  const params = new URLSearchParams(location.search);
  if (!params.has("mock")) return;
  const page = ["chat", "panel", "settings"].includes(params.get("page")) ? params.get("page") : "chat";
  const response = await fetch(`${page}.html`);
  if (!response.ok) throw new Error(`Preview: ${response.status}`);
  const html = new DOMParser().parseFromString(await response.text(), "text/html");
  for (const node of html.querySelectorAll("script[src], link[rel=stylesheet]")) {
    const attr = node.tagName === "SCRIPT" ? "src" : "href";
    const path = node.getAttribute(attr);
    node.setAttribute(attr, path === "/avatars.js" ? "avatars/avatars.js" : path.replace(/^\/(chat|panel|settings|manage)\//, ""));
    if (path.endsWith("/common-status.js")) {
      const mock = html.createElement("script");
      mock.src = "mock.js";
      mock.defer = true;
      node.after(mock);
    }
  }
  if (params.get("language") === "en") html.documentElement.lang = "en";
  document.open();
  document.write("<!doctype html>" + html.documentElement.outerHTML);
  document.close();
})().catch(error => { document.body.textContent = error.message; });
