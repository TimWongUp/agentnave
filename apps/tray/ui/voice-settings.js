"use strict";
(function () {
  const { h, art } = AN;
  const root = document.getElementById("voice-settings");
  let config = null, voices = [], models = [], summaryModels = [], busy = false, loaded = false, audio = null, audioUrl = null, controller = null;
  const drafts = { base: null, model: null, prompts: new Map() };
  const status = (message) => { document.getElementById("notice").textContent = message; };
  function summaryModelLabel(model) {
    const supplier = model.owned_by && !model.id.startsWith(`${model.owned_by}/`) ? model.owned_by : "";
    return supplier ? `${supplier}/${model.id}` : model.id;
  }
  async function request(path, payload) {
    const response = await fetch(path, payload === undefined ? { cache: "no-store" } : {
      method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body: JSON.stringify(payload),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || "无法保存语音设置");
    return data;
  }
  function stop() {
    controller?.abort(); controller = null;
    if (audio) { audio.pause(); audio.removeAttribute("src"); audio.load(); audio = null; }
    if (audioUrl) URL.revokeObjectURL(audioUrl);
    audioUrl = null;
  }
  function segments(label, field, choices) {
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [value, name] of choices) {
      const button = h(`button.segment-btn${config[field] === value ? ".active" : ""}`, {
        type: "button", role: "radio", "aria-checked": String(config[field] === value), disabled: busy,
      }, name);
      button.addEventListener("click", () => save({ [field]: value, ...(field === "engine" ? { voices: {} } : {}) }));
      control.append(button);
    }
    return control;
  }
  const row = (label, control) => h("div.setting-row", {}, h("h2", {}, label), control);
  // A long editor takes the row's whole width under its label.
  const wide = (label, control) => h("div.setting-row.voice-wide", {}, h("h2", {}, label), control);
  // A catalog that arrives after the engine changed belongs to the old engine and is dropped.
  async function loadVoices() {
    const engine = config.engine, usable = config.enabled && config.keys[engine];
    voices = [];
    models = [];
    const fixed = engine === "minimax" ? await request("/app/voice/models/minimax") : [];
    const listed = usable ? await request(`/app/voice/voices/${engine}`) : [];
    const loadedModels = usable && engine !== "minimax" ? await request(`/app/voice/models/${engine}`) : fixed;
    if (config.engine !== engine) return;
    voices = listed;
    models = loadedModels;
  }
  async function loadSummaryModels() {
    summaryModels = [];
    if (!config.enabled || !config.summary.enabled || !summaryUsable()) return;
    summaryModels = await request("/app/voice/summary/models", {});
  }
  async function loadCatalogs(label) {
    const failures = [];
    const reads = [loadVoices().catch(failure => failures.push(`TTS 目录不可用：${failure.message}`))];
    if (config.summary.enabled) {
      reads.push(loadSummaryModels().catch(failure => failures.push(`摘要目录不可用：${failure.message}`)));
    }
    await Promise.all(reads);
    return failures.length ? `${label}；${failures.join("；")}` : label;
  }
  async function save(patch) {
    if (busy) return;
    busy = true; stop(); render();
    try {
      config = await request("/app/voice", { ...config, ...patch });
      status(await loadCatalogs("语音设置已保存"));
      return true;
    } catch (failure) { status(failure.message); return false; }
    finally { busy = false; render(); }
  }
  function keyRow(engine = config.engine, label = engine === "minimax" ? "MiniMax" : "ElevenLabs") {
    const input = h("input", { type: "password", autocomplete: "off", spellcheck: "false", maxlength: "4096", "aria-label": `${engine} API 密钥`, disabled: busy,
      placeholder: config.keys[engine] ? "已保存，粘贴新密钥可替换" : "粘贴密钥" });
    const put = h("button", { type: "button", disabled: busy }, "保存");
    const remove = h("button", { type: "button", disabled: busy || !config.keys[engine] }, "删除");
    async function store(key) {
      if (busy) return;
      busy = true; input.value = ""; stop(); render();
      try {
        config = await request("/app/voice/key", { engine, key });
        const saved = key ? "密钥已保存" : "密钥已删除";
        let detail = "";
        try {
          if (engine.startsWith("summary")) {
            if (config.summary.enabled) await loadSummaryModels();
          } else {
            await loadVoices();
          }
        } catch (failure) {
          detail = `；${engine.startsWith("summary") ? "摘要" : "TTS"}目录不可用：${failure.message}`;
        }
        status(saved + detail);
      }
      catch (failure) { status(failure.message); }
      finally { busy = false; render(); }
    }
    put.addEventListener("click", () => { if (input.value.trim()) void store(input.value); });
    remove.addEventListener("click", () => store(""));
    return row(h("label", {}, `${label} API 密钥`), h("div.voice-key", {}, input, put, remove));
  }
  // Details keeps the list anchored below its row, independent of the OS select menu placement.
  function picker(label, items, selected, pick, icon = null) {
    const name = items.find(v => v.id === selected)?.name || selected || "—";
    const box = h("details.voice-select");
    const summary = h("summary", { "aria-label": label, "aria-disabled": String(busy || !items.length), title: name }, icon?.(selected), name);
    const list = h("div.voice-options", { role: "listbox", "aria-label": label });
    for (const item of items) {
      const option = h("button", { type: "button", role: "option", "aria-selected": String(item.id === selected), title: item.name }, icon?.(item.id), item.name);
      option.addEventListener("click", () => { box.open = false; pick(item.id); });
      list.append(option);
    }
    summary.addEventListener("click", event => { if (busy || !items.length) event.preventDefault(); });
    box.addEventListener("toggle", () => {
      if (!box.open) return;
      root.querySelectorAll("details.voice-select[open]").forEach(other => { if (other !== box) other.open = false; });
    });
    box.addEventListener("keydown", event => {
      if (event.key === "Escape") { box.open = false; summary.focus(); }
      if (!["ArrowDown", "ArrowUp"].includes(event.key) || busy || !items.length) return;
      event.preventDefault(); box.open = true;
      const options = Array.from(list.children);
      const at = options.indexOf(document.activeElement);
      options[(at + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length].focus();
    });
    box.append(summary, list);
    return box;
  }
  async function preview(family, button) {
    stop();
    const pending = controller = new AbortController();
    button.disabled = true;
    try {
      const response = await fetch("/app/voice/speak", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, signal: pending.signal,
        body: JSON.stringify({ family, text: "大家好，我先说一下我的看法。", full: true, ...(config.voices[family] ? { voice: config.voices[family] } : {}) }) });
      if (!response.ok) { const data = await response.json(); throw new Error(data.error || "试听失败"); }
      if (response.status === 204) return;
      const blob = await response.blob();
      if (pending.signal.aborted) return;
      audioUrl = URL.createObjectURL(blob); audio = new Audio(audioUrl);
      audio.onended = stop; audio.onerror = () => { status("无法播放试听音频"); stop(); };
      await audio.play();
    } catch (failure) { if (!pending.signal.aborted) { status(failure.message); stop(); } }
    finally { button.disabled = false; }
  }
  const summarySave = patch => save({ summary: { ...config.summary, ...patch } });
  function section(key, label, ...parts) {
    const previous = root.querySelector(`details[data-section="${key}"]`);
    const box = h("details.voice-section", { "data-section": key }, h("summary", {}, label), ...parts);
    box.open = previous ? previous.open : key === "summary-model";
    return box;
  }
  function promptEditor(key, label, value, limit, persist, reset = false) {
    const input = h("textarea", { rows: limit > 500 ? "4" : "2", maxlength: String(limit), disabled: busy,
      "aria-label": label, placeholder: limit > 500 ? "摘要要求" : "例如：你是一位冷静可靠、偶尔幽默的同伴，说话直接，关心对方。" });
    input.value = drafts.prompts.get(key) ?? value;
    const dirty = () => input.value !== value;
    const put = h("button", { type: "button", disabled: busy || !dirty() }, dirty() ? "保存" : "已保存");
    input.addEventListener("input", () => {
      drafts.prompts.set(key, input.value);
      put.disabled = busy || !dirty(); put.textContent = dirty() ? "保存" : "已保存";
    });
    async function store(text) {
      if (await persist(text)) { drafts.prompts.delete(key); render(); }
    }
    put.addEventListener("click", () => store(input.value));
    const actions = h("div.voice-prompt-actions", {}, put);
    if (reset) {
      const restore = h("button", { type: "button", disabled: busy }, "恢复默认");
      restore.addEventListener("click", () => store("")); actions.append(restore);
    }
    return h("div.voice-prompt-editor", {}, input, actions);
  }
  const platformIcon = id => h("img.voice-platform-icon", { alt: "", src: art.url(["magpie", "custom"].includes(id) ? "logo-default.svg" : `platform/${id}.svg`) });
  function summarySource() {
    return picker("摘要平台", config.summary_platforms, config.summary.provider, id => {
      drafts.model = null;
      summarySave({ provider: id, ...(id === config.summary.provider ? {} : { model: "", effort: "" }) });
    }, platformIcon);
  }
  const summaryPlatform = () => !["chatgpt", "magpie", "custom"].includes(config.summary.provider);
  // Asking for a list before the sign-in or key would only report what the row above already shows.
  const summaryUsable = () => config.summary.provider === "chatgpt" ? config.chatgpt.signed_in
    : !summaryPlatform() || config.keys[`summary:${config.summary.provider}`];
  async function chatgptAction(path) {
    if (busy) return;
    busy = true; render();
    try { config = await request(path, {}); }
    catch (failure) { status(failure.message); }
    finally { busy = false; render(); watchSignIn(); }
  }
  // The sign-in finishes in the browser; only this window's waiting needs polling.
  let signInTimer = null;
  function watchSignIn() {
    clearTimeout(signInTimer);
    if (!config?.chatgpt.signing_in) return;
    signInTimer = setTimeout(async () => {
      const next = await request("/app/voice").catch(() => null);
      if (next) config = next;
      if (config.chatgpt.signing_in) return watchSignIn();
      status(await loadCatalogs(config.chatgpt.signed_in ? "ChatGPT 已登录" : config.chatgpt.error || "ChatGPT 未登录"));
      render();
    }, 1500);
  }
  function chatgptAccount() {
    const account = config.chatgpt;
    const action = account.signed_in
      ? h("button", { type: "button", disabled: busy }, "退出登录")
      : h("button", { type: "button", disabled: busy }, account.signing_in ? "重新打开登录页" : "登录 ChatGPT");
    action.addEventListener("click", () => chatgptAction(`/app/voice/chatgpt/${account.signed_in ? "sign-out" : "start"}`));
    const said = account.signed_in ? [account.user, account.plan].filter(Boolean).join(" · ") : account.signing_in ? "等待浏览器完成登录" : account.error || "";
    return row("ChatGPT 账号", h("div.voice-chatgpt", {}, h("span", { title: said }, said), action));
  }
  function summaryConnection() {
    const base = h("input.voice-summary-url", { type: "url", value: drafts.base ?? config.summary.base_url,
      placeholder: "https://api.example.com/v1", disabled: busy, "aria-label": "摘要 API 地址" });
    const dirty = () => base.value.trim() !== config.summary.base_url;
    const put = h("button", { type: "button", disabled: busy || !dirty() }, dirty() ? "保存" : "已保存");
    base.addEventListener("input", () => {
      drafts.base = base.value; put.disabled = busy || !dirty(); put.textContent = dirty() ? "保存" : "已保存";
    });
    put.addEventListener("click", () => { drafts.base = base.value.trim(); summarySave({ base_url: drafts.base }); });
    return row("摘要 API 地址", h("div.voice-summary-input", {}, base, put));
  }
  function summaryEffort() {
    const supported = summaryModels.find(model => model.id === config.summary.model)?.reasoning_levels || [];
    const effort = config.summary.effort || "";
    const choices = [["", "自动（最低）"], ...supported.map(level => [level, level])];
    if (effort && !supported.includes(effort)) choices.push([effort, `${effort}（不可用）`]);
    const select = h("select.voice-summary-effort", { disabled: busy, "aria-label": "摘要努力程度" });
    for (const [value, label] of choices)
      select.append(h("option", { value, selected: value === effort, disabled: Boolean(value) && !supported.includes(value) }, label));
    select.addEventListener("change", () => summarySave({ effort: select.value }));
    return row("努力程度", select);
  }
  function manualSummaryModel() {
    const fallback = summaryModels.some(item => item.id === config.summary.model) ? "" : config.summary.model;
    const input = h("input", { value: drafts.model ?? fallback, placeholder: "或手填模型 ID", disabled: busy, "aria-label": "手填摘要模型 ID" });
    input.addEventListener("input", () => { drafts.model = input.value; });
    const put = h("button", { type: "button", disabled: busy }, "使用");
    put.addEventListener("click", () => {
      if (!input.value.trim()) return;
      drafts.model = input.value.trim();
      summarySave({ model: drafts.model, ...(drafts.model === config.summary.model ? {} : { effort: "" }) });
    });
    return row("摘要模型 ID", h("div.voice-summary-input", {}, input, put));
  }
  function modelSettings() {
    const enabled = h("input.switch-input", { type: "checkbox", checked: config.summary.enabled, disabled: busy, "aria-label": "使用摘要模型" });
    enabled.addEventListener("change", () => summarySave({ enabled: enabled.checked }));
    const parts = [row("使用摘要模型", enabled), row("摘要平台", summarySource())];
    const provider = config.summary.provider;
    if (provider === "custom") parts.push(summaryConnection(), keyRow("summary", "摘要"));
    if (provider === "chatgpt") parts.push(chatgptAccount());
    if (summaryPlatform()) parts.push(keyRow(`summary:${provider}`, config.summary_platforms.find(p => p.id === provider)?.name));
    const refresh = h("button.voice-refresh", { type: "button", disabled: busy || !config.enabled || !config.summary.enabled }, "刷新");
    refresh.addEventListener("click", async () => {
      busy = true; render();
      try { await loadSummaryModels(); status("摘要模型已刷新"); }
      catch (failure) { status(failure.message); }
      finally { busy = false; render(); }
    });
    const choices = summaryModels.map(model => ({ id: model.id, name: summaryModelLabel(model) }));
    const select = picker("摘要模型", choices, config.summary.model, id => {
      drafts.model = null; summarySave({ model: id, ...(id === config.summary.model ? {} : { effort: "" }) });
    });
    parts.push(row("摘要模型", h("div.voice-summary-model", {}, select, refresh)), summaryEffort());
    if (provider !== "magpie") parts.push(manualSummaryModel());
    parts.push(wide("摘要 Prompt", promptEditor("summary", "摘要 Prompt", config.summary.prompt, 4000,
      value => summarySave({ prompt: value }), true)));
    return section("summary-model", "摘要模型设置", ...parts);
  }
  function persona(family, label) {
    return section(`persona-${family}`, "人设",
      promptEditor(`persona-${family}`, `${label}人设`, config.personas[family] || "", 500,
        value => save({ personas: { ...config.personas, [family]: value } }), true));
  }
  function familySettings() {
    const table = h("div.voice-families", {}, h("div.voice-character", {},
      h("div.voice-family", {}, h("span"), h("span", {}, "boss")), persona("boss", "boss ")));
    Object.entries(Avatars.families).forEach(([family, [label]], index) => {
      const button = h("button.voice-preview", { type: "button", disabled: busy || !config.enabled || !voices.length, "aria-label": `试听${label}音色` }, "试听");
      button.addEventListener("click", () => preview(family, button));
      const select = picker(`${label}音色`, voices, config.voices[family] || voices[index % voices.length]?.id,
        id => save({ voices: { ...config.voices, [family]: id } }));
      table.append(h("div.voice-character", {}, h("div.voice-family", {},
        art.head(art.character("", family), "idle"), h("span", {}, label), select, button), persona(family, label)));
    });
    return table;
  }
  function render() {
    if (!config) return;
    const enabled = h("input.switch-input", { type: "checkbox", checked: config.enabled, disabled: busy, "aria-label": "语音播报" });
    enabled.addEventListener("change", () => save({ enabled: enabled.checked }));
    const automatic = h("input.switch-input", { type: "checkbox", checked: config.auto_read, disabled: busy, "aria-label": "新消息自动读" });
    automatic.addEventListener("change", () => save({ auto_read: automatic.checked }));
    const chained = h("input.switch-input", { type: "checkbox", checked: config.continue_read, disabled: busy, "aria-label": "点读后连播" });
    chained.addEventListener("change", () => save({ continue_read: chained.checked }));
    const parts = [row("语音播报", enabled), row("新消息自动读", automatic), row("点读后连播", chained),
      row("引擎", segments("语音引擎", "engine", [["elevenlabs", "ElevenLabs"], ["minimax", "MiniMax"]])), keyRow()];
    if (config.engine === "minimax") parts.push(row("地区", segments("MiniMax 地区", "minimax_region", [["cn", "国内"], ["global", "海外"]])));
    parts.push(row("语音模型", picker("语音模型", models, config.model,
      id => save({ models: { ...config.models, [config.engine]: id } }))));
    parts.push(modelSettings());
    parts.push(h("p.voice-cloud", {}, config.summary.enabled
      ? "回答内容会发送给所选摘要平台；摘要文本会发送给所选语音引擎。" : "回答内容会发送给所选语音引擎。"));
    root.replaceChildren(...parts, h("h2.voice-group", {}, "角色"), familySettings());
  }
  // The settings work as soon as they are read; catalogs fill their pickers when they arrive.
  async function load() {
    if (busy) return;
    busy = true;
    try {
      config = await request("/app/voice");
      loaded = true;
      watchSignIn();
    }
    catch (failure) { status(failure.message); return; }
    finally { busy = false; render(); }
    const result = await loadCatalogs("语音设置已加载");
    if (result !== "语音设置已加载") status(result);
    // Redrawing would take the caret from a field being typed in; wait until it is left.
    const field = root.contains(document.activeElement) && ["INPUT", "TEXTAREA"].includes(document.activeElement.tagName) ? document.activeElement : null;
    if (field) field.addEventListener("blur", render, { once: true });
    else render();
  }
  addEventListener("hashchange", () => { if (location.hash !== "#voice") stop(); });
  document.addEventListener("visibilitychange", () => { if (document.hidden) stop(); });
  addEventListener("pagehide", stop);
  // Coming back only reloads what another window changed, so the click that refocused still lands.
  addEventListener("focus", async () => {
    if (!loaded || busy || location.hash !== "#voice") return;
    const next = await request("/app/voice").catch(() => null);
    if (next && JSON.stringify(next) !== JSON.stringify(config)) void load();
  });
  window.AgentNaveVoiceSettings = { load };
})();
