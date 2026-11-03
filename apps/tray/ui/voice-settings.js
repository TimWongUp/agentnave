"use strict";
{
  const { h, art, L } = AN;
  const root = document.getElementById("voice-settings");
  let config = null, voices = [], models = [], summaryModels = [], cacheBytes = 0, busy = false, clearingCache = false, loaded = false, audio = null, audioUrl = null, controller = null;
  const drafts = { base: null, model: null, prompts: new Map() };
  const status = (message) => { document.getElementById("notice").textContent = message; };
  function normalizeConfig(raw) {
    if (!raw) return raw;
    return {
      read_scope: "all",
      code: "skip",
      cache_limit_mb: 200,
      ...raw,
      summary: {
        level: "standard",
        language: "source",
        ...(raw.summary || {}),
      },
    };
  }
  function formatBytes(bytes) {
    const n = Number(bytes) || 0;
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
  function summaryModelLabel(model) {
    const supplier = model.owned_by && !model.id.startsWith(`${model.owned_by}/`) ? model.owned_by : "";
    return supplier ? `${supplier}/${model.id}` : model.id;
  }
  async function request(path, payload) {
    const response = await fetch(path, payload === undefined ? { cache: "no-store" } : {
      method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body: JSON.stringify(payload),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || L("无法保存语音设置", "Failed to save voice settings"));
    return data;
  }
  function stop() {
    controller?.abort(); controller = null;
    if (audio) { audio.pause(); audio.removeAttribute("src"); audio.load(); audio = null; }
    if (audioUrl) URL.revokeObjectURL(audioUrl);
    audioUrl = null;
  }
  function choiceButtons(label, current, choices, pick, disabled = false) {
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [value, name] of choices) {
      const active = current === value;
      const button = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button", role: "radio", "aria-checked": String(active), disabled: busy || disabled,
      }, name);
      button.addEventListener("click", () => { if (!active) pick(value); });
      control.append(button);
    }
    return control;
  }
  function segments(label, field, choices, disabled = false) {
    return choiceButtons(label, config[field], choices,
      value => save({ [field]: value, ...(field === "engine" ? { voices: {} } : {}) }), disabled);
  }
  const row = (label, control, extraClass = "") => h(`div.setting-row${extraClass}`, {}, h("h2", {}, label), control);
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
  async function loadCache() {
    const res = await request("/app/voice/cache").catch(() => null);
    if (res && typeof res.bytes === "number") cacheBytes = res.bytes;
  }
  async function clearCache() {
    if (busy || clearingCache) return;
    clearingCache = true; render();
    try {
      await request("/app/voice/cache/clear", {});
      cacheBytes = 0;
      await loadCache();
      status(L("语音缓存已清空", "Voice cache cleared"));
    } catch (failure) {
      status(failure.message);
    } finally {
      clearingCache = false; render();
    }
  }
  async function loadCatalogs(label) {
    const failures = [];
    const reads = [
      loadVoices().catch(failure => failures.push(L(`TTS 目录不可用：${failure.message}`, `TTS catalog unavailable: ${failure.message}`))),
      loadCache(),
    ];
    if (config.summary.enabled) {
      reads.push(loadSummaryModels().catch(failure => failures.push(L(`摘要目录不可用：${failure.message}`, `Summary catalog unavailable: ${failure.message}`))));
    }
    await Promise.all(reads);
    return failures.length ? `${label}${L("；", "; ")}${failures.join(L("；", "; "))}` : label;
  }
  async function save(patch) {
    if (busy) return;
    busy = true; stop(); render();
    try {
      config = normalizeConfig(await request("/app/voice", { ...config, ...patch }));
      status(await loadCatalogs(L("语音设置已保存", "Voice settings saved")));
      return true;
    } catch (failure) { status(failure.message); return false; }
    finally { busy = false; render(); }
  }
  function keyRow(engine = config.engine, label = engine === "minimax" ? "MiniMax" : "ElevenLabs") {
    const input = h("input", { type: "password", autocomplete: "off", spellcheck: "false", maxlength: "4096", "aria-label": L(`${engine} API 密钥`, `${engine} API key`), disabled: busy,
      placeholder: config.keys[engine] ? L("已保存，粘贴新密钥可替换", "Saved; paste a new key to replace") : L("粘贴密钥", "Paste API key") });
    const put = h("button", { type: "button", disabled: busy }, L("保存", "Save"));
    const remove = h("button", { type: "button", disabled: busy || !config.keys[engine] }, L("删除", "Delete"));
    async function store(key) {
      if (busy) return;
      busy = true; input.value = ""; stop(); render();
      try {
        config = normalizeConfig(await request("/app/voice/key", { engine, key }));
        const saved = key ? L("密钥已保存", "API key saved") : L("密钥已删除", "API key deleted");
        let detail = "";
        try {
          if (engine.startsWith("summary")) {
            if (config.summary.enabled) await loadSummaryModels();
          } else {
            await loadVoices();
          }
        } catch (failure) {
          detail = engine.startsWith("summary")
            ? L(`；摘要目录不可用：${failure.message}`, `; Summary catalog unavailable: ${failure.message}`)
            : L(`；TTS目录不可用：${failure.message}`, `; TTS catalog unavailable: ${failure.message}`);
        }
        status(saved + detail);
      }
      catch (failure) { status(failure.message); }
      finally { busy = false; render(); }
    }
    put.addEventListener("click", () => { if (input.value.trim()) void store(input.value); });
    remove.addEventListener("click", () => store(""));
    return row(h("label", {}, L(`${label} API 密钥`, `${label} API key`)), h("div.voice-key", {}, input, put, remove));
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
      if (!response.ok) { const data = await response.json(); throw new Error(data.error || L("试听失败", "Preview failed")); }
      if (response.status === 204) return;
      const blob = await response.blob();
      if (pending.signal.aborted) return;
      audioUrl = URL.createObjectURL(blob); audio = new Audio(audioUrl);
      audio.onended = stop; audio.onerror = () => { status(L("无法播放试听音频", "Cannot play preview audio")); stop(); };
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
      "aria-label": label, placeholder: limit > 500 ? L("摘要要求", "Summary instructions") : L("例如：你是一位冷静可靠、偶尔幽默的同伴，说话直接，关心对方。", "e.g. You are a calm, reliable companion with occasional humor who speaks directly and cares about others.") });
    input.value = drafts.prompts.get(key) ?? value;
    const dirty = () => input.value !== value;
    const put = h("button", { type: "button", disabled: busy || !dirty() }, dirty() ? L("保存", "Save") : L("已保存", "Saved"));
    input.addEventListener("input", () => {
      drafts.prompts.set(key, input.value);
      put.disabled = busy || !dirty(); put.textContent = dirty() ? L("保存", "Save") : L("已保存", "Saved");
    });
    async function store(text) {
      if (await persist(text)) { drafts.prompts.delete(key); render(); }
    }
    put.addEventListener("click", () => store(input.value));
    const actions = h("div.voice-prompt-actions", {}, put);
    if (reset) {
      const restore = h("button", { type: "button", disabled: busy }, L("恢复默认", "Restore default"));
      restore.addEventListener("click", () => store("")); actions.append(restore);
    }
    return h("div.voice-prompt-editor", {}, input, actions);
  }
  const platformIcon = id => h("img.voice-platform-icon", { alt: "", src: art.url(["magpie", "custom"].includes(id) ? "logo-default.svg" : `platform/${id}.svg`) });
  function summarySource() {
    return picker(L("摘要平台", "Summary platform"), config.summary_platforms, config.summary.provider, id => {
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
    try { config = normalizeConfig(await request(path, {})); }
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
      if (next) config = normalizeConfig(next);
      if (config.chatgpt.signing_in) return watchSignIn();
      status(await loadCatalogs(config.chatgpt.signed_in ? L("ChatGPT 已登录", "Signed in to ChatGPT") : config.chatgpt.error || L("ChatGPT 未登录", "Not signed in to ChatGPT")));
      render();
    }, 1500);
  }
  function chatgptAccount() {
    const account = config.chatgpt;
    const action = account.signed_in
      ? h("button", { type: "button", disabled: busy }, L("退出登录", "Sign out"))
      : h("button", { type: "button", disabled: busy }, account.signing_in ? L("重新打开登录页", "Reopen sign-in page") : L("登录 ChatGPT", "Sign in to ChatGPT"));
    action.addEventListener("click", () => chatgptAction(`/app/voice/chatgpt/${account.signed_in ? "sign-out" : "start"}`));
    const said = account.signed_in ? [account.user, account.plan].filter(Boolean).join(" · ") : account.signing_in ? L("等待浏览器完成登录", "Waiting for browser sign-in") : account.error || "";
    return row(L("ChatGPT 账号", "ChatGPT account"), h("div.voice-chatgpt", {}, h("span", { title: said }, said), action));
  }
  function summaryConnection() {
    const base = h("input.voice-summary-url", { type: "url", value: drafts.base ?? config.summary.base_url,
      placeholder: "https://api.example.com/v1", disabled: busy, "aria-label": L("摘要 API 地址", "Summary API URL") });
    const dirty = () => base.value.trim() !== config.summary.base_url;
    const put = h("button", { type: "button", disabled: busy || !dirty() }, dirty() ? L("保存", "Save") : L("已保存", "Saved"));
    base.addEventListener("input", () => {
      drafts.base = base.value; put.disabled = busy || !dirty(); put.textContent = dirty() ? L("保存", "Save") : L("已保存", "Saved");
    });
    put.addEventListener("click", () => { drafts.base = base.value.trim(); summarySave({ base_url: drafts.base }); });
    return row(L("摘要 API 地址", "Summary API URL"), h("div.voice-summary-input", {}, base, put));
  }
  function summaryEffort() {
    const supported = summaryModels.find(model => model.id === config.summary.model)?.reasoning_levels || [];
    const effort = config.summary.effort || "";
    const choices = [["", L("自动（最低）", "Auto (lowest)")], ...supported.map(level => [level, level])];
    if (effort && !supported.includes(effort)) choices.push([effort, L(`${effort}（不可用）`, `${effort} (unavailable)`)]);
    const select = h("select.voice-summary-effort", { disabled: busy, "aria-label": L("摘要努力程度", "Summary effort") });
    for (const [value, label] of choices)
      select.append(h("option", { value, selected: value === effort, disabled: Boolean(value) && !supported.includes(value) }, label));
    select.addEventListener("change", () => summarySave({ effort: select.value }));
    return row(L("努力程度", "Effort"), select);
  }
  function manualSummaryModel() {
    const fallback = summaryModels.some(item => item.id === config.summary.model) ? "" : config.summary.model;
    const input = h("input", { value: drafts.model ?? fallback, placeholder: L("或手填模型 ID", "Or enter model ID"), disabled: busy, "aria-label": L("手填摘要模型 ID", "Custom summary model ID") });
    input.addEventListener("input", () => { drafts.model = input.value; });
    const put = h("button", { type: "button", disabled: busy }, L("使用", "Use"));
    put.addEventListener("click", () => {
      if (!input.value.trim()) return;
      drafts.model = input.value.trim();
      summarySave({ model: drafts.model, ...(drafts.model === config.summary.model ? {} : { effort: "" }) });
    });
    return row(L("摘要模型 ID", "Summary model ID"), h("div.voice-summary-input", {}, input, put));
  }
  function modelSettings() {
    const enabled = h("input.switch-input", { type: "checkbox", checked: config.summary.enabled, disabled: busy, "aria-label": L("使用摘要模型", "Use summary model") });
    enabled.addEventListener("change", () => summarySave({ enabled: enabled.checked }));
    const levelLabel = L("摘要详细程度", "Summary level");
    const langLabel = L("朗读语言", "Reading language");
    const summaryOff = !config.summary.enabled;
    const parts = [
      row(L("使用摘要模型", "Use summary model"), enabled),
      row(levelLabel, choiceButtons(levelLabel, config.summary.level, [
        ["short", L("简短", "Short")],
        ["standard", L("标准", "Standard")],
        ["detailed", L("详细", "Detailed")],
      ], level => summarySave({ level }))),
      row(langLabel, choiceButtons(langLabel, config.summary.language, [
        ["source", L("跟随原文", "Follow source")],
        ["zh", L("中文", "中文")],
        ["en", L("English", "English")],
      ], language => summarySave({ language }), summaryOff), summaryOff ? ".disabled" : ""),
      row(L("摘要平台", "Summary platform"), summarySource()),
    ];
    const provider = config.summary.provider;
    if (provider === "custom") parts.push(summaryConnection(), keyRow("summary", L("摘要", "Summary")));
    if (provider === "chatgpt") parts.push(chatgptAccount());
    if (summaryPlatform()) parts.push(keyRow(`summary:${provider}`, config.summary_platforms.find(p => p.id === provider)?.name));
    const refresh = h("button.voice-refresh", { type: "button", disabled: busy || !config.enabled || !config.summary.enabled }, L("刷新", "Refresh"));
    refresh.addEventListener("click", async () => {
      busy = true; render();
      try { await loadSummaryModels(); status(L("摘要模型已刷新", "Summary models refreshed")); }
      catch (failure) { status(failure.message); }
      finally { busy = false; render(); }
    });
    const choices = summaryModels.map(model => ({ id: model.id, name: summaryModelLabel(model) }));
    const select = picker(L("摘要模型", "Summary model"), choices, config.summary.model, id => {
      drafts.model = null; summarySave({ model: id, ...(id === config.summary.model ? {} : { effort: "" }) });
    });
    parts.push(row(L("摘要模型", "Summary model"), h("div.voice-summary-model", {}, select, refresh)), summaryEffort());
    if (provider !== "magpie") parts.push(manualSummaryModel());
    return section("summary-model", L("摘要模型设置", "Summary model settings"), ...parts);
  }
  function cacheRow() {
    const label = L("语音缓存", "Voice cache");
    const clearBtn = h("button", {
      type: "button",
      disabled: busy || clearingCache || !cacheBytes,
    }, L("立即清空", "Clear now"));
    clearBtn.addEventListener("click", () => void clearCache());
    return row(label, h("div.voice-cache", {},
      segments(label, "cache_limit_mb", [
        [100, "100 MB"],
        [200, "200 MB"],
        [500, "500 MB"],
        [0, L("不缓存", "No cache")],
      ]),
      h("div.voice-cache-meta", {},
        h("span.voice-cache-size", {}, L(`当前占用 ${formatBytes(cacheBytes)}`, `Used ${formatBytes(cacheBytes)}`)),
        clearBtn,
      ),
    ));
  }
  function advancedSettings() {
    const promptLabel = L("摘要 Prompt", "Summary prompt");
    return section("advanced", L("高级", "Advanced"),
      wide(promptLabel, promptEditor("summary", promptLabel, config.summary.prompt, 4000,
        value => summarySave({ prompt: value }), true)));
  }
  function persona(family, label) {
    return section(`persona-${family}`, L("人设", "Persona"),
      promptEditor(`persona-${family}`, L(`${label}人设`, `${label.trim()} persona`), config.personas[family] || "", 500,
        value => save({ personas: { ...config.personas, [family]: value } }), true));
  }
  function familySettings() {
    const table = h("div.voice-families", {}, h("div.voice-character", {},
      h("div.voice-family", {}, h("span"), h("span", {}, "boss")), persona("boss", "boss ")));
    Object.entries(Avatars.families).forEach(([family, [label]], index) => {
      const button = h("button.voice-preview", { type: "button", disabled: busy || !config.enabled || !voices.length, "aria-label": L(`试听${label}音色`, `Preview ${label} voice`) }, L("试听", "Preview"));
      button.addEventListener("click", () => preview(family, button));
      const select = picker(L(`${label}音色`, `${label} voice`), voices, config.voices[family] || voices[index % voices.length]?.id,
        id => save({ voices: { ...config.voices, [family]: id } }));
      table.append(h("div.voice-character", {}, h("div.voice-family", {},
        art.head(art.character("", family), "idle"), h("span", {}, label), select, button), persona(family, label)));
    });
    return table;
  }
  function render() {
    if (!config) return;
    const enabled = h("input.switch-input", { type: "checkbox", checked: config.enabled, disabled: busy, "aria-label": L("语音播报", "Voice playback") });
    enabled.addEventListener("change", () => save({ enabled: enabled.checked }));
    const automatic = h("input.switch-input", { type: "checkbox", checked: config.auto_read, disabled: busy, "aria-label": L("新消息自动读", "Auto-read new messages") });
    automatic.addEventListener("change", () => save({ auto_read: automatic.checked }));
    const chained = h("input.switch-input", { type: "checkbox", checked: config.continue_read, disabled: busy, "aria-label": L("点读后连播", "Continue reading after click") });
    chained.addEventListener("change", () => save({ continue_read: chained.checked }));
    const scopeLabel = L("朗读哪些消息", "Messages to read");
    const codeLabel = L("代码和路径", "Code and paths");
    const parts = [
      row(L("语音播报", "Voice playback"), enabled),
      row(L("新消息自动读", "Auto-read new messages"), automatic),
      row(L("点读后连播", "Continue reading after click"), chained),
      row(scopeLabel, segments(scopeLabel, "read_scope", [
        ["all", L("全部", "All")],
        ["members", L("只读员工的回答", "Employee replies only")],
        ["boss", L("只读 boss 的发言", "Boss messages only")],
        ["meetings", L("只读会议", "Meetings only")],
      ])),
      row(codeLabel, segments(codeLabel, "code", [
        ["skip", L("跳过不读", "Skip")],
        ["mention", L("说“这里有一段代码”", "Say “Here is a code block”")],
      ])),
      row(L("引擎", "Engine"), segments(L("语音引擎", "Voice engine"), "engine", [["elevenlabs", "ElevenLabs"], ["minimax", "MiniMax"]])),
      keyRow(),
    ];
    if (config.engine === "minimax") parts.push(row(L("地区", "Region"), segments(L("MiniMax 地区", "MiniMax region"), "minimax_region", [["cn", L("国内", "China")], ["global", L("海外", "Global")]])));
    parts.push(row(L("语音模型", "Voice model"), picker(L("语音模型", "Voice model"), models, config.model,
      id => save({ models: { ...config.models, [config.engine]: id } }))));
    parts.push(cacheRow());
    parts.push(modelSettings());
    parts.push(h("p.voice-cloud", {}, config.summary.enabled
      ? L("回答内容会发送给所选摘要平台；摘要文本会发送给所选语音引擎。", "Replies are sent to the selected summary platform; summary text is sent to the selected voice engine.")
      : L("回答内容会发送给所选语音引擎。", "Replies are sent to the selected voice engine.")));
    root.replaceChildren(...parts, h("h2.voice-group", {}, L("角色", "Characters")), familySettings(), advancedSettings());
  }
  // The settings work as soon as they are read; catalogs fill their pickers when they arrive.
  async function load() {
    if (busy) return;
    busy = true;
    try {
      config = normalizeConfig(await request("/app/voice"));
      loaded = true;
      watchSignIn();
    }
    catch (failure) { status(failure.message); return; }
    finally { busy = false; render(); }
    const loadedMsg = L("语音设置已加载", "Voice settings loaded");
    const result = await loadCatalogs(loadedMsg);
    if (result !== loadedMsg) status(result);
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
    const next = normalizeConfig(await request("/app/voice").catch(() => null));
    if (next && JSON.stringify(next) !== JSON.stringify(config)) void load();
  });
  window.AgentNaveVoiceSettings = { load };
}
