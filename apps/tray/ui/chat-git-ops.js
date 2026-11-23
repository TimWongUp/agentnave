"use strict";
// Confirmation dialogs for user-confirmed basic Git & PR operations (ADR-0115 §3, §5).
// Loaded before chat-git.js; never writes to user repositories until the user clicks confirm.
const ChatGitOps = (() => {
  const { h, L } = AN;
  let dialog = null;

  function ensureDialog() {
    if (dialog && dialog.isConnected) return dialog;
    if (typeof document === "undefined" || !document.body) return null;
    dialog = h("dialog.act.gp-dialog");
    document.body.append(dialog);
    return dialog;
  }

  function say(text, bad = false) {
    if (typeof Actions !== "undefined" && Actions.say) Actions.say(text, bad);
  }

  function fileBadge(file) {
    if (file.unmerged) return ["U", "bad"];
    if (file.untracked) return ["?", "muted"];
    const c = String(file.code || "").replace(/\./g, "")[0] || "M";
    if (c === "A") return ["A", "good"];
    if (c === "D") return ["D", "bad"];
    if (c === "R" || c === "C") return [c, "accent"];
    return ["M", "warn"];
  }

  function renderFileList(listEl, files, stageAll) {
    const shown = stageAll ? files : files.filter((f) => f.staged);
    if (!shown.length) {
      listEl.replaceChildren(h("li.gp-file-empty", {}, L("没有待提交的文件", "No files to commit")));
      return 0;
    }
    listEl.replaceChildren(
      ...shown.map((f) => {
        const [mark, tone] = fileBadge(f);
        return h(
          "li.gp-file-item",
          {},
          h(`span.gp-file-code.gp-tone-${tone}`, {}, mark),
          h("code.gp-file-path", {}, f.path),
        );
      }),
    );
    return shown.length;
  }

  function runModal({ title, body, primaryLabel, runningLabel, primaryDisabled = false, onConfirm, focusEl }) {
    const dlg = ensureDialog();
    if (!dlg || dlg.open) return;
    const errorBox = h("pre.gp-error", { role: "alert", hidden: true });
    let busy = false;
    const cancelBtn = h("button", { type: "button" }, L("取消", "Cancel"));
    const primaryBtn = h("button.primary", { type: "button", disabled: primaryDisabled }, primaryLabel);
    const close = () => {
      if (busy) return;
      dlg.close();
      dlg.replaceChildren();
    };
    cancelBtn.addEventListener("click", close);
    dlg.oncancel = (e) => {
      if (busy) {
        e.preventDefault();
        return;
      }
      e.preventDefault();
      close();
    };
    const submit = async () => {
      if (busy || primaryBtn.disabled) return;
      busy = true;
      errorBox.hidden = true;
      errorBox.textContent = "";
      cancelBtn.disabled = true;
      primaryBtn.disabled = true;
      primaryBtn.textContent = runningLabel;
      try {
        const err = await onConfirm();
        if (err) {
          errorBox.textContent = err;
          errorBox.hidden = false;
        } else {
          busy = false;
          dlg.close();
          dlg.replaceChildren();
          return;
        }
      } catch (error) {
        errorBox.textContent = error?.message || String(error);
        errorBox.hidden = false;
      } finally {
        if (busy) {
          busy = false;
          cancelBtn.disabled = false;
          primaryBtn.disabled = false;
          primaryBtn.textContent = primaryLabel;
        }
      }
    };
    primaryBtn.addEventListener("click", submit);
    const buttons = h("div.act-buttons", {}, cancelBtn, primaryBtn);
    dlg.replaceChildren(h("h2.act-title", {}, title), ...body.filter(Boolean), errorBox, buttons);
    dlg.showModal();
    (focusEl || (primaryBtn.disabled ? cancelBtn : primaryBtn))?.focus?.();
    return { primaryBtn, submit, errorBox };
  }

  async function prepare(op, target, extra = {}) {
    try {
      return await AN.post("/app/git-pr/prepare", { op, target, ...extra });
    } catch (error) {
      say(error?.message || L("无法准备 Git 操作", "Failed to prepare Git operation"), true);
      return null;
    }
  }

  function afterWrite() {
    if (typeof ChatGit !== "undefined") ChatGit.refresh?.("manual");
  }

  async function openCommit(target) {
    const info = await prepare("commit", target);
    if (!info) return;
    if (info.working) {
      say(info.busy_reason || L("CLI 正在工作，暂时无法提交", "CLI is working; commit unavailable"), true);
      return;
    }
    const files = Array.isArray(info.files) ? info.files : [];
    let stageAll = info.has_unstaged || !info.has_staged;
    const countEl = h("span.gp-dlg-count");
    const listEl = h("ul.gp-file-list", { "aria-label": L("将提交的文件", "Files to commit") });
    const updateList = () => {
      const n = renderFileList(listEl, files, stageAll);
      countEl.textContent = L(`${n} 个文件`, `${n} file(s)`);
      return n;
    };
    let count = updateList();

    let stageToggle = null;
    if (info.has_staged && info.has_unstaged) {
      const cb = h("input", { type: "checkbox", checked: stageAll });
      cb.addEventListener("change", () => {
        stageAll = Boolean(cb.checked);
        count = updateList();
        if (modal) modal.primaryBtn.disabled = info.has_unmerged || count === 0;
      });
      stageToggle = h("label.gp-check", {}, cb, L("暂存全部改动", "Stage all changes"));
    }

    const msgId = "gp-commit-msg";
    const msgInput = h("textarea.gp-textarea", {
      id: msgId,
      rows: "3",
      placeholder: L("输入提交说明", "Commit message"),
      "aria-label": L("提交说明", "Commit message"),
    });
    const metaRow = h(
      "div.gp-dlg-meta",
      {},
      h("code.gp-branch", {}, info.branch || "HEAD"),
      countEl,
    );
    const conflictTag = info.has_unmerged
      ? h("p.act-note.warn", {}, L("存在未解决的合并冲突", "Unresolved merge conflicts"))
      : null;
    const modal = runModal({
      title: L("提交改动", "Commit changes"),
      body: [
        metaRow,
        conflictTag,
        listEl,
        stageToggle,
        h("label.gp-field", { for: msgId }, h("span.gp-field-label", {}, L("提交说明", "Commit message")), msgInput),
      ],
      primaryLabel: L("提交", "Commit"),
      runningLabel: L("提交中…", "Committing…"),
      primaryDisabled: info.has_unmerged || count === 0,
      focusEl: msgInput,
      onConfirm: async () => {
        const message = msgInput.value.trim();
        if (!message) return L("请填写提交说明", "Enter a commit message");
        const res = await AN.post("/app/git-pr/commit", { target, message, stage_all: stageAll });
        say(res.sha ? L(`已提交 ${res.sha}`, `Committed ${res.sha}`) : L("已提交改动", "Committed changes"));
        afterWrite();
        return "";
      },
    });
    msgInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) modal?.submit?.();
    });
  }

  async function openPull(target) {
    const info = await prepare("pull", target);
    if (!info) return;
    if (info.working) {
      say(info.busy_reason || L("CLI 正在工作，暂时无法拉取", "CLI is working; pull unavailable"), true);
      return;
    }
    if (!info.upstream) {
      say(L("当前分支未配置上游分支，无法拉取", "Current branch has no upstream"), true);
      return;
    }
    const meta = h(
      "div.gp-dlg-meta",
      {},
      h("code.gp-branch", {}, info.branch || "HEAD"),
      h("span.gp-arrow", {}, "←"),
      h("code.gp-branch", {}, info.upstream),
      info.behind > 0 ? h("span.gp-tag", {}, `↓${info.behind}`) : null,
    );
    runModal({
      title: L("快进拉取", "Fast-forward pull"),
      body: [meta],
      primaryLabel: L("拉取", "Pull"),
      runningLabel: L("拉取中…", "Pulling…"),
      onConfirm: async () => {
        const res = await AN.post("/app/git-pr/pull", { target });
        say(
          res.up_to_date
            ? L("当前已是最新", "Already up to date")
            : L(`已快进拉取到 ${res.sha || info.upstream}`, `Fast-forwarded to ${res.sha || info.upstream}`),
        );
        afterWrite();
        return "";
      },
    });
  }

  async function openPush(target) {
    const info = await prepare("push", target);
    if (!info) return;
    if (info.working) {
      say(info.busy_reason || L("CLI 正在工作，暂时无法推送", "CLI is working; push unavailable"), true);
      return;
    }
    const meta = h(
      "div.gp-dlg-meta",
      {},
      h("code.gp-branch", {}, info.branch),
      h("span.gp-arrow", {}, "→"),
      h("code.gp-branch", {}, `${info.remote}/${info.remote_branch}`),
      info.set_upstream ? h("span.gp-tag", {}, L("设置上游", "Set upstream")) : null,
      info.ahead > 0 ? h("span.gp-tag", {}, `↑${info.ahead}`) : null,
    );
    runModal({
      title: L("推送分支", "Push branch"),
      body: [meta],
      primaryLabel: L("推送", "Push"),
      runningLabel: L("推送中…", "Pushing…"),
      onConfirm: async () => {
        const res = await AN.post("/app/git-pr/push", { target });
        say(L(`已推送到 ${res.upstream || `${info.remote}/${info.remote_branch}`}`, `Pushed to ${res.upstream || `${info.remote}/${info.remote_branch}`}`));
        afterWrite();
        return "";
      },
    });
  }

  async function openCreatePr(target) {
    const info = await prepare("create_pr", target);
    if (!info) return;
    if (info.working) {
      say(info.busy_reason || L("CLI 正在工作，暂时无法创建 PR", "CLI is working; create PR unavailable"), true);
      return;
    }
    const titleId = "gp-pr-title";
    const bodyId = "gp-pr-body";
    const titleInput = h("input.act-input", {
      id: titleId,
      type: "text",
      value: info.title || "",
      placeholder: L("输入 PR 标题", "PR title"),
      "aria-label": L("PR 标题", "PR title"),
    });
    const bodyInput = h("textarea.gp-textarea", {
      id: bodyId,
      rows: "4",
      placeholder: L("输入 PR 描述（可选）", "PR description (optional)"),
      "aria-label": L("PR 描述", "PR description"),
    });
    bodyInput.value = info.body || "";
    const draftCb = h("input", { type: "checkbox" });
    const meta = h(
      "div.gp-dlg-meta",
      {},
      h("code.gp-pr-ref", {}, info.repo),
      h("code.gp-branch", {}, `${info.head} → ${info.base}`),
      info.needs_push ? h("span.gp-tag", {}, L("先推送", "Pushes first")) : null,
    );
    const modal = runModal({
      title: L("创建 Pull Request", "Create Pull Request"),
      body: [
        meta,
        h("label.gp-field", { for: titleId }, h("span.gp-field-label", {}, L("标题", "Title")), titleInput),
        h("label.gp-field", { for: bodyId }, h("span.gp-field-label", {}, L("描述", "Description")), bodyInput),
        h("label.gp-check", {}, draftCb, L("草稿 PR", "Draft PR")),
      ],
      primaryLabel: L("创建 PR", "Create PR"),
      runningLabel: L("创建中…", "Creating…"),
      focusEl: titleInput,
      onConfirm: async () => {
        const title = titleInput.value.trim();
        if (!title) return L("请填写 PR 标题", "Enter a PR title");
        const res = await AN.post("/app/git-pr/pr/create", {
          target,
          title,
          body: bodyInput.value,
          base: info.base,
          draft: Boolean(draftCb.checked),
        });
        say(res.number ? L(`已创建 PR #${res.number}`, `Created PR #${res.number}`) : L("已创建 PR", "Created PR"));
        afterWrite();
        return "";
      },
    });
    titleInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.isComposing) modal?.submit?.();
    });
  }

  const METHOD_LABELS = () => ({
    merge: L("合并提交", "Merge commit"),
    squash: L("压缩合并", "Squash"),
    rebase: L("变基合并", "Rebase"),
  });

  async function openMergePr(target, pr) {
    if (!pr?.repo || !pr?.number) return;
    const info = await prepare("merge_pr", target, { repo: pr.repo, number: pr.number });
    if (!info) return;
    if (info.working) {
      say(info.busy_reason || L("CLI 正在工作，暂时无法合并 PR", "CLI is working; merge PR unavailable"), true);
      return;
    }
    const allowed = Array.isArray(info.allowed_methods) ? info.allowed_methods : [];
    if (!allowed.length) {
      say(L("该仓库未启用任何 PR 合并方式", "No merge methods allowed by repository"), true);
      return;
    }
    let chosen = info.default_method && allowed.includes(info.default_method) ? info.default_method : allowed[0];
    const labels = METHOD_LABELS();
    const choices = h(
      "div.act-days.gp-methods",
      { role: "radiogroup", "aria-label": L("合并方式", "Merge method") },
      allowed.map((method) => {
        const btn = h(
          "button",
          { type: "button", role: "radio", "aria-checked": String(method === chosen), "data-method": method },
          labels[method] || method,
        );
        btn.addEventListener("click", () => {
          chosen = method;
          for (const child of choices.children) {
            child.setAttribute("aria-checked", String(child.getAttribute("data-method") === chosen));
          }
        });
        return btn;
      }),
    );
    const meta = h(
      "div.gp-dlg-meta",
      {},
      h("code.gp-pr-ref", {}, `${pr.repo}#${pr.number}`),
      pr.head_ref_name && pr.base_ref_name
        ? h("code.gp-branch", {}, `${pr.head_ref_name} → ${pr.base_ref_name}`)
        : null,
      info.delete_branch_on_merge ? h("span.gp-tag", {}, L("合并后自动删远端分支", "Deletes remote branch")) : null,
    );
    runModal({
      title: L(`合并 PR #${pr.number}`, `Merge PR #${pr.number}`),
      body: [
        pr.title ? h("p.gp-dlg-pr-title", {}, pr.title) : null,
        meta,
        h("div.gp-field", {}, h("span.gp-field-label", {}, L("合并方式", "Merge method")), choices),
      ],
      primaryLabel: L("合并 PR", "Merge PR"),
      runningLabel: L("合并中…", "Merging…"),
      onConfirm: async () => {
        await AN.post("/app/git-pr/pr/merge", {
          target,
          repo: pr.repo,
          number: pr.number,
          method: chosen,
        });
        say(L(`已合并 PR #${pr.number}`, `Merged PR #${pr.number}`));
        afterWrite();
        return "";
      },
    });
  }

  return {
    openCommit,
    openPull,
    openPush,
    openCreatePr,
    openMergePr,
  };
})();
