"use strict";

let cliUpdates = { clis: {} };
let cliInstallCommands = {};
let cliUpdating = null;
let cliBatch = null;
let cliSummary = "";
const cliUpdateErrors = {};
let cliQuery = 0;

async function loadCliInstallCommands() {
  try {
    const res = await fetch("/app/cli-install-commands", { cache: "no-store" });
    if (!res.ok) throw new Error(String(res.status));
    cliInstallCommands = await res.json();
  } catch (_) { /* Keep commands already loaded from the static route. */ }
}

// Called only after cards have painted; channel checks never hold up initial members data.
async function loadCliUpdates() {
  if (!cliUpdating && !cliBatch) { cliSummary = ""; renderCards(); }
  const query = ++cliQuery;
  try {
    const res = await fetch("/app/cli-updates", { cache: "no-store" });
    if (!res.ok) throw new Error(String(res.status));
    const data = await res.json();
    if (query !== cliQuery || cliUpdating || cliBatch) return;
    cliUpdates = data;
  } catch (_) {
    if (query !== cliQuery || cliUpdating || cliBatch) return;
    // A failed check must not leave an older 'update available' result actionable.
    cliUpdates = { clis: {} };
  }
  renderCards();
}

function renderCliUpdate(provider) {
  const status = cliUpdates.clis?.[provider];
  const running = cliUpdating === provider;
  return el("span", { className: "cli-update-slot" },
    status?.update || running ? el("button", {
      type: "button", className: "cli-update", disabled: !!cliUpdating || !!cliBatch,
      dataset: { focusKey: `${provider}/update` },
      title: L(`通过 ${status?.command} 更新`, `Update via ${status?.command}`),
      onClick: () => updateSingleCli(provider),
    }, running ? L("更新中…", "Updating…") : L(`更新到 ${status.latest}`, `Update to ${status.latest}`)) : null);
}

function renderCliUpdateError(provider) {
  const error = cliUpdateErrors[provider] || "";
  return el("p", { className: "member-error cli-update-error", role: "status", title: error }, error);
}

function updatableClis() {
  return PROVIDERS.filter((p) => detected[p]?.installed && cliUpdates.clis?.[p]?.update);
}

function renderCliUpdateSummary() {
  const providers = updatableClis();
  const visible = providers.length >= 2 || cliBatch || cliSummary;
  let text = cliSummary;
  if (cliBatch) {
    const name = PROVIDER_NAMES[cliBatch.provider] || cliBatch.provider;
    text = L(`正在更新 ${name}…（${cliBatch.index}/${cliBatch.total}）`, `Updating ${name}… (${cliBatch.index}/${cliBatch.total})`);
  } else if (!text && providers.length >= 2) {
    text = L(`${providers.length} 个 CLI 有新版本`, `${providers.length} CLIs have updates`);
  }
  return el("div", { className: `cli-update-summary${visible ? "" : " member-hidden"}`, role: "status" },
    el("span", {}, text),
    providers.length >= 2 ? el("button", { type: "button", disabled: !!cliUpdating || !!cliBatch,
      onClick: updateAllClis }, L("全部更新", "Update all")) : null);
}

async function performCliUpdate(provider) {
  cliUpdating = provider;
  delete cliUpdateErrors[provider];
  renderCards();
  try {
    // Invalidate in-flight GETs that could restore stale availability after the POST.
    cliQuery++;
    const res = await fetch("/app/cli-updates/update", {
      method: "POST", headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify({ provider }),
    });
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || L("更新失败", "Update failed"));
    cliUpdates = data;
    return true;
  } catch (err) {
    cliUpdateErrors[provider] = err.message;
    return false;
  } finally {
    // Read the backend's refreshed detection and members document without discarding edits.
    try {
      const data = await apiGetMembers(false);
      detected = data.detected || {};
      if (!dirty && !busy) {
        currentDoc = data.document || currentDoc;
        markSaved();
        documentError = data.error || null;
        takeReview(data);
        renderErrorBanner();
      }
    } catch (err) {
      cliUpdateErrors[provider] = [cliUpdateErrors[provider], err.message].filter(Boolean).join("\n");
    }
    cliQuery++;
    cliUpdating = null;
    renderCards();
  }
}

async function updateSingleCli(provider) {
  if (cliUpdating || cliBatch) return;
  cliSummary = "";
  await performCliUpdate(provider);
}

async function updateAllClis() {
  if (cliUpdating || cliBatch) return;
  const providers = updatableClis();
  let succeeded = 0;
  try {
    for (const [i, provider] of providers.entries()) {
      cliBatch = { provider, index: i + 1, total: providers.length };
      if (await performCliUpdate(provider)) succeeded++;
    }
    cliSummary = L(`更新完成：成功 ${succeeded}，失败 ${providers.length - succeeded}`, `Updates finished: ${succeeded} succeeded, ${providers.length - succeeded} failed`);
  } finally {
    cliBatch = null;
    renderCards();
  }
}

async function copyInstallCommand(command, button) {
  try {
    try { await navigator.clipboard.writeText(command); }
    catch (_) {
      const input = el("textarea", { className: "cli-copy-input", "aria-hidden": "true" });
      input.value = command;
      document.body.appendChild(input);
      try {
        input.select();
        if (!document.execCommand("copy")) throw new Error("copy");
      } finally { input.remove(); button.focus({ preventScroll: true }); }
    }
    button.textContent = L("已复制", "Copied");
  } catch (_) { button.textContent = L("复制失败", "Copy failed"); }
}

function renderMissingClis(missing) {
  const box = el("div", { className: "cli-missing" },
    el("button", { type: "button", className: "cli-missing-toggle", dataset: { focusKey: "missing/toggle" },
      "aria-expanded": String(missingOpen), onClick: () => { missingOpen = !missingOpen; renderCards(); },
    }, el("span", { className: "cli-chevron", "aria-hidden": "true" }),
    textNode(L(`未安装的 CLI（${missing.length}）`, `Uninstalled CLIs (${missing.length})`))));
  if (!missingOpen) return box;
  const list = el("div", { className: "cli-missing-list" });
  let hasNpm = false;
  for (const provider of missing) {
    const commands = cliInstallCommands[provider] || [];
    const row = el("div", { className: "cli-install-row" },
      el("span", { className: "cli-install-name" }, PROVIDER_NAMES[provider] || provider));
    const methods = el("div", { className: "cli-install-methods" });
    for (const entry of commands) {
      hasNpm ||= entry.label === "npm";
      const button = el("button", { type: "button", className: "cli-install-copy",
        onClick: () => copyInstallCommand(entry.command, button) }, L("复制", "Copy"));
      methods.appendChild(el("div", { className: "cli-install-command" },
        el("span", { className: "badge badge-muted" }, entry.label === "安装脚本" ? L("安装脚本", "Install script") : entry.label),
        el("code", {}, entry.command), button));
    }
    row.appendChild(methods);
    list.appendChild(row);
  }
  if (hasNpm) list.appendChild(el("p", {}, L("npm 命令需要先装 Node.js。", "Install Node.js before using npm commands.")));
  list.appendChild(el("p", {}, L("装好后点「刷新模型列表」。", "After installing, click “Refresh models”.")));
  box.appendChild(list);
  return box;
}
