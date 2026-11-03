"use strict";
{
  const { h, $, L, load, post } = AN;
  const root = $("storage-settings");
  let data = null;
  let busy = false;
  let pendingConfirm = null; // { days, workspaces }

  const CHOICES = [
    [null, () => L("永不", "Never")],
    [30, () => L("30 天", "30 days")],
    [90, () => L("90 天", "90 days")],
    [180, () => L("180 天", "180 days")],
  ];

  function say(text, bad = false) {
    const notice = $("notice");
    if (!notice) return;
    notice.textContent = text;
    notice.classList.toggle("error", bad);
  }

  function formatBytes(bytes) {
    const n = Number(bytes) || 0;
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function isShorter(current, next) {
    if (next === null) return false;
    if (current === null || current === undefined) return true;
    return next < current;
  }

  async function selectRetention(days) {
    if (busy || !data) return;
    const current = data.retention_days ?? null;
    if (days === current && !pendingConfirm) return;
    if (days === current && pendingConfirm) {
      pendingConfirm = null;
      render();
      return;
    }
    if (isShorter(current, days)) {
      busy = true;
      render();
      try {
        const preview = await load(`/app/storage/preview?days=${days}`);
        pendingConfirm = { days, workspaces: Number(preview?.workspaces) || 0 };
        say("");
      } catch (err) {
        pendingConfirm = null;
        say(err.message || L("无法预览清理结果", "Failed to preview cleanup"), true);
      } finally {
        busy = false;
        render();
      }
      return;
    }
    pendingConfirm = null;
    await commitRetention(days);
  }

  async function commitRetention(days) {
    if (busy || !data) return;
    const previous = data.retention_days ?? null;
    busy = true;
    render();
    try {
      await post("/app/settings", { version: 1, history_retention_days: days });
      data.retention_days = days;
      pendingConfirm = null;
      say(L("已保存", "Saved"));
      const latest = await load("/app/storage").catch(() => null);
      if (latest) data = latest;
    } catch (err) {
      data.retention_days = previous;
      pendingConfirm = null;
      say(err.message || L("保存失败", "Failed to save"), true);
    } finally {
      busy = false;
      render();
    }
  }

  async function openFolder() {
    if (busy) return;
    busy = true;
    render();
    try {
      await post("/app/storage/open", {});
    } catch (err) {
      say(err.message || L("无法打开文件夹", "Failed to open folder"), true);
    } finally {
      busy = false;
      render();
    }
  }

  function renderRetentionCard() {
    const title = L("消息记录自动清理", "Auto-clean message history");
    const saved = data.retention_days ?? null;
    const activeChoice = pendingConfirm ? pendingConfirm.days : saved;
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": title });

    for (const [val, labelFn] of CHOICES) {
      const active = activeChoice === val;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled: busy,
      }, labelFn());
      btn.addEventListener("click", () => void selectRetention(val));
      control.append(btn);
    }

    const card = h("div.updates-card.storage-card", {},
      h("div.storage-row", {},
        h("h2", {}, title),
        control,
      ),
    );

    if (pendingConfirm) {
      const n = pendingConfirm.workspaces;
      const cancelBtn = h("button", {
        type: "button",
        disabled: busy,
      }, L("取消", "Cancel"));
      cancelBtn.addEventListener("click", () => {
        pendingConfirm = null;
        render();
      });
      const confirmBtn = h("button.danger", {
        type: "button",
        disabled: busy,
      }, L("确认", "Confirm"));
      confirmBtn.addEventListener("click", () => void commitRetention(pendingConfirm.days));

      card.append(
        h("div.disable-confirm.storage-confirm", {
          role: "group",
          "aria-label": title,
        },
          h("p", {}, L(`将删除 ${n} 个工作区`, `Will delete ${n} workspace(s)`)),
          h("div.btn-group", {}, cancelBtn, confirmBtn),
        ),
      );
    }

    return h("div.updates-panel", {}, card);
  }

  function renderDirectoryCard() {
    const title = L("数据目录", "Data directory");
    const openBtn = h("button", {
      type: "button",
      disabled: busy,
    }, L("打开文件夹", "Open folder"));
    openBtn.addEventListener("click", () => void openFolder());

    return h("div.updates-panel", {},
      h("div.updates-card.storage-card", {},
        h("h2", {}, title),
        h("div.storage-dir-row", {},
          h("div.storage-dir-info", {},
            h("code.storage-path", {}, data.path || "—"),
            h("span.storage-size", {}, formatBytes(data.bytes)),
          ),
          openBtn,
        ),
      ),
    );
  }

  function render() {
    if (!root || !data) return;
    root.replaceChildren(renderRetentionCard(), renderDirectoryCard());
  }

  async function refresh() {
    if (!root || busy) return;
    try {
      data = await load("/app/storage");
      pendingConfirm = null;
      render();
    } catch (err) {
      say(err.message || L("无法读取数据与存储设置", "Failed to load storage settings"), true);
    }
  }

  const visible = () => !$("pane-storage")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveStorageSettings = { load: refresh };
  if (!$("pane-storage")?.hidden) void refresh();
}
