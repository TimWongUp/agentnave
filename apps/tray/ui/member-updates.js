"use strict";

let cliUpdates = { clis: {} };
let cliInstallCommands = {};
const cliUpdating = new Set();
let cliBatch = null;
let cliSummary = "";
const cliUpdateErrors = {};
let cliQuery = 0;
let cliRead = 0;
let cliReading = 0;

function cliBusy() {
  return cliUpdating.size > 0 || cliReading > 0;
}

async function loadCliInstallCommands() {
  try {
    const res = await fetch("/app/cli-install-commands", { cache: "no-store" });
    if (!res.ok) throw new Error(String(res.status));
    cliInstallCommands = await res.json();
  } catch (_) { /* Keep commands already loaded from the static route. */ }
}

// Called only after cards have painted; channel checks never hold up initial members data.
async function loadCliUpdates() {
  if (cliSummary && !cliBusy() && !cliBatch) { cliSummary = ""; renderCards(); }
  const query = ++cliQuery;
  // A failed check must not leave an older 'update available' result actionable.
  let latest = { clis: {} };
  try {
    const res = await fetch("/app/cli-updates", { cache: "no-store" });
    if (!res.ok) throw new Error(String(res.status));
    latest = await res.json();
  } catch (_) { /* Nothing to offer until a check succeeds. */ }
  if (query !== cliQuery || cliBusy()) return;
  // A check that found nothing new leaves the cards, and any open control in them, alone.
  if (JSON.stringify(latest) === JSON.stringify(cliUpdates)) return;
  cliUpdates = latest;
  renderCards();
}

function cliKey(provider, target) {
  return target ? `${provider}/${target}` : provider;
}

// A lone standalone installation is an ordinary CLI: its card needs no source rows.
function cliInstallations(provider) {
  const installations = cliUpdates.clis?.[provider]?.installations || [];
  return installations.length === 1 && installations[0].id === "standalone" ? [] : installations;
}

function cliInstallationName({ id, label }) {
  if (id === "standalone") return L("独立 CLI", "Standalone CLI");
  const app = ["ChatGPT", "Codex"].find((name) => label.includes(name));
  return app ? L(label, `${app} built-in CLI`) : label;
}

function renderCliUpdateButton(provider, target, status) {
  const key = cliKey(provider, target);
  const running = cliUpdating.has(key);
  return status?.update || running ? el("button", {
    type: "button", className: "cli-update", disabled: running,
    dataset: { focusKey: `${key}/update` },
    title: L(`通过 ${status?.command} 更新到 ${status?.latest}`, `Update to ${status?.latest} via ${status?.command}`),
    onClick: () => updateSingleCli(provider, target),
  }, running ? L("更新中…", "Updating…") : L(`更新到 ${status.latest}`, `Update to ${status.latest}`)) : null;
}

function renderCliUpdate(provider) {
  if (cliInstallations(provider).length) return null;
  return el("span", { className: "cli-update-slot" }, renderCliUpdateButton(provider, undefined, cliUpdates.clis?.[provider]));
}

// The check's version moves as soon as an update lands; detection catches up when all have finished.
function renderCliVersion(provider, info) {
  const version = cliUpdates.clis?.[provider]?.version || info.version;
  if (!info.installed || !version || cliInstallations(provider).length) return null;
  return el("span", { className: "cli-version" }, version);
}

function renderCliInstallation(provider, install) {
  const standalone = install.id === "standalone";
  const note = standalone ? "" : L("随桌面应用更新", "Updates with the desktop app");
  const source = provider === "codex" ? currentDoc.clis?.codex?.executable_source : null;
  return el("div", { className: "cli-installation", title: [note, install.path].filter(Boolean).join("\n") },
    el("span", { className: "cli-installation-name" }, cliInstallationName(install)),
    el("span", { className: "cli-version" }, install.version || ""),
    el("span", { className: "cli-installation-active" },
      (source ? install.id === source : install.active) ? el("span", { className: "badge badge-accent" }, L("当前使用", "In use")) : null),
    el("span", { className: "cli-update-slot" }, standalone ? renderCliUpdateButton(provider, install.id, install) : null));
}

function renderCodexSource() {
  const selected = currentDoc.clis?.codex?.executable_source || "auto";
  const installs = cliUpdates.clis?.codex?.installations || [];
  const available = selected === "auto" || installs.some((install) => install.id === selected);
  return el("div", { className: "member-row" },
    el("label", { className: "member-scene", htmlFor: "codex-executable-source" }, L("运行来源", "CLI source")),
    el("div", { className: "member-line" },
      el("select", { id: "codex-executable-source", className: "member-model", dataset: { focusKey: "codex/source" },
        onChange: (event) => {
          currentDoc.clis ||= {};
          currentDoc.clis.codex ||= { models: {} };
          if (event.target.value === "auto") delete currentDoc.clis.codex.executable_source;
          else currentDoc.clis.codex.executable_source = event.target.value;
          edited();
        },
      }, ...[["auto", L("自动", "Automatic")], ["desktop", L("桌面端内置", "Desktop built-in")], ["standalone", L("独立 CLI", "Standalone CLI")]]
        .map(([value, label]) => el("option", { value, selected: value === selected }, label))),
      available ? null : el("span", { className: "member-error", role: "status" }, L("所选安装不可用", "Selected installation unavailable"))));
}

function renderCliInstallations(provider) {
  const installations = cliInstallations(provider);
  if (!installations.length) return null;
  return el("div", { className: "cli-installations" }, installations.map((install) => renderCliInstallation(provider, install)));
}

function renderCliUpdateError(provider) {
  const error = cliUpdateErrors[provider] || "";
  return el("p", { className: "member-error cli-update-error", role: "status", title: error }, error);
}

function updatableClis() {
  return PROVIDERS.filter((p) => detected[p]?.installed).flatMap((provider) => {
    const installations = cliInstallations(provider);
    if (installations.length) {
      return installations.some((i) => i.id === "standalone" && i.update) ? [{ provider, target: "standalone" }] : [];
    }
    return cliUpdates.clis?.[provider]?.update ? [{ provider }] : [];
  });
}

function waitingClis() {
  return updatableClis().filter(({ provider, target }) => !cliUpdating.has(cliKey(provider, target)));
}

function renderCliUpdateSummary() {
  const targets = updatableClis();
  const visible = targets.length >= 2 || cliBatch || cliSummary;
  let text = cliSummary;
  if (cliBatch) {
    const total = cliBatch.keys.length;
    const done = cliBatch.keys.filter((key) => !cliUpdating.has(key)).length;
    text = L(`正在更新…（${done}/${total}）`, `Updating… (${done}/${total})`);
  } else if (!text && targets.length >= 2) {
    text = L(`${targets.length} 个 CLI 有新版本`, `${targets.length} CLIs have updates`);
  }
  return el("div", { className: `cli-update-summary${visible ? "" : " member-hidden"}`, role: "status" },
    el("span", {}, text),
    targets.length >= 2 ? el("button", { type: "button", disabled: !!cliBatch || !waitingClis().length,
      onClick: updateAllClis }, L("全部更新", "Update all")) : null);
}

// Read the backend's refreshed detection and members document without discarding edits.
async function settleCliUpdates(provider) {
  const read = ++cliRead;
  cliReading++;
  renderCards();
  try {
    const before = savedSnapshot;
    const seatsBefore = reviewSaves;
    const data = await apiGetMembers(false);
    // Only the newest read made after the last update has finished describes every CLI.
    if (read === cliRead && !cliUpdating.size) {
      detected = data.detected || {};
      // A save made during this read is newer than what it returned.
      if (!dirty && !busy && savedSnapshot === before && reviewSaves === seatsBefore) {
        currentDoc = data.document || currentDoc;
        markSaved();
        documentError = data.error || null;
        takeReview(data);
        renderErrorBanner();
      }
    }
  } catch (err) {
    cliUpdateErrors[provider] = [cliUpdateErrors[provider], err.message].filter(Boolean).join("\n");
  }
  cliReading--;
  cliQuery++;
  renderCards();
}

async function performCliUpdate(provider, target) {
  const key = cliKey(provider, target);
  cliUpdating.add(key);
  delete cliUpdateErrors[provider];
  renderCards();
  try {
    // Invalidate in-flight GETs that could restore stale availability after the POST.
    cliQuery++;
    const res = await fetch("/app/cli-updates/update", {
      method: "POST", headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(target ? { provider, target } : { provider }),
    });
    const data = await res.json();
    if (!res.ok || data.error) throw new Error(data.error || L("更新失败", "Update failed"));
    // Each response is only current for its own CLI while others are still updating.
    cliUpdates = { ...cliUpdates, clis: { ...cliUpdates.clis, [provider]: data.clis?.[provider] } };
    return true;
  } catch (err) {
    cliUpdateErrors[provider] = err.message;
    return false;
  } finally {
    cliUpdating.delete(key);
    cliQuery++;
    if (cliUpdating.size) renderCards();
    else await settleCliUpdates(provider);
  }
}

async function updateSingleCli(provider, target) {
  if (cliUpdating.has(cliKey(provider, target))) return;
  cliSummary = "";
  await performCliUpdate(provider, target);
}

async function updateAllClis() {
  const targets = waitingClis();
  if (cliBatch || !targets.length) return;
  cliBatch = { keys: targets.map(({ provider, target }) => cliKey(provider, target)) };
  try {
    const results = await Promise.all(targets.map(({ provider, target }) => performCliUpdate(provider, target)));
    const succeeded = results.filter(Boolean).length;
    cliSummary = L(`更新完成：成功 ${succeeded}，失败 ${targets.length - succeeded}`, `Updates finished: ${succeeded} succeeded, ${targets.length - succeeded} failed`);
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
