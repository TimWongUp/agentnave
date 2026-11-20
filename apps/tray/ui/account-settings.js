"use strict";
{
  // Settings -> 员工 -> Claude Code card: the named accounts (ADR-0106). Each one is a folder with
  // its own sign-in; the default account is Claude Code as the machine has it. The sign-in state
  // (e-mail, plan) is read on the spot, shown here and kept nowhere.

  const S = AN.settings ||= {};
  const { $, L, el } = S;
  const PROVIDER = "claude";

  // Where the sign-in states stand: id -> { loading } | { state } | { error }; "default" is the
  // default account's.
  const states = new Map();
  let folders = null; // Folders a scan found, to offer as links; null until looked for.
  let adding = null; // The name typed for a new account while its row is open.
  let removing = null; // { id, deleteDir } while the removal waits for its confirmation.
  let busy = false;
  let login = null; // The open sign-in dialog: { dialog, session, id }.

  const post = async (path, body = {}) => {
    const res = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" }, body: JSON.stringify(body) });
    const data = await res.json().catch(() => ({}));
    if (!res.ok || data.error) throw new Error(data.error || L(`操作失败（${res.status}）`, `Failed (${res.status})`));
    return data;
  };

  const METHODS = () => ({
    "claude.ai": L("订阅登录", "Subscription"),
    api_key: "API Key",
    oauth_token: "OAuth Token",
  });

  // The row's sign-in line: whether signed in, how, and who.
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
    try {
      folders = (await (await fetch("/app/accounts/scan", { cache: "no-store" })).json()).folders || [];
    } catch {
      folders = [];
    }
    S.renderCards();
  }

  // What a change to the accounts leaves: the list as the App holds it now.
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

  async function create() {
    const label = (adding || "").trim();
    if (!label) return;
    const data = await act(() => post("/app/accounts/create", { label }), L("新建失败：", "Could not create: "));
    if (!data) return;
    adding = null;
    take(data);
    sync(data);
    S.renderCards();
    void readState(data.account.id, true);
    await act(() => startLogin(data.account), L("无法打开登录终端：", "Could not open the sign-in terminal: "));
  }

  async function link(value) {
    const dir = value === "pick" ? undefined : value;
    const data = await act(() => post("/app/accounts/link", { dir }), L("关联失败：", "Could not link: "));
    if (!data || data.cancelled) return;
    take(data);
    sync(data);
    if (data.shared_sessions) {
      S.showNotice(L(`已关联。这个目录与已有账号有 ${data.shared_sessions} 个相同的会话 ID，续接它们可能串到另一个账号`, `Linked. This folder shares ${data.shared_sessions} session IDs with another account; continuing them may cross accounts`), true);
    }
    void readState(data.account.id, true);
  }

  async function rename(account, input) {
    const label = input.value.trim();
    if (label === account.label) return;
    const data = await act(() => post("/app/accounts/rename", { id: account.id, label }), L("改名失败：", "Could not rename: "));
    if (data) take(data);
    else input.value = account.label;
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

  // ------------------------------------------------------------------ rows
  const row = (...cells) => el("div", { className: "account-row" }, ...cells);

  function headRow() {
    const scanned = folders || [];
    const add = el("button", {
      type: "button", disabled: busy || adding !== null || Boolean(S.accountsError), dataset: { focusKey: `${PROVIDER}/accounts/add` },
      onClick: () => { adding = ""; S.renderCards(); },
    }, L("新建账号", "New account"));
    const choose = el("select", {
      className: "account-link", disabled: busy || Boolean(S.accountsError), dataset: { focusKey: `${PROVIDER}/accounts/link` },
      "aria-label": L("关联已有目录", "Link an existing folder"),
      onFocus: () => { if (folders === null) void readFolders(); },
      onChange: (event) => { const value = event.target.value; event.target.value = ""; if (value) void link(value); },
    },
    el("option", { value: "", selected: true }, L("关联已有目录", "Link an existing folder")),
    ...scanned.map((folder) => el("option", { value: folder.dir, title: folder.dir }, folder.name)),
    el("option", { value: "pick" }, L("选择文件夹…", "Choose a folder…")));
    return row(el("span", { className: "member-scene" }, L("账号", "Accounts")), choose, add, el("span"));
  }

  function defaultRow() {
    return row(el("span", { className: "member-scene" }, L("默认账号", "Default")), statusLine("default"), el("span"), el("span"));
  }

  function accountRow(account) {
    const input = el("input", {
      type: "text", className: "account-label", maxlength: "40", spellcheck: "false", value: account.label,
      "aria-label": L("账号名称", "Account name"), dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/label` },
      disabled: busy,
      onChange: (event) => void rename(account, event.target),
    });
    const signedIn = states.get(account.id)?.state?.logged_in;
    return row(input, statusLine(account.id),
      el("button", {
        type: "button", disabled: busy, dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/login` },
        onClick: () => void act(() => startLogin(account), L("无法打开登录终端：", "Could not open the sign-in terminal: ")),
      }, signedIn ? L("重新登录", "Sign in again") : L("登录", "Sign in")),
      el("button", {
        type: "button", disabled: busy || removing !== null, dataset: { focusKey: `${PROVIDER}/accounts/${account.id}/remove` },
        onClick: () => { removing = { id: account.id, deleteDir: false }; S.renderCards(); },
      }, L("移除", "Remove")));
  }

  function addRow() {
    const input = el("input", {
      type: "text", className: "account-label", maxlength: "40", spellcheck: "false", value: adding,
      "aria-label": L("账号名称", "Account name"), placeholder: L("账号名称", "Account name"), dataset: { focusKey: `${PROVIDER}/accounts/new` },
      disabled: busy,
      onInput: (event) => { adding = event.target.value; submit.disabled = busy || !adding.trim(); },
      onKeydown: (event) => { if (event.key === "Enter" && adding.trim()) void create(); },
    });
    const submit = el("button", {
      type: "button", className: "primary", disabled: busy || !adding.trim(), dataset: { focusKey: `${PROVIDER}/accounts/create` },
      onClick: () => void create(),
    }, L("创建并登录", "Create and sign in"));
    const cancel = el("button", { type: "button", disabled: busy, onClick: () => { adding = null; S.renderCards(); } }, L("取消", "Cancel"));
    return row(input, el("span"), submit, cancel);
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

  // The accounts block of the Claude card; the card draws it again whenever anything changes.
  function renderAccounts() {
    const accounts = S.accounts;
    if (!states.has("default")) void readState("default");
    for (const account of accounts) void readState(account.id);
    return el("div", { className: "account-box" },
      S.accountsError ? el("p", { className: "member-error" }, S.accountsError) : null,
      headRow(), defaultRow(),
      ...accounts.flatMap((account) => [accountRow(account), removing?.id === account.id ? confirmRemoval(account) : null]),
      adding !== null ? addRow() : null);
  }

  Object.assign(S, { renderAccounts });
}
