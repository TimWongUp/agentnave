"use strict";
function isVersionOlder(a, b) {
  if (!a || !b) return false;
  const partsA = String(a).replace(/^v/, "").split(".").map(Number);
  const partsB = String(b).replace(/^v/, "").split(".").map(Number);
  for (let i = 0; i < Math.max(partsA.length, partsB.length); i++) {
    const numA = partsA[i] || 0;
    const numB = partsB[i] || 0;
    if (numA !== numB) return numA < numB;
  }
  return false;
}

function formatTime(iso) {
  if (!iso) return L("尚未检查", "Never");
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleString(L("zh-CN", "en-US"), {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

// Render Updates panel
function renderUpdates() {
  if (!appStatus) return;

  $("app-version").textContent = appStatus.version ? `v${appStatus.version}` : "—";
  const appBadge = $("app-badge");
  const updateInfo = appStatus.update;

  if (updateInfo?.state === "available") {
    appBadge.hidden = false;
    appBadge.className = "tag available";
    appBadge.textContent = L(`发现新版 v${updateInfo.version}`, `New version v${updateInfo.version}`);
  } else if (updateInfo?.state === "installing") {
    appBadge.hidden = false;
    appBadge.className = "tag installing";
    appBadge.textContent = L("正在下载更新…", "Downloading update…");
  } else if (updateInfo?.state === "failed") {
    appBadge.hidden = false;
    appBadge.className = "tag failed";
    appBadge.textContent = L("更新失败", "Update failed");
  } else if (updateInfo?.state === "none") {
    appBadge.hidden = false;
    appBadge.className = "tag done";
    appBadge.textContent = L("已是最新版本", "Up to date");
  } else {
    appBadge.hidden = true;
  }

  const serviceInfo = appStatus.service;
  renderServiceStatus(serviceInfo);

  $("last-checked").textContent = formatTime(appStatus.last_checked);

  // Update action buttons and progress
  const hasAppUpdate = updateInfo?.state === "available";
  const isServiceOutdated =
    serviceInfo &&
    (serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version)));
  const hasUpdate = hasAppUpdate || Boolean(isServiceOutdated);

  const isUpdating =
    updateInfo?.state === "checking" ||
    updateInfo?.state === "installing" ||
    ["upgrading", "installing"].includes(serviceInfo?.state);

  const btnCheck = $("btn-check-update");
  const btnApply = $("btn-apply-update");
  const updateHint = $("update-hint");
  const progressBox = $("update-progress-box");
  const progressText = $("update-progress-text");
  const errorBox = $("update-error-box");
  const errorText = $("update-error-text");

  // One action at a time: with an update found, "更新" leads and checking again stays secondary.
  $("update-badge").hidden = !hasUpdate && updateInfo?.state !== "installing";
  btnCheck.disabled = isUpdating;
  btnCheck.textContent = hasUpdate ? L("重新检查", "Check again") : L("立即检查", "Check now");
  // While anything runs the progress line says what; a disabled action would only contradict it.
  btnApply.hidden = !hasUpdate || isUpdating;
  btnApply.textContent = hasAppUpdate ? L(`更新到 v${updateInfo.version}…`, `Update to v${updateInfo.version}…`) : L("升级本机服务…", "Upgrade local service…");
  updateHint.hidden = !hasUpdate || isUpdating;

  if (hasUpdate) {
    if (hasAppUpdate) {
      updateHint.textContent = L("先看更新内容，确认后一次完成：升级本机服务，再安装 App 并自动重启。", "Review release notes first; once confirmed, it upgrades the local service, installs the App, and restarts automatically.");
    } else {
      updateHint.textContent = L("本机服务比 App 旧。升级约需一分钟，宿主重连后生效。", "Local service is older than the App. Upgrading takes ~1 min and takes effect after reconnecting the host.");
    }
  }

  if (isUpdating) {
    progressBox.hidden = false;
    if (updateInfo?.state === "checking") {
      progressText.textContent = L("正在检查更新…", "Checking for updates…");
    } else if (updateInfo?.state === "installing") {
      const percent = updateInfo.total > 0 ? ` ${Math.min(100, Math.floor(updateInfo.downloaded / updateInfo.total * 100))}%` : "";
      progressText.textContent = {
        service: L("正在更新本机服务，约需一分钟…", "Updating local service (~1 min)…"),
        app: L("正在安装 App…", "Installing App…"),
        restart: L("正在重启并更新 Skill…", "Restarting and updating Skill…"),
      }[updateInfo.step] || L(`正在下载 App 更新包${percent}…`, `Downloading App update${percent}…`);
    } else if (["upgrading", "installing"].includes(serviceInfo?.state)) {
      progressText.textContent = L("正在安装或升级本机服务，约需一分钟…", "Installing or upgrading local service (~1 min)…");
    }
  } else {
    progressBox.hidden = true;
  }

  const hasError = updateInfo?.state === "failed" || serviceInfo?.state === "failed" || Boolean(updateInfo?.error);
  $("btn-retry-update").hidden = isUpdating;
  if (hasError && !isUpdating) {
    errorBox.hidden = false;
    errorText.textContent = updateInfo?.error || serviceInfo?.error || L("操作失败，请重试。", "Operation failed. Please try again.");
  } else {
    errorBox.hidden = true;
  }

  updateStatusPolling(isUpdating);
}

window.addEventListener("agentnave:hosts", ({ detail }) => {
  installedService = detail.service;
  renderUpdates();
});
window.addEventListener("agentnave:service-installed", () => loadStatus());
// The update dialog reports each status it reads; skip the ones this page sent itself.
window.addEventListener("agentnave:status", ({ detail }) => {
  if (detail === appStatus) return;
  appStatus = detail;
  renderUpdates();
});
// A hidden window skips the update poll; catch up as soon as it is shown again.
document.addEventListener("visibilitychange", () => document.hidden || !statusPollTimer || loadStatus());

async function loadStatus() {
  try {
    appStatus = await apiGetStatus();
    renderUpdates();
    window.dispatchEvent(new CustomEvent("agentnave:status", { detail: appStatus }));
  } catch (err) {
    showNotice(L(`获取版本状态失败: ${err.message}`, `Failed to load version status: ${err.message}`), true);
  }
}

// Opening this tab checks again when the automatic daily check is over an hour old. A found
// update or a failure the user has not retried is left as it is.
const AUTO_CHECK_AFTER = 60 * 60 * 1000;
async function checkIfStale() {
  if ((await window.AgentNaveAboutSettings?.automatic()) === false) return;
  const update = appStatus?.update?.state;
  const service = appStatus?.service?.state;
  if (!appStatus || ["checking", "installing", "available", "failed"].includes(update) || ["upgrading", "installing"].includes(service)) return;
  const checked = Date.parse(appStatus.last_checked || "");
  if (Number.isNaN(checked) || Date.now() - checked > AUTO_CHECK_AFTER) triggerCheckUpdate(true);
}

async function triggerCheckUpdate(quiet = false) {
  const btnCheck = $("btn-check-update");
  btnCheck.disabled = true;
  const previous = appStatus;
  try {
    // The server refuses updates while it checks; show that instead of a live button.
    appStatus = { ...appStatus, update: { state: "checking", version: null, error: null } };
    renderUpdates();
    if (!quiet) showNotice(L("正在检查更新…", "Checking for updates…"));
    appStatus = await apiCheckUpdate(quiet);
    const state = appStatus?.update?.state;
    if (!quiet || state === "available") showNotice(
      state === "failed" ? L("检查更新失败", "Update check failed") : state === "available" ? L(`发现新版 v${appStatus.update.version}`, `New version v${appStatus.update.version}`) : state === "none" ? L("已是最新版本", "Up to date") : L("检查更新完成", "Update check complete"),
      state === "failed",
    );
  } catch (err) {
    appStatus = previous;
    if (quiet) await loadStatus();
    else showNotice(L(`检查更新失败: ${err.message}`, `Update check failed: ${err.message}`), true);
  } finally {
    renderUpdates();
  }
}

// Bind top-level event listeners
for (const event of ["click", "change"]) $("cli-cards").addEventListener(event, (e) => {
  operatedControl = e.target.closest("[data-focus-key]");
  queueMicrotask(() => { operatedControl = null; });
}, true);
bindAdvanced();

function renderServiceStatus(serviceInfo) {
  // Service status
  const serviceBadge = $("service-badge");
  const serviceMsg = $("service-msg");

  if (!serviceInfo) {
    $("service-version").textContent = installedService?.version ? `v${installedService.version}` : "—";
    serviceBadge.hidden = true;
    serviceMsg.hidden = Boolean(installedService?.installed);
    serviceMsg.textContent = L("请在 boss 分区安装本机服务。", "Install the local service in the Boss section.");
  } else {
    $("service-version").textContent = serviceInfo.version ? `v${serviceInfo.version}` : "—";
    const isServiceOutdated =
      serviceInfo.state === "outdated" ||
      (serviceInfo.version && isVersionOlder(serviceInfo.version, appStatus.version));

    if (["upgrading", "installing"].includes(serviceInfo.state)) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag upgrading";
      serviceBadge.textContent = serviceInfo.state === "installing" ? L("正在安装…", "Installing…") : L("正在升级…", "Upgrading…");
      serviceMsg.hidden = false;
      serviceMsg.textContent = L("正在从 GitHub 安装或升级本机服务，约需一分钟…", "Installing or upgrading local service from GitHub (~1 min)…");
    } else if (serviceInfo.state === "failed") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag failed";
      serviceBadge.textContent = L("升级失败", "Upgrade failed");
      serviceMsg.hidden = false;
      serviceMsg.textContent = serviceInfo.error || L("升级服务失败，请重试", "Failed to upgrade service; please try again");
    } else if (isServiceOutdated) {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag outdated";
      serviceBadge.textContent = L("比 App 旧，需要升级", "Older than App; upgrade needed");
      serviceMsg.hidden = true;
    } else if (serviceInfo.state === "done") {
      serviceBadge.hidden = false;
      serviceBadge.className = "tag done";
      serviceBadge.textContent = L("安装完成", "Installed");
      serviceMsg.hidden = false;
      serviceMsg.textContent = L("服务已安装，各 boss 重连后使用新版本。", "Service installed. Each boss will use the new version after reconnecting.");
    } else {
      serviceBadge.hidden = true;
      serviceMsg.hidden = true;
    }
  }

}

function updateStatusPolling(isUpdating) {
  // Manage polling
  if (isUpdating && !statusPollTimer) {
    statusPollTimer = setInterval(() => document.hidden || loadStatus(), 2000);
  } else if (!isUpdating && statusPollTimer) {
    clearInterval(statusPollTimer);
    statusPollTimer = null;
  }
}
