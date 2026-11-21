"use strict";
{
  // Settings -> 员工 -> Claude Code named accounts (ADR-0107): each account renders as a
  // sibling employee card after the Claude Code card. Sign-in state is read on demand and never
  // written to disk.

  const S = AN.settings ||= {};
  const { $, L, el } = S;
  const PROVIDER = "claude";

  // Where the sign-in states stand: id -> { loading } | { state } | { error }.
  const states = new Map();
  let folders = null; // Scanned candidate folders; null until loaded.
  let foldersLoading = false;
  let editing = null; // { id, value } while an account card title is being renamed inline.
  let removing = null; // { id, deleteDir } while the removal confirmation is open on a card.
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

  function startRename(account) {
    if (busy) return;
    editing = { id: account.id, value: account.label };
    S.renderCards();
    const input = document.querySelector?.(`[data-focus-key="${PROVIDER}/accounts/${account.id}/label-input"]`);
    if (input && typeof input.focus === "function") {
      input.focus({ preventScroll: true });
      input.select?.();
    }
  }

  async function commitRename(account, input, fromBlur = false) {
    const label = input.value.trim();
    if (!label || label === account.label) {
      editing = null;
      if (!fromBlur) {
        S.renderCards();
        document.querySelector?.(`[data-focus-key="${PROVIDER}/accounts/${account.id}/label"]`)?.focus({ preventScroll: true });
      }
      return;
    }
    if (busy) return;
    busy = true;
    input.disabled = true;
    try {
      const data = await post("/app/accounts/rename", { id: account.id, label });
      editing = null;
      take(data);
    } catch (error) {
      S.showNotice(`${L("改名失败：", "Could not rename: ")}${error.message}`, true);
      input.value = account.label;
      editing = null;
    } finally {
      busy = false;
      S.renderCards();
      if (!fromBlur) {
        document.querySelector?.(`[data-focus-key="${PROVIDER}/accounts/${account.id}/label"]`)?.focus({ preventScroll: true });
      }
    }
  }

  async function finishRenameOnBlur(account, input) {
    if (editing?.id !== account.id) return;
    const label = input.value.trim();
    if (!label || label === account.label) {
      editing = null;
      // Defer re-render so a concurrent click on Login/Remove is not detached before firing.
      setTimeout(() => { if (!editing) S.renderCards(); }, 150);
      return;
    }
    await commitRename(account, input, true);
  }

  async function remove(account) {
    const { deleteDir } = removing;
    const data = await act(() => post("/app/accounts/remove", { id: account.id, delete_dir: deleteDir }), L("移除失败：", "Could not remove: "));
    if (!data) return;
    removing = null;
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

  // ------------------------------------------------------------------ cards
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
      className: "member-toggle account-add-btn",
      disabled: busy || Boolean(S.accountsError),
      dataset: { focusKey: `${PROVIDER}/accounts/add` },
      onClick: openAddDialog,
    }, L("添加账号", "Add account"));
  }

  function accountTitle(account) {
    const cliTitle = S.PROVIDER_NAMES?.[PROVIDER] || "Claude Code";
    if (editing?.id === account.id) {
      const input = el("input", {
        type: "text",
        className: "account-title-input",
        maxlength: "40",
        spellcheck: "false",
        value: editing.value,
        disabled: busy,
        "aria-label": L("账号名称", "Account name"),
        dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/label-input` },
        onInput: (event) => { if (editing?.id === account.id) editing.value = event.target.value; },
        onKeydown: (event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            void commitRename(account, event.target);
          } else if (event.key === "Escape") {
            event.preventDefault();
            editing = null;
            S.renderCards();
            document.querySelector?.(`[data-focus-key="${PROVIDER}/accounts/${account.id}/label"]`)?.focus({ preventScroll: true });
          }
        },
        onBlur: (event) => { void finishRenameOnBlur(account, event.target); },
      });
      return el("span", { className: "cli-name account-title-editing" },
        L(`${cliTitle}（`, `${cliTitle} (`),
        input,
        L("）", ")"));
    }
    const display = L(`${cliTitle}（${account.label}）`, `${cliTitle} (${account.label})`);
    return el("button", {
      type: "button",
      className: "cli-name account-title-btn",
      disabled: busy,
      dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/label` },
      title: L("点击修改账号名称", "Click to rename"),
      "aria-label": L(`${display}，点击修改账号名称`, `${display}, click to rename`),
      onClick: () => startRename(account),
    }, el("span", { className: "account-title-text" }, display),
       el("span", { className: "account-title-pencil", "aria-hidden": "true" }, "✎"));
  }

  function accountCard(account) {
    const cliTitle = S.PROVIDER_NAMES?.[PROVIDER] || "Claude Code";
    const signedIn = states.get(account.id)?.state?.logged_in;
    const head = el("div", { className: "account-head" },
      el("div", { className: "member-title" },
        el("span", { className: "cli-icon", "aria-hidden": "true" }, AN.hostTile(PROVIDER, cliTitle)),
        accountTitle(account)),
      statusLine(account.id),
      el("button", {
        type: "button",
        className: "member-toggle account-action-login",
        disabled: busy,
        dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/login` },
        onClick: () => void act(() => startLogin(account), L("无法打开登录终端：", "Could not open the sign-in terminal: ")),
      }, signedIn ? L("重新登录", "Sign in again") : L("登录", "Sign in")),
      el("button", {
        type: "button",
        className: "member-toggle account-action-remove",
        disabled: busy || removing !== null,
        dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/remove` },
        onClick: () => { removing = { id: account.id, deleteDir: false }; S.renderCards(); },
      }, L("移除", "Remove")));
    return el("div", {
      className: "cli-card member-card account-card",
      dataset: { provider: PROVIDER, account: account.id },
    },
    head,
    removing?.id === account.id ? confirmRemoval(account) : null);
  }

  function renderAccountCards() {
    const accounts = S.accounts || [];
    for (const account of accounts) void readState(account.id);
    return accounts.map(accountCard);
  }

  Object.assign(S, { renderAddAccountButton, renderAccountCards, openAddDialog });
}
