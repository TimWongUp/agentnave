"use strict";

// Updating, as one flow everywhere (header button, settings, the menu's "检查更新"): use what the
// last check found (check again only when it found nothing to do or is too old), show what changed
// since this version, and only on "立即更新" upgrade the service, install the App and restart. An
// update already running shows its progress. The release notes are plain text set with
// textContent, never as HTML.
{
  const { h, L, load, post } = AN;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  let dialog = null;
  let running = false;

  // A small subset of the release notes' Markdown: "## " headings, "- " bullets, paragraphs and
  // `code`.
  function inline(text) {
    return text.split(/(`[^`]+`)/).filter(Boolean).map((part) =>
      part.startsWith("`") && part.endsWith("`") ? h("code", {}, part.slice(1, -1)) : part.replace(/\*\*/g, ""));
  }
  function notesNode(text) {
    const out = [];
    let list = null;
    for (const raw of text.split("\n")) {
      const line = raw.trim();
      if (!line) { list = null; continue; }
      if (line.startsWith("#")) {
        list = null;
        out.push(h("h4.upd-h", {}, ...inline(line.replace(/^#+\s*/, ""))));
      } else if (/^[-*] /.test(line)) {
        if (!list) out.push((list = h("ul.upd-list")));
        list.append(h("li", {}, ...inline(line.slice(2))));
      } else {
        list = null;
        out.push(h("p.upd-p", {}, ...inline(line)));
      }
    }
    return out;
  }

  function show(title, body, buttons) {
    if (!dialog) {
      dialog = h("dialog.upd");
      dialog.addEventListener("cancel", (e) => { if (running) e.preventDefault(); });
      document.body.append(dialog);
    }
    const row = h("div.upd-row", {}, ...buttons.map(([label, run, primary]) => {
      const b = h(`button${primary ? ".primary" : ""}`, { type: "button" }, label);
      b.addEventListener("click", run);
      return b;
    }));
    dialog.replaceChildren(h("h2.upd-title", {}, title), h("div.upd-body", {}, ...body), row);
    if (!dialog.open) dialog.showModal();
    dialog.querySelector(".primary")?.focus();
  }
  const close = () => dialog?.close();
  const done = (status) => window.dispatchEvent(new CustomEvent("agentnave:status", { detail: status }));

  const STEPS = [
    ["download", L("下载 App 更新包", "Download app update")],
    ["service", L("更新本机服务", "Update local service")],
    ["app", L("安装 App", "Install app")],
    ["restart", L("重启并更新 Skill", "Restart and update skills")],
  ];
  const mb = (bytes) => (bytes / 1048576).toFixed(1);
  const took = (seconds) => seconds >= 60
    ? L(`已用 ${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`, `${Math.floor(seconds / 60)}m ${seconds % 60}s elapsed`)
    : L(`已用 ${seconds} 秒`, `${seconds}s elapsed`);
  function stepList(update, elapsed, failed) {
    const at = STEPS.findIndex(([key]) => key === update.step);
    return h("ol.upd-steps", {}, ...STEPS.map(([key, label], i) => {
      if (i !== at) return h(`li.${i < at ? "done" : "todo"}`, {}, label);
      if (failed) return h("li.bad", {}, label);
      if (key !== "download") return h(`li.now${elapsed >= 3 ? ".told" : ""}`, {}, label, elapsed >= 3 && h("span.upd-detail", {}, took(elapsed)));
      const known = update.total > 0;
      const percent = known ? Math.min(100, Math.floor(update.downloaded / update.total * 100)) : 0;
      return h(`li.now${update.downloaded > 0 ? ".told" : ""}`, {}, label, update.downloaded > 0 && h("span.upd-detail", {}, known
        ? L(`${percent}%（${mb(update.downloaded)} / ${mb(update.total)} MB）`, `${percent}% (${mb(update.downloaded)} / ${mb(update.total)} MB)`)
        : L(`已下载 ${mb(update.downloaded)} MB`, `${mb(update.downloaded)} MB downloaded`)),
        known && h("div.upd-bar", {}, h("i", { style: `width:${percent}%` })));
    }));
  }
  function progress(status) {
    const update = status.update;
    show(L(`正在更新到 v${update.version}`, `Updating to v${update.version}`), [
      stepList(update, status.elapsed),
      h("p.upd-note", {}, L("正在运行的任务不受影响。", "Running tasks are not affected.")),
    ], []);
  }
  function serviceProgress(status) {
    show(L("正在升级本机服务", "Upgrading local service"), [
      h("p.upd-p", {}, status.elapsed >= 3
        ? L(`约需一分钟，${took(status.elapsed)}。`, `About a minute, ${took(status.elapsed)}.`)
        : L("约需一分钟。", "About a minute.")),
      h("p.upd-note", {}, L("正在运行的任务不受影响。", "Running tasks are not affected.")),
    ], []);
  }

  function failure(update) {
    show(L("更新没有完成", "Update incomplete"), [
      stepList(update, 0, true),
      h("p.upd-p.upd-error", {}, update.error || L("更新失败", "Update failed")),
    ], [[L("关闭", "Close"), close], [L("重试", "Retry"), () => apply(update.version, false), true]]);
  }

  // Report what a started update does until it ends; on a real App update the restart ends it.
  async function follow(status, serviceOnly) {
    running = true;
    const version = status.update?.version;
    try {
      for (;;) {
        done(status);
        const update = status.update;
        const service = status.service;
        if (!serviceOnly && update?.state === "failed") {
          running = false;
          if (update.step) return failure(update);
          throw new Error(update.error || L("更新失败", "Update failed"));
        }
        if (serviceOnly) {
          if (service?.state === "failed") throw new Error(service.error || L("升级本机服务失败", "Failed to upgrade local service"));
          if (!["upgrading", "installing"].includes(service?.state)) break;
          serviceProgress(status);
        } else if (update?.state === "installing") {
          progress(status);
        } else if (status.version === version) {
          break;
        }
        await sleep(1000);
        status = await load("/app/status");
      }
      running = false;
      // The App normally restarts before this point.
      show(serviceOnly ? L("本机服务已升级", "Local service upgraded") : L(`已更新到 v${version}`, `Updated to v${version}`), [h("p.upd-p", {}, L("服务已安装，各 boss 重连后使用新版本。", "Service installed. Each boss will use the new version after reconnecting."))], [[L("好", "OK"), close, true]]);
    } catch (error) {
      running = false;
      show(L("更新没有完成", "Update incomplete"), [h("p.upd-p.upd-error", {}, error.message)], [[L("关闭", "Close"), close], [L("重新检查", "Check again"), () => run(true), true]]);
    }
  }

  async function apply(version, serviceOnly) {
    running = true;
    try {
      if (serviceOnly) serviceProgress({});
      else progress({ update: { version, step: "download" } });
      const status = await post(serviceOnly ? "/app/update/apply?service=1" : "/app/update/apply");
      if (!status.started) throw new Error(L("另一项检查或更新正在进行，请稍后重试。", "Another check or update is in progress. Please try again later."));
      return follow(status, serviceOnly);
    } catch (error) {
      running = false;
      show(L("更新没有完成", "Update incomplete"), [h("p.upd-p.upd-error", {}, error.message)], [[L("关闭", "Close"), close], [L("重新检查", "Check again"), () => run(true), true]]);
    }
  }

  // Something to do, from a check recent enough to use as it is.
  const usable = (status) => status.check_fresh && (status.update?.state === "available"
    || (status.update?.state === "none" && ["outdated", "failed"].includes(status.service?.state)));

  async function run(recheck = false) {
    if (running) return;
    let status = null;
    if (!recheck) {
      try { status = await load("/app/status"); } catch { /* A check below reports the trouble. */ }
      const update = status?.update;
      if (update?.state === "installing") return follow(status, false);
      if (["upgrading", "installing"].includes(status?.service?.state)) return follow(status, true);
      if (update?.state === "failed" && update.step) return failure(update);
    }
    // Only a found update can stay if this check fails, so only that one may fail silently.
    const found = status?.update?.state === "available";
    if (recheck || !status || !usable(status)) {
      const reason = found
        ? L("上次检查已超过一天，先确认有没有更新的版本…", "Last checked over a day ago; checking for a newer version first…")
        : L("正在检查最新版本…", "Checking for the latest version…");
      show(L("检查更新", "Check for updates"), [h("p.upd-p", {}, reason)], [[L("取消", "Cancel"), close]]);
      try {
        status = await post(found ? "/app/update/check?quiet=1" : "/app/update/check");
      } catch (error) {
        return show(L("检查更新失败", "Update check failed"), [h("p.upd-p.upd-error", {}, error.message)], [[L("关闭", "Close"), close], [L("重试", "Retry"), () => run(true), true]]);
      }
    }
    done(status);
    const update = status.update;
    const service = status.service;
    if (update?.state === "installing") return follow(status, false);
    if (update?.state === "failed")
      return show(L("检查更新失败", "Update check failed"), [h("p.upd-p.upd-error", {}, update.error || L("请检查网络后重试", "Please check your network and try again"))], [[L("关闭", "Close"), close], [L("重试", "Retry"), () => run(true), true]]);
    if (update?.state === "available") {
      const items = update.notes?.items || [];
      const link = update.notes?.link;
      const notes = [
        ...(items.length
          ? items.map((item) => h("section.upd-ver", {}, h("h3.upd-v", {}, `v${item.version}`), ...notesNode(item.notes)))
          : [h("p.upd-p", {}, L("这个版本没有附带更新说明。", "No release notes included for this version."))]),
        // With items, the link covers the releases older than those the manifest carries.
        ...(link ? [h("a.upd-link", { href: link, target: "_blank", rel: "noopener" }, items.length ? L("更早版本的更新说明见 GitHub", "Older release notes on GitHub") : L("在 GitHub 查看更新说明", "View release notes on GitHub"))] : []),
      ];
      return show(L(`有新版本 v${update.version}`, `New version v${update.version}`), [
        h("p.upd-sub", {}, L(`当前 v${status.version}`, `Current v${status.version}`)),
        h("div.upd-notes", {}, ...notes),
        h("p.upd-note", {}, L("更新时先升级本机服务，再安装 App 并自动重启；服务升级失败就不装 App。", "Upgrades the local service first, then installs the app and restarts. If the service upgrade fails, the app will not be installed.")),
      ], [[L("稍后", "Later"), close], [L("立即更新", "Update now"), () => apply(update.version, false), true]]);
    }
    if (service?.state === "failed")
      return show(L("升级本机服务失败", "Service upgrade failed"), [h("p.upd-p.upd-error", {}, service.error || L("升级本机服务失败", "Failed to upgrade local service"))],
        [[L("关闭", "Close"), close], [L("重试升级", "Retry upgrade"), () => apply(status.version, true), true]]);
    if (service?.state === "outdated")
      return show(L("升级本机服务", "Upgrade local service"), [h("p.upd-p", {}, L(`本机服务 v${service.version} 比 App v${status.version} 旧。升级约需一分钟，正在运行的任务不受影响。`, `Local service v${service.version} is older than app v${status.version}. Upgrading takes about a minute; running tasks are not affected.`))],
        [[L("稍后", "Later"), close], [L("升级", "Upgrade"), () => apply(status.version, true), true]]);
    show(L("已是最新版本", "Up to date"), [h("p.upd-p", {}, L(`App 与本机服务都是 v${status.version}。`, `Both app and local service are on v${status.version}.`))], [[L("好", "OK"), close, true]]);
  }
  AN.updateFlow = run;
  // The menu's "检查更新" and the panel open the main window with ?check-update.
  addEventListener("DOMContentLoaded", () => {
    const url = new URL(location.href);
    if (!url.searchParams.has("check-update")) return;
    url.searchParams.delete("check-update");
    history.replaceState(null, "", url);
    run();
  });
}
