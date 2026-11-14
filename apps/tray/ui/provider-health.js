"use strict";
{
  const PROVIDER_NAMES = {
    codex: "Codex CLI",
    claude: "Claude Code",
    codebuddy: "CodeBuddy Code",
    grok: "Grok CLI",
    antigravity: "Antigravity CLI",
    pi: "Pi Coding Agent",
    deepseek: "DeepSeek Harness",
    devin: "Devin CLI",
    zcode: "ZCode",
  };
  function rows(providers) {
    const result = [];
    for (const [provider, group] of Object.entries(providers || {})) {
      if (group?.code) result.push({ provider, item: group });
      if (group?.default) result.push({ provider, item: group.default });
      for (const [model, item] of Object.entries(group?.models || {})) result.push({ provider, model, item });
      for (const [quotaGroup, item] of Object.entries(group?.groups || {})) result.push({ provider, quotaGroup, item });
    }
    return result.filter(({ item }) => {
      if (item?.code === "auth" && item.until == null) return true;
      const end = Date.parse(item?.until), now = Date.now();
      return ["auth", "billing", "quota"].includes(item?.code) && end > now && end <= now + 8 * 86400000;
    });
  }
  function reason(item, L) {
    return { quota: L("额度用完", "Quota exhausted"), billing: L("余额不足或欠费", "Insufficient balance or unpaid bill"), auth: L("未登录或登录失效", "Not signed in or session expired") }[item.code];
  }
  function recovery(item, L) {
    if (item.code === "auth") return L("重新登录后点“立即恢复”", "Sign in again, then select “Resume now”");
    const date = new Date(item.until), now = new Date();
    const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
    const day = date.toDateString() === now.toDateString() ? "" : `${date.toLocaleDateString()} `;
    return L(`约 ${day}${time} 恢复`, `Resumes around ${day}${time}`);
  }
  const DISMISSED = "agentnave.providerHealthDismissed";
  function dismissed() {
    try { return localStorage.getItem(DISMISSED); } catch { return null; }
  }
  function bindClose(box) {
    const close = box.querySelector(".provider-health-close");
    if (!close || close.dataset.bound) return;
    close.dataset.bound = "1";
    close.addEventListener("click", () => {
      try { localStorage.setItem(DISMISSED, box.dataset.key || ""); } catch { /* Closed for this view only. */ }
      box.hidden = true;
    });
  }
  async function refresh(box) {
    if (!box) return;
    try {
      const response = await fetch("/app/provider-health", { cache: "no-store" });
      if (!response.ok) return;
      const data = await response.json(), L = AN.L || ((zh) => zh), grouped = new Map();
      for (const row of rows(data.providers)) {
        const key = JSON.stringify([row.provider, row.item.code, row.item.until]);
        if (!grouped.has(key)) grouped.set(key, []);
        grouped.get(key).push(row);
      }
      const lines = [...grouped.values()].map(group => {
        const first = group[0], models = group.filter(row => row.model).length;
        const scopes = group.filter(row => row.quotaGroup).map(row => row.quotaGroup);
        const affected = [models ? L(`${models} 个模型暂停派活`, `${models} model${models === 1 ? "" : "s"} paused`) : "", ...scopes.map(name => L(`${name} 暂停派活`, `${name} paused`))];
        if (group.some(row => !row.model && !row.quotaGroup)) affected.unshift(L("暂停派活", "Dispatch paused"));
        const line = document.createElement("span");
        const comma = L("，", ", ");
        line.textContent = `⚠ ${PROVIDER_NAMES[first.provider] || first.provider} ${reason(first.item, L)}${comma}${affected.filter(Boolean).join(L("、", ", "))}${comma}${recovery(first.item, L)}`;
        return line;
      });
      const list = box.querySelector(".provider-health-lines") || box;
      list.replaceChildren(...lines);
      bindClose(box);
      // A closed warning stays closed until what it says changes.
      box.dataset.key = lines.map(line => line.textContent).join("\n");
      box.hidden = !lines.length || box.dataset.key === dismissed();
    } catch { /* Keep the previous warning when a refresh fails. */ }
  }
  AN.providerHealth = { names: PROVIDER_NAMES, rows, reason, recovery, refresh };
}
