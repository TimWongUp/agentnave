"use strict";
{
  // Settings -> 员工 -> Claude Code named accounts (ADR-0107): managed inside the Claude Code
  // detail view as an expandable compact list. Sign-in state is read on demand and never written to disk.

  const S = AN.settings ||= {};
  const { $, L, el, textNode } = S;
  const PROVIDER = "claude";

  // Where the sign-in states stand: id -> { loading } | { state } | { error }.
  const states = new Map();
  let folders = null; // Scanned candidate folders; null until loaded.
  let foldersLoading = false;
  let expandedAccountId = null; // Only one named account expands in place at a time.
  let renameDraft = null; // { id, value } while the user is typing in the display-name field.
  let removing = null; // { id, deleteDir } while the removal confirmation is open in detail.
  let busy = false;
  let login = null; // The open sign-in dialog: { dialog, session, id }.
  let addDialog = null; // The open add-account dialog state.

  const post = async (path, body = {}) => {
    const res = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(body),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok || data.error) throw new Error(data.error || L(`操作失败（${res.status}）`, `Failed (${res.status})`));
    return data;
  };

  const METHODS = () => ({
    "claude.ai": L("订阅登录", "Subscription"),
    api_key: "API Key",
    oauth_token: "OAuth Token",
  });

  function statusLine(id) {
    const entry = states.get(id);
    if (!entry || entry.loading) return el("span", { className: "member-muted" }, L("读取中…", "Reading…"));
    if (entry.error) return el("span", { className: "member-muted" }, L("无法读取登录状态", "Could not read the sign-in state"));
    const { logged_in: signedIn, method, email, plan } = entry.state;
    if (!signedIn) return el("span", { className: "badge badge-warning" }, L("未登录", "Not signed in"));
    const detail = [METHODS()[method] || method, email, plan].filter(Boolean).join(" · ");
    return el("span", { className: "account-status" },
      el("span", { className: "badge badge-run" }, L("已登录", "Signed in")),
      detail ? el("span", { className: "member-muted", title: detail }, detail) : null);
  }

  function accountListStatus(account) {
    void readState(account.id);
    const healthRows = (AN.providerHealth?.rows?.(S.providerHealth || {}) || [])
      .filter((row) => row.provider === PROVIDER && row.account === account.id);
    const entry = states.get(account.id);
    if (healthRows.length) {
      const plan = entry?.state?.logged_in ? entry.state.plan : null;
      return { text: plan ? L(`冷却中 · ${plan}`, `Cooldown · ${plan}`) : L("冷却中", "Cooldown"), tone: "warn" };
    }
    if (!entry || entry.loading) return { text: L("读取中…", "Reading…"), tone: "muted" };
    if (entry.error) return { text: L("无法读取状态", "Status unavailable"), tone: "bad" };
    if (!entry.state.logged_in) return { text: L("未登录", "Not signed in"), tone: "warn" };
    const tag = entry.state.plan || (entry.state.method && entry.state.method !== "claude.ai"
      ? METHODS()[entry.state.method] || entry.state.method : null);
    return { text: tag ? L(`已登录 · ${tag}`, `Signed in · ${tag}`) : L("已登录", "Signed in"), tone: "run" };
  }

  async function readState(id, force = false) {
    if (!force && states.has(id)) return;
    states.set(id, { loading: true });
    try {
      const state = await (await fetch(`/app/accounts/status?id=${encodeURIComponent(id)}`, { cache: "no-store" })).json();
      states.set(id, state.error ? { error: state.error } : { state });
    } catch (error) {
      states.set(id, { error: String(error?.message || error) });
    }
    S.renderCards();
  }

  async function readFolders() {
    foldersLoading = true;
    renderDialogFolders();
    try {
      folders = (await (await fetch("/app/accounts/scan", { cache: "no-store" })).json()).folders || [];
    } catch {
      folders = [];
    } finally {
      foldersLoading = false;
      renderDialogFolders();
    }
  }

  function take(data) {
    if (data.state) {
      S.accounts = data.state.accounts || [];
      S.accountsError = data.state.error || null;
    }
    folders = null;
  }

  function sync(data) {
    if (data.sync && data.sync.ok === false) {
      S.showNotice(L(`账号已添加，但接入没有同步成功：${data.sync.message}`, `Account added, but its connection was not synced: ${data.sync.message}`), true);
    }
  }

  async function act(run, failure) {
    if (busy) return null;
    busy = true;
    S.renderCards();
    try {
      return await run();
    } catch (error) {
      S.showNotice(`${failure}${error.message}`, true);
      return null;
    } finally {
      busy = false;
      S.renderCards();
    }
  }

  async function startLogin(account) {
    const data = await post("/app/accounts/login", { id: account.id, cols: 96, rows: 22 });
    openLogin(account, data.session);
  }

  async function commitRename(account, input, fromBlur = false) {
    const label = input.value.trim();
    if (!label || label === account.label) {
      renameDraft = null;
      input.value = account.label;
      if (!fromBlur) S.renderCards();
      return;
    }
    if (busy) return;
    busy = true;
    input.disabled = true;
    try {
      const data = await post("/app/accounts/rename", { id: account.id, label });
      renameDraft = null;
      take(data);
    } catch (error) {
      S.showNotice(`${L("改名失败：", "Could not rename: ")}${error.message}`, true);
      input.value = account.label;
      renameDraft = null;
    } finally {
      busy = false;
      S.renderCards();
    }
  }

  async function remove(account) {
    const { deleteDir } = removing;
    const data = await act(() => post("/app/accounts/remove", { id: account.id, delete_dir: deleteDir }), L("移除失败：", "Could not remove: "));
    if (!data) return;
    if (expandedAccountId === account.id) expandedAccountId = null;
    removing = null;
    renameDraft = null;
    states.delete(account.id);
    take(data);
    if (data.delete_error) S.showNotice(L(`账号已移除，但目录没有删除：${data.delete_error}`, `Account removed, but its folder was not deleted: ${data.delete_error}`), true);
  }

  // ------------------------------------------------------------------ sign-in dialog
  function closeLogin() {
    if (!login) return;
    const { dialog, session, id } = login;
    login = null;
    fetch(`/app/terminal/close?session=${session}`, { method: "POST", headers: { "X-AgentNave-Action": "1" } }).catch(() => {});
    if (dialog.open) dialog.close();
    dialog.remove();
    void readState(id, true);
  }

  function openLogin(account, session) {
    closeLogin();
    const frame = el("iframe", { className: "account-login-frame", src: `/terminal/?session=${session}`, title: L(`${account.label} 的登录终端`, `Sign-in terminal of ${account.label}`) });
    const done = el("button", { type: "button", className: "primary", onClick: closeLogin }, L("完成", "Done"));
    const dialog = el("dialog", { className: "upd account-login", "aria-label": L(`登录 ${account.label}`, `Sign in ${account.label}`) },
      el("h2", {}, account.label), frame, el("div", { className: "upd-row" }, done));
    dialog.addEventListener("cancel", (event) => { event.preventDefault(); closeLogin(); });
    document.body.appendChild(dialog);
    login = { dialog, session, id: account.id };
    dialog.showModal();
  }

  // The login CLI ends by itself once the browser step is done; the state is read again then.
  window.addEventListener("message", (event) => {
    if (event.data?.type !== "agentnave:terminal" || event.data.action !== "exited" || !login) return;
    if (event.data.session === login.session) void readState(login.id, true);
  });

  // ------------------------------------------------------------------ add-account dialog
  function renderDialogFolders() {
    if (!addDialog) return;
    const { linkBox, dialogBusy } = addDialog;
    if (!linkBox?.replaceChildren) return;
    if (foldersLoading && folders === null) {
      linkBox.replaceChildren(el("span", { className: "member-muted" }, L("扫描中…", "Scanning…")));
      return;
    }
    const items = (folders || []).map((folder) => el("button", {
      type: "button",
      disabled: dialogBusy,
      title: folder.dir,
      dataset: { focusKey: `${PROVIDER}/accounts/link-folder` },
      onClick: () => void linkFromDialog(folder.dir),
    }, folder.name));
    const pick = el("button", {
      type: "button",
      disabled: dialogBusy,
      dataset: { focusKey: `${PROVIDER}/accounts/link-pick` },
      onClick: () => void linkFromDialog("pick"),
    }, L("选择文件夹…", "Choose a folder…"));
    linkBox.replaceChildren(...items, pick);
  }

  function setDialogBusy(nextBusy) {
    if (!addDialog) return;
    addDialog.dialogBusy = nextBusy;
    const { input, submit, cancel } = addDialog;
    input.disabled = nextBusy;
    cancel.disabled = nextBusy;
    submit.disabled = nextBusy || !input.value.trim();
    submit.textContent = nextBusy ? L("创建中…", "Creating…") : L("下一步", "Continue");
    renderDialogFolders();
  }

  function setDialogError(message) {
    if (!addDialog) return;
    const { errorEl } = addDialog;
    errorEl.textContent = message || "";
    errorEl.hidden = !message;
  }

  function closeAddDialog() {
    if (!addDialog || addDialog.dialogBusy) return;
    const { dialog } = addDialog;
    addDialog = null;
    if (dialog.open) dialog.close();
    dialog.remove();
  }

  async function createFromDialog() {
    if (!addDialog || addDialog.dialogBusy || busy) return;
    const label = addDialog.input.value.trim();
    if (!label) return;
    setDialogError("");
    setDialogBusy(true);
    let created;
    try {
      created = await post("/app/accounts/create", { label });
    } catch (error) {
      setDialogBusy(false);
      setDialogError(`${L("新建失败：", "Could not create: ")}${error.message}`);
      addDialog?.input.focus?.();
      return;
    }
    take(created);
    sync(created);
    S.selectedMember = "cli:claude";
    expandedAccountId = created.account.id;
    removing = null;
    addDialog.dialogBusy = false;
    closeAddDialog();
    S.renderCards();
    void readState(created.account.id, true);
    await act(() => startLogin(created.account), L("无法打开登录终端：", "Could not open the sign-in terminal: "));
  }

  async function linkFromDialog(value) {
    if (!addDialog || addDialog.dialogBusy || busy) return;
    const dir = value === "pick" ? undefined : value;
    const label = addDialog.input.value.trim();
    setDialogError("");
    setDialogBusy(true);
    let data;
    try {
      data = await post("/app/accounts/link", { ...(dir ? { dir } : {}), ...(label ? { label } : {}) });
    } catch (error) {
      setDialogBusy(false);
      setDialogError(`${L("关联失败：", "Could not link: ")}${error.message}`);
      return;
    }
    if (data.cancelled) {
      setDialogBusy(false);
      return;
    }
    take(data);
    sync(data);
    S.selectedMember = "cli:claude";
    expandedAccountId = data.account.id;
    removing = null;
    addDialog.dialogBusy = false;
    closeAddDialog();
    S.renderCards();
    if (data.shared_sessions) {
      S.showNotice(L(`已关联。这个目录与已有账号有 ${data.shared_sessions} 个相同的会话 ID，续接它们可能串到另一个账号`, `Linked. This folder shares ${data.shared_sessions} session IDs with another account; continuing them may cross accounts`), true);
    }
    void readState(data.account.id, true);
  }

  function openAddDialog() {
    if (busy || S.accountsError) return;
    if (addDialog) return;
    const input = el("input", {
      id: "account-add-name",
      type: "text",
      maxlength: "40",
      spellcheck: "false",
      placeholder: L("例如：工作、个人", "e.g. Work, Personal"),
      dataset: { focusKey: `${PROVIDER}/accounts/new` },
      onInput: () => {
        if (!addDialog) return;
        setDialogError("");
        addDialog.submit.disabled = addDialog.dialogBusy || !input.value.trim();
      },
    });
    const form = el("form", {
      id: "account-add-form",
      className: "account-add-form",
      onSubmit: (event) => {
        event.preventDefault();
        void createFromDialog();
      },
    }, el("div", { className: "account-add-field" },
      el("label", { htmlFor: "account-add-name" }, L("账号名称", "Account name")),
      input));
    const errorEl = el("p", { className: "member-error account-add-error", role: "status", hidden: true });
    const linkBox = el("div", { className: "account-link-body" });
    const linkSection = el("details", { className: "account-link-section" },
      el("summary", {}, L("关联已有目录", "Link existing folder")),
      linkBox);
    const cancel = el("button", {
      type: "button",
      onClick: closeAddDialog,
    }, L("取消", "Cancel"));
    const submit = el("button", {
      type: "submit",
      form: "account-add-form",
      className: "primary",
      disabled: true,
      dataset: { focusKey: `${PROVIDER}/accounts/create` },
    }, L("下一步", "Continue"));
    const dialog = el("dialog", {
      className: "upd account-add-dialog",
      "aria-labelledby": "account-add-title",
    },
    el("h2", { id: "account-add-title", className: "upd-title" }, L("添加账号", "Add account")),
    form,
    errorEl,
    linkSection,
    el("div", { className: "upd-row" }, cancel, submit));
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      closeAddDialog();
    });
    addDialog = { dialog, input, submit, cancel, linkBox, errorEl, dialogBusy: false };
    document.body.appendChild(dialog);
    dialog.showModal();
    input.focus?.({ preventScroll: true });
    void readFolders();
  }

  // ------------------------------------------------------------------ Claude detail accounts group
  function accountHasDirty() {
    if (!renameDraft) return false;
    const account = (S.accounts || []).find((a) => a.id === renameDraft.id);
    if (!account) return false;
    const trimmed = renameDraft.value.trim();
    return Boolean(trimmed) && trimmed !== account.label;
  }

  function accountHasError() {
    if (S.accountsError) return true;
    if (!renameDraft) return false;
    return renameDraft.value.trim() === "";
  }

  function clearAccountPending() {
    removing = null;
  }

  function confirmRemoval(account) {
    const folder = account.managed
      ? el("label", { className: "account-delete" },
        el("input", {
          type: "checkbox", checked: removing.deleteDir, dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/delete-dir` },
          onChange: (event) => { removing = { ...removing, deleteDir: event.target.checked }; S.renderCards(); },
        }), L("同时删除账号目录（登录和聊天记录一并删除）", "Also delete the account folder (sign-in and chats)"))
      : null;
    return el("div", { className: "disable-confirm", role: "group", "aria-label": L(`确认移除 ${account.label}`, `Confirm removing ${account.label}`) },
      el("p", {}, L(`移除“${account.label}”后不再用它派活，账号目录里的登录和聊天记录保留。`, `After removing “${account.label}” it is no longer used for dispatching; the sign-in and chats in its folder stay.`)),
      folder,
      el("div", { className: "btn-group" },
        el("button", { type: "button", disabled: busy, onClick: () => { removing = null; S.renderCards(); } }, L("保留", "Keep")),
        el("button", {
          type: "button", className: "danger", disabled: busy, dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/confirm-remove` },
          onClick: () => void remove(account),
        }, removing.deleteDir ? L("移除并删除目录", "Remove and delete folder") : L("确认移除", "Remove"))));
  }

  function renderAddAccountButton() {
    return el("button", {
      type: "button",
      className: "accounts-add-btn",
      disabled: busy || Boolean(S.accountsError),
      dataset: { focusKey: `${PROVIDER}/accounts/add` },
      onClick: openAddDialog,
    }, L("添加账号", "Add account"));
  }

  function renderAccountItem(account) {
    const expanded = expandedAccountId === account.id;
    const status = accountListStatus(account);
    const toggle = el("button", {
      type: "button",
      className: "account-item-toggle",
      "aria-expanded": String(expanded),
      dataset: {
        focusKey: `${PROVIDER}/accounts/${account.id}/toggle`,
        account: account.id,
      },
      onClick: () => {
        if (expanded) {
          expandedAccountId = null;
          if (removing?.id === account.id) removing = null;
        } else {
          expandedAccountId = account.id;
          removing = null;
        }
        S.renderCards();
      },
    },
    el("span", { className: "account-item-name", title: account.label }, account.label),
    el("span", { className: "account-item-meta" },
      el("span", { className: `member-status-dot tone-${status.tone}`, "aria-hidden": "true" }),
      el("span", { className: "account-item-status", title: status.text }, status.text),
      el("span", { className: "cli-chevron account-item-chevron", "aria-hidden": "true" })));

    if (!expanded) {
      return el("div", { className: "account-item", dataset: { account: account.id } }, toggle);
    }

    const signedIn = states.get(account.id)?.state?.logged_in;
    const inputId = `account-label-${account.id}`;
    const value = renameDraft?.id === account.id ? renameDraft.value : account.label;

    const loginBtn = el("button", {
      type: "button",
      className: "member-toggle account-action-login",
      disabled: busy,
      dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/login` },
      onClick: () => void act(() => startLogin(account), L("无法打开登录终端：", "Could not open the sign-in terminal: ")),
    }, signedIn ? L("重新登录", "Sign in again") : L("登录", "Sign in"));

    const nameInput = el("input", {
      id: inputId,
      type: "text",
      className: "account-detail-input",
      maxlength: "40",
      spellcheck: "false",
      value,
      disabled: busy,
      dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/label-input` },
      onInput: (event) => {
        renameDraft = { id: account.id, value: event.target.value };
        S.refreshClaudeFlags?.();
      },
      onKeydown: (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          void commitRename(account, event.target);
        } else if (event.key === "Escape") {
          event.preventDefault();
          renameDraft = null;
          event.target.value = account.label;
          S.refreshClaudeFlags?.();
          event.target.blur?.();
        }
      },
      onBlur: (event) => { void commitRename(account, event.target, true); },
    });

    const removeBtn = el("button", {
      type: "button",
      className: "member-toggle account-action-remove",
      disabled: busy || removing !== null,
      dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/remove` },
      onClick: () => { removing = { id: account.id, deleteDir: false }; S.renderCards(); },
    }, L("移除", "Remove"));

    const body = el("div", { className: "account-item-body" },
      el("div", { className: "account-field-row" },
        el("label", { className: "account-field-label", htmlFor: inputId }, L("显示名", "Display name")),
        el("div", { className: "account-field-control" }, nameInput)),
      el("div", { className: "account-field-row" },
        el("span", { className: "account-field-label" }, L("登录状态", "Sign-in status")),
        el("div", { className: "account-detail-status" }, statusLine(account.id), loginBtn)),
      removing?.id === account.id ? confirmRemoval(account) : null,
      el("div", { className: "account-item-actions" }, removeBtn));

    return el("div", { className: "account-item is-expanded", dataset: { account: account.id } }, toggle, body);
  }

  function renderClaudeAccountsSection() {
    const accounts = S.accounts || [];
    if (expandedAccountId && !accounts.some((account) => account.id === expandedAccountId)) {
      expandedAccountId = null;
    }
    if (removing && !accounts.some((account) => account.id === removing.id)) {
      removing = null;
    }
    const content = [
      accounts.length ? el("div", { className: "accounts-list" }, ...accounts.map(renderAccountItem)) : null,
      el("div", { className: "accounts-group-actions" }, renderAddAccountButton()),
    ];
    return el("div", { className: "member-row claude-accounts-row" },
      el("span", { className: "member-scene" }, L("账号", "Accounts")),
      el("div", { className: "accounts-group" }, ...content));
  }

  Object.assign(S, {
    renderClaudeAccountsSection,
    accountHasDirty,
    accountHasError,
    clearAccountPending,
    accountListStatus,
    openAddDialog,
  });
  S.share({
    expandedAccountId: [() => expandedAccountId, (v) => { expandedAccountId = v; }],
  });
}
