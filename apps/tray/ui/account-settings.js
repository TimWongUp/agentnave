"use strict";
{
  // Settings -> 员工 -> Claude Code accounts (ADR-0107): default account status at top of
  // Claude Code detail, expandable account list (default + named accounts), and a 2-step add
  // wizard (name -> in-dialog sign-in). Sign-in state is read on demand and never saved to disk.

  const S = AN.settings ||= {};
  const { $, L, el } = S;
  const PROVIDER = "claude";
  const DEFAULT_ID = "default";

  // Where the sign-in states stand: id -> { loading } | { state } | { error }.
  const states = new Map();
  const revealedEmails = new Set(); // Account IDs whose email is currently unmasked in the UI.
  let expandedAccountId = null; // Only one account row expands in place at a time.
  let renameDraft = null; // { id, value } while the user is typing in the display-name field.
  let removing = null; // { id, deleteDir } while the removal confirmation is open in detail.
  let busy = false;
  let login = null; // The open sign-in dialog: { dialog, session, id }.
  let addDialog = null; // The open 2-step add-account wizard dialog state.

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

  function maskEmail(email) {
    const text = String(email || "").trim();
    if (!text) return "";
    const at = text.lastIndexOf("@");
    if (at <= 0) {
      const keep = text.slice(0, text.length > 2 ? 2 : 1);
      return `${keep}***`;
    }
    const local = text.slice(0, at);
    const domain = text.slice(at + 1);
    const localShown = local.slice(0, local.length > 2 ? 2 : 1);
    const dot = domain.lastIndexOf(".");
    const domainMasked = dot > 0 ? `***${domain.slice(dot)}` : "***";
    return `${localShown}***@${domainMasked}`;
  }

  function statusTag(state) {
    return state?.plan || (state?.method && state.method !== "claude.ai"
      ? METHODS()[state.method] || state.method : null);
  }

  function emailToggleButton(id, email, focusKey) {
    const value = String(email || "").trim();
    if (!value) return null;
    const revealed = revealedEmails.has(id);
    const label = revealed ? L("点击隐藏邮箱", "Click to hide email") : L("点击显示邮箱", "Click to reveal email");
    return el("button", {
      type: "button",
      className: `account-email-toggle${revealed ? " is-revealed" : ""}`,
      "aria-pressed": String(revealed),
      "aria-label": label,
      title: label,
      dataset: focusKey ? { focusKey } : undefined,
      onClick: () => {
        if (revealedEmails.has(id)) revealedEmails.delete(id);
        else revealedEmails.add(id);
        S.renderCards();
        renderWizardStatus();
      },
    }, revealed ? value : maskEmail(value));
  }

  function statusLine(id, focusKey) {
    const entry = states.get(id);
    if (!entry || entry.loading) return el("span", { className: "member-muted" }, L("读取中…", "Reading…"));
    if (entry.error) return el("span", { className: "member-muted" }, L("无法读取登录状态", "Could not read the sign-in state"));
    const { logged_in: signedIn, email } = entry.state;
    if (!signedIn) return el("span", { className: "badge badge-warning" }, L("未登录", "Not signed in"));
    const tag = statusTag(entry.state);
    return el("span", { className: "account-status" },
      el("span", { className: "badge badge-run" }, L("已登录", "Signed in")),
      emailToggleButton(id, email, focusKey),
      tag ? el("span", { className: "member-muted" }, `· ${tag}`) : null);
  }

  function claudeDefaultListStatus(info) {
    if (!info?.installed) return null;
    void readState(DEFAULT_ID);
    const entry = states.get(DEFAULT_ID);
    if (!entry || entry.loading) return { text: L("读取中…", "Reading…"), tone: "muted" };
    if (entry.error) return { text: L("无法读取状态", "Status unavailable"), tone: "bad" };
    if (!entry.state.logged_in) return { text: L("未登录", "Not signed in"), tone: "warn" };
    const tag = statusTag(entry.state);
    return { text: tag ? L(`已登录 · ${tag}`, `Signed in · ${tag}`) : L("已登录", "Signed in"), tone: "run" };
  }

  function renderClaudeTopStatus(info) {
    if (!info?.installed) return null;
    void readState(DEFAULT_ID);
    const entry = states.get(DEFAULT_ID);
    if (!entry || entry.loading) {
      return el("div", { className: "detail-account-bar", dataset: { status: "loading" } },
        el("span", { className: "member-status-dot tone-muted", "aria-hidden": "true" }),
        el("span", { className: "member-muted" }, L("读取中…", "Reading…")));
    }
    if (entry.error) {
      return el("div", { className: "detail-account-bar", dataset: { status: "error" } },
        el("span", { className: "member-status-dot tone-bad", "aria-hidden": "true" }),
        el("span", { className: "member-muted" }, L("无法读取登录状态", "Could not read the sign-in state")));
    }
    if (!entry.state.logged_in) {
      return el("div", { className: "detail-account-bar", dataset: { status: "signed-out" } },
        el("span", { className: "member-status-dot tone-warn", "aria-hidden": "true" }),
        el("span", { className: "detail-account-text" }, L("未登录", "Not signed in")));
    }
    const tag = statusTag(entry.state);
    const emailBtn = emailToggleButton(DEFAULT_ID, entry.state.email, "claude/default-email");
    return el("div", { className: "detail-account-bar", dataset: { status: "signed-in" } },
      el("span", { className: "member-status-dot tone-run", "aria-hidden": "true" }),
      el("span", { className: "detail-account-text" }, L("已登录", "Signed in")),
      emailBtn,
      tag ? el("span", { className: "detail-account-plan" }, `· ${tag}`) : null);
  }

  function accountListStatus(account) {
    void readState(account.id);
    const healthRows = (AN.providerHealth?.rows?.(S.providerHealth || {}) || [])
      .filter((row) => row.provider === PROVIDER && (account.isDefault ? !row.account : row.account === account.id));
    const entry = states.get(account.id);
    if (healthRows.length) {
      const plan = entry?.state?.logged_in ? entry.state.plan : null;
      return { text: plan ? L(`冷却中 · ${plan}`, `Cooldown · ${plan}`) : L("冷却中", "Cooldown"), tone: "warn" };
    }
    if (!entry || entry.loading) return { text: L("读取中…", "Reading…"), tone: "muted" };
    if (entry.error) return { text: L("无法读取状态", "Status unavailable"), tone: "bad" };
    if (!entry.state.logged_in) return { text: L("未登录", "Not signed in"), tone: "warn" };
    const email = entry.state.email ? (revealedEmails.has(account.id) ? entry.state.email : maskEmail(entry.state.email)) : null;
    const tag = statusTag(entry.state);
    const parts = [email || L("已登录", "Signed in"), tag].filter(Boolean);
    return { text: parts.join(" · "), tone: "run" };
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
    renderWizardStatus();
  }

  function take(data) {
    if (data.state) {
      S.accounts = data.state.accounts || [];
      S.accountsError = data.state.error || null;
    }
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
    revealedEmails.delete(account.id);
    take(data);
    if (data.delete_error) S.showNotice(L(`账号已移除，但目录没有删除：${data.delete_error}`, `Account removed, but its folder was not deleted: ${data.delete_error}`), true);
  }

  function createLoginFrame(session, label) {
    const frame = el("iframe", {
      className: "account-login-frame",
      src: `/terminal/?session=${session}`,
      title: L(`${label} 的登录终端`, `Sign-in terminal of ${label}`),
    });
    AN.onMockTerminalFrame?.(frame, session, label);
    return frame;
  }

  // ------------------------------------------------------------------ standalone sign-in dialog
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
    const frame = createLoginFrame(session, account.label);
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
    if (event.data?.type !== "agentnave:terminal" || event.data.action !== "exited") return;
    if (login && event.data.session === login.session) {
      void readState(login.id, true);
    }
    if (addDialog?.step === 2 && addDialog.account && event.data.session === addDialog.session) {
      void readState(addDialog.account.id, true);
    }
  });

  // ------------------------------------------------------------------ 2-step add-account wizard dialog
  function renderWizardStatus() {
    if (!addDialog || addDialog.step !== 2 || !addDialog.account) return;
    const { account, statusBox, finishBtn } = addDialog;
    const entry = states.get(account.id);
    const signedIn = Boolean(entry?.state?.logged_in);
    if (statusBox?.replaceChildren) {
      statusBox.replaceChildren(statusLine(account.id, `claude/accounts/${account.id}/wizard-email`));
    }
    if (finishBtn) {
      finishBtn.replaceChildren(signedIn ? L("完成", "Done") : L("稍后登录", "Sign in later"));
      finishBtn.className = signedIn ? "primary" : "";
    }
  }

  function scheduleWizardPoll(accountId) {
    if (!addDialog || addDialog.step !== 2 || addDialog.account?.id !== accountId) return;
    if (typeof setTimeout !== "function") return;
    if (addDialog.pollTimer) clearTimeout(addDialog.pollTimer);
    addDialog.pollTimer = setTimeout(async () => {
      if (!addDialog || addDialog.step !== 2 || addDialog.account?.id !== accountId) return;
      if (states.get(accountId)?.state?.logged_in) return;
      await readState(accountId, true);
      if (addDialog && addDialog.step === 2 && !states.get(accountId)?.state?.logged_in) {
        scheduleWizardPoll(accountId);
      }
    }, 3000);
  }

  function setDialogBusy(nextBusy) {
    if (!addDialog || addDialog.step !== 1) return;
    addDialog.dialogBusy = nextBusy;
    const { input, submit, cancel } = addDialog;
    input.disabled = nextBusy;
    cancel.disabled = nextBusy;
    submit.disabled = nextBusy || !input.value.trim();
    submit.replaceChildren(nextBusy ? L("创建中…", "Creating…") : L("下一步", "Continue"));
  }

  function setDialogError(message) {
    if (!addDialog) return;
    const { errorEl } = addDialog;
    errorEl.textContent = message || "";
    errorEl.hidden = !message;
  }

  function closeAddDialog() {
    if (!addDialog || addDialog.dialogBusy) return;
    const { dialog, session, account, pollTimer } = addDialog;
    if (pollTimer && typeof clearTimeout === "function") clearTimeout(pollTimer);
    addDialog = null;
    if (session != null) {
      fetch(`/app/terminal/close?session=${session}`, { method: "POST", headers: { "X-AgentNave-Action": "1" } }).catch(() => {});
    }
    if (dialog.open) dialog.close();
    dialog.remove();
    if (account) void readState(account.id, true);
  }

  async function enterWizardLoginStep(account) {
    if (!addDialog) return;
    addDialog.step = 2;
    addDialog.account = account;
    addDialog.dialogBusy = false;
    addDialog.dialog.className = "upd account-add-dialog is-login-step";
    addDialog.step1Badge.className = "account-wizard-step is-done";
    addDialog.step2Badge.className = "account-wizard-step is-active";

    const statusBox = el("div", { className: "account-wizard-status" });
    const frameSlot = el("div", { className: "account-wizard-frame-slot" });
    const finishBtn = el("button", {
      type: "button",
      dataset: { focusKey: `${PROVIDER}/accounts/wizard-finish` },
      onClick: closeAddDialog,
    }, L("稍后登录", "Sign in later"));

    addDialog.statusBox = statusBox;
    addDialog.finishBtn = finishBtn;
    addDialog.stepBody.replaceChildren(statusBox, frameSlot);
    addDialog.footer.replaceChildren(finishBtn);
    setDialogError("");
    renderWizardStatus();

    try {
      const data = await post("/app/accounts/login", { id: account.id, cols: 96, rows: 22 });
      if (!addDialog || addDialog.account?.id !== account.id) return;
      addDialog.session = data.session;
      frameSlot.replaceChildren(createLoginFrame(data.session, account.label));
      scheduleWizardPoll(account.id);
    } catch (error) {
      if (!addDialog || addDialog.account?.id !== account.id) return;
      setDialogError(`${L("无法打开登录终端：", "Could not open the sign-in terminal: ")}${error.message}`);
    }
    void readState(account.id, true);
  }

  async function createFromDialog() {
    if (!addDialog || addDialog.step !== 1 || addDialog.dialogBusy || busy) return;
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
    S.renderCards();
    await enterWizardLoginStep(created.account);
  }

  function openAddDialog() {
    if (busy || S.accountsError) return;
    if (addDialog) return;
    const step1Badge = el("span", { className: "account-wizard-step is-active" },
      el("span", { className: "account-wizard-num" }, "1"),
      L("显示名", "Display name"));
    const step2Badge = el("span", { className: "account-wizard-step" },
      el("span", { className: "account-wizard-num" }, "2"),
      L("登录", "Sign in"));
    const steps = el("div", { className: "account-wizard-steps", "aria-hidden": "true" }, step1Badge, step2Badge);

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
      el("label", { htmlFor: "account-add-name" }, L("显示名", "Display name")),
      input));
    const stepBody = el("div", { className: "account-wizard-body" }, form);
    const errorEl = el("p", { className: "member-error account-add-error", role: "status", hidden: true });
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
    const footer = el("div", { className: "upd-row" }, cancel, submit);
    const dialog = el("dialog", {
      className: "upd account-add-dialog",
      "aria-labelledby": "account-add-title",
    },
    el("h2", { id: "account-add-title", className: "upd-title" }, L("添加账号", "Add account")),
    steps,
    stepBody,
    errorEl,
    footer);
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      closeAddDialog();
    });
    addDialog = {
      dialog, step: 1, input, submit, cancel, stepBody, footer, errorEl,
      step1Badge, step2Badge, dialogBusy: false, account: null, session: null,
      statusBox: null, finishBtn: null, pollTimer: null,
    };
    document.body.appendChild(dialog);
    dialog.showModal();
    input.focus?.({ preventScroll: true });
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
    const loginBtn = el("button", {
      type: "button",
      className: "member-toggle account-action-login",
      disabled: busy,
      dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/login` },
      onClick: () => void act(() => startLogin(account), L("无法打开登录终端：", "Could not open the sign-in terminal: ")),
    }, signedIn ? L("重新登录", "Sign in again") : L("登录", "Sign in"));

    const statusRow = el("div", { className: "account-field-row" },
      el("span", { className: "account-field-label" }, L("登录状态", "Sign-in status")),
      el("div", { className: "account-detail-status" },
        statusLine(account.id, `${PROVIDER}/accounts/${account.id}/email`),
        loginBtn));

    if (account.isDefault) {
      return el("div", { className: "account-item is-expanded is-default", dataset: { account: account.id } },
        toggle,
        el("div", { className: "account-item-body" }, statusRow));
    }

    const inputId = `account-label-${account.id}`;
    const value = renameDraft?.id === account.id ? renameDraft.value : account.label;
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
      statusRow,
      removing?.id === account.id ? confirmRemoval(account) : null,
      el("div", { className: "account-item-actions" }, removeBtn));

    return el("div", { className: "account-item is-expanded", dataset: { account: account.id } }, toggle, body);
  }

  function renderClaudeAccountsSection() {
    const accounts = S.accounts || [];
    if (expandedAccountId && expandedAccountId !== DEFAULT_ID && !accounts.some((account) => account.id === expandedAccountId)) {
      expandedAccountId = null;
    }
    if (removing && !accounts.some((account) => account.id === removing.id)) {
      removing = null;
    }
    const items = [
      { id: DEFAULT_ID, label: L("默认", "Default"), isDefault: true },
      ...accounts,
    ];
    return el("div", { className: "s-row claude-accounts-row" },
      el("span", { className: "s-label" }, L("账号", "Accounts")),
      el("div", { className: "accounts-group" },
        el("div", { className: "accounts-list" }, ...items.map(renderAccountItem)),
        el("div", { className: "accounts-group-actions" }, renderAddAccountButton())));
  }

  Object.assign(S, {
    renderClaudeAccountsSection,
    renderClaudeTopStatus,
    claudeDefaultListStatus,
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
