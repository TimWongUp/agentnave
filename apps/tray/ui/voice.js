"use strict";
// The office alone owns playback. A generation invalidates fetches when stopped or hidden.
(function () {
  const { h } = AN;
  let config = null, group = null, view = null, observed = new Map(), skip = true;
  let queue = [], current = null, generation = 0, failures = 0, halted = false;
  let running = false, loading = null, baselineEpoch = 0;
  let roomView = null, playlistScope = null, playlistFetch = null;
  let privateView = null, channelVisible = false, paused = false, wakePlayback = null;
  const switchButton = label => h("button.voice-toggle", { type: "button", hidden: true, "aria-label": label, "aria-pressed": "false" }, h("span.voice-track"), label);
  // Auto read speaks new messages as they arrive; chaining follows a clicked message down its conversation.
  const toggle = switchButton("新消息自动读"), chain = switchButton("点读后连播");
  const actions = document.getElementById("voice-actions");
  const roster = h("div.voice-roster");
  const channelStop = h("button.voice-stop", { type: "button", disabled: true, hidden: true }, "暂停");
  const statusName = h("span.voice-status-name");
  const status = h("div.voice-status", { "aria-live": "polite" }, h("span.voice-waves", {}, h("i"), h("i"), h("i")), statusName);
  const more = h("div.voice-more");
  actions.append(roster, status, toggle, chain, h("div.voice-channel-controls", {}, channelStop));
  let channelContext = null;
  const participants = new Map();
  const ROSTER_LIMIT = 6;
  const ready = () => Boolean(config?.enabled && config.keys?.[config.engine]);
  const changed = () => { updateChannel(); window.dispatchEvent(new Event("agentnave:voice")); };
  function error(message) {
    const banner = document.getElementById("banner");
    banner.textContent = message;
    banner.hidden = false;
    document.getElementById("announce").textContent = message;
  }
  async function state() {
    const response = await fetch("/app/voice", { cache: "no-store" });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "无法读取语音播报设置");
    return data;
  }
  async function reload() {
    if (loading) return loading;
    loading = (async () => {
      const next = await state();
      const previous = config;
      const different = !previous || JSON.stringify(previous) !== JSON.stringify(next);
      const usable = c => Boolean(c.enabled && c.keys?.[c.engine]);
      // Only a change that silences or redirects playback interrupts it; other settings
      // apply to the next clip.
      const interrupt = previous && different && (usable(previous) !== usable(next)
        || previous.engine !== next.engine || previous.auto_read !== false && next.auto_read === false);
      if (previous && different) {
        if (previous.enabled !== next.enabled || previous.auto_read !== next.auto_read || usable(previous) !== usable(next)) { skip = true; baselineEpoch++; }
        if (!previous.enabled && next.enabled) { halted = false; failures = 0; }
      }
      config = next;
      switches();
      if (interrupt) stop();
      else if (different) changed();
    })().finally(() => { loading = null; });
    return loading;
  }
  function eligible(m) {
    return Boolean(m.text?.trim()) && ["member", "manager"].includes(m.speaker?.role)
      && ["prompt", "instruction", "forward", "reply"].includes(m.kind)
      && !["running", "pending", "not_started", "review"].includes(m.status);
  }
  function privateMessage(m, desk) {
    return m.thread?.kind !== "room" && (m.desk === desk || m.thread?.key === desk
      || m.to?.some(t => t.key === desk));
  }
  function bossIdentity(m, c) {
    const speaker = m?.speaker || {};
    const provider = speaker.provider || c.host || "";
    const known = [...(c.messages || [])].reverse().filter(p => p.speaker?.role === "manager"
      && p.speaker.model && (!p.speaker.provider || p.speaker.provider === provider));
    const source = known.find(p => m?.round != null && p.conversation === m.conversation && p.round === m.round)
      || known.find(p => m && Date.parse(p.time) <= Date.parse(m.time)) || known[0];
    return { provider, model: speaker.model || source?.speaker.model || "" };
  }
  function answerKey(m) {
    return m.conversation && m.round != null && m.desk
      ? JSON.stringify([m.conversation, m.round, m.desk]) : null;
  }
  // Keep each private briefing beside its own answer, without crossing seats or rounds.
  // Match the message list's briefing-before-answer order.
  function beforeAnswers(list) {
    const answers = new Set(list.filter(m => m.kind === "reply").map(answerKey).filter(Boolean));
    const instructions = new Map();
    for (const m of list) if (m.kind === "instruction" && answers.has(answerKey(m))) {
      const k = answerKey(m);
      if (!instructions.has(k)) instructions.set(k, []);
      instructions.get(k).push(m);
    }
    const paired = new Set([...instructions.values()].flat());
    return list.flatMap(m => {
      if (paired.has(m)) return [];
      const prior = m.kind === "reply" && instructions.get(answerKey(m)) || [];
      if (prior.length) instructions.delete(answerKey(m));
      return [...prior, m];
    });
  }
  // How a speaker is called out loud; GPT's family is labelled by its company elsewhere.
  function spokenName(provider, model) {
    const family = Avatars.family(provider, model);
    return family === "gpt" ? "GPT" : Avatars.families?.[family]?.[0] || AN.cliName?.(provider) || "";
  }
  // How the summary model may call a desk: its spoken name and duty, and whether another seat of
  // this meeting shares that name.
  function person(d, ctx, m) {
    const family = Avatars.family(d.provider, d.model);
    const seats = m?.thread?.kind === "room" ? ctx.rooms?.get(m.thread.id)?.seats || [] : [];
    const twin = seats.some(s => s.desk !== d.key && (t => t && Avatars.family(t.provider, t.model) === family)(ctx.desks.get(s.desk)));
    return { name: spokenName(d.provider, d.model), role: d.role || "", twin };
  }
  // Whom a boss message is said to: its one employee in private, or the seats it names in a
  // meeting; nobody named means the whole meeting.
  function listeners(m, ctx, boss) {
    if (!boss || m.kind === "forward") return [];
    const keys = m.to?.map(t => t.key).filter(Boolean) || [];
    const meeting = m.thread?.kind === "room" && m.kind !== "instruction";
    const seats = meeting ? ctx.rooms?.get(m.thread.id)?.seats || [] : [];
    const named = meeting ? (keys.length && keys.length < seats.length ? keys : []) : m.desk ? [m.desk] : keys.slice(0, 1);
    return named.map(key => ctx.desks.get(key)).filter(Boolean).map(d => person(d, ctx, m));
  }
  // Whose words a boss forward relays, when the forwarded message is still in view.
  function source(m, ctx) {
    const id = m.forward?.message_id;
    const original = id && ctx.c.messages?.find(o => o.message_id === id);
    if (!original) return null;
    if (original.speaker?.role === "manager") return { name: "boss", role: "" };
    const d = original.desk && ctx.desks.get(original.desk);
    return d ? person(d, ctx, original) : { name: spokenName(original.speaker?.provider, original.speaker?.model), role: "" };
  }
  function item(m, ctx, automatic = false) {
    const d = m.desk && ctx.desks.get(m.desk);
    const boss = m.speaker?.role === "manager";
    const { provider, model } = boss ? bossIdentity(m, ctx.c)
      : { provider: m.speaker?.provider || d?.provider || ctx.c.host, model: m.speaker?.model || d?.model || "" };
    return { id: m.id, room: m.thread?.kind === "room" ? m.thread.id : null, desk: m.desk,
      kind: m.kind, conversation: m.conversation, round: m.round,
      boss, family: Avatars.family(provider, model) || null,
      provider, model, role: boss ? "" : d?.role || m.speaker?.desk_role || "", label: boss ? "boss" : d ? AN.deskName(d) : m.speaker?.label || provider,
      name: boss ? "boss" : spokenName(provider, model), to: listeners(m, ctx, boss),
      private: m.thread?.kind !== "room" || m.kind === "instruction" || m.kept_private === true,
      forward: boss && m.kind === "forward" ? source(m, ctx) : null,
      text: m.text, time: Date.parse(m.time) || 0, automatic, controller: new AbortController() };
  }
  async function audio(job) {
    const response = await fetch("/app/voice/speak", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
      signal: job.controller.signal, body: JSON.stringify({ family: job.family, text: job.text, full: false, boss: job.boss,
        scene: { name: job.name, role: job.role, room: Boolean(job.room), private: job.private, to: job.to, forward: job.forward } }) });
    if (response.status === 204) return null;
    if (!response.ok) {
      const data = await response.json().catch(() => ({}));
      throw new Error(data.error || "语音播报合成失败");
    }
    const notice = response.headers.get("X-AgentNave-Notice");
    if (notice) error(decodeURIComponent(notice));
    return response.blob();
  }
  function prepare(job) {
    // Attach both handlers immediately: a later clip may fail before playback reaches it.
    job.prepared ??= audio(job).then(blob => ({ blob }), failure => ({ failure }));
    return job.prepared;
  }
  function finish(job) {
    job.prepared = null;
    job.speaking = false;
    if (job.url) URL.revokeObjectURL(job.url);
    job.url = null;
    job.audio?.pause();
    if (job.audio) { job.audio.removeAttribute("src"); job.audio.load(); }
    job.resolve?.();
    job.resolve = null;
    job.reject = null;
  }
  function stop(notify = true) {
    generation++;
    paused = false; wakePlayback?.(); wakePlayback = null;
    playlistFetch?.abort(); playlistFetch = null; playlistScope = null;
    for (const job of queue) { job.controller.abort(); job.prepared = null; }
    queue = [];
    if (current) { current.controller.abort(); finish(current); current = null; }
    if (notify) changed();
  }
  // Ends a chained read; what auto read queued meanwhile stays queued.
  function dropPlaylist(keepCurrent = false) {
    playlistFetch?.abort(); playlistFetch = null; playlistScope = null;
    queue = queue.filter(job => job.automatic || (job.controller.abort(), false));
    if (!keepCurrent && current && !current.automatic) interrupt();
  }
  // Ends the clip in progress but keeps what is queued after it.
  function interrupt() {
    generation++;
    paused = false; wakePlayback?.(); wakePlayback = null;
    if (current) { current.controller.abort(); finish(current); current = null; }
  }
  function pause() {
    if (!current && !queue.length) return;
    paused = true;
    if (current?.audio) { current.audio.pause(); current.speaking = false; }
    changed();
  }
  async function resume() {
    if (!paused || !ready() || halted) return;
    paused = false;
    wakePlayback?.(); wakePlayback = null;
    if (current?.audio) {
      const job = current, token = generation;
      job.speaking = true;
      changed();
      try { await job.audio.play(); }
      catch (failure) { if (token === generation && !paused) job.reject?.(failure); }
    } else { changed(); void drain(); }
  }
  async function play(job, token) {
    const { blob, failure } = await prepare(job);
    if (token !== generation) return;
    if (paused) await new Promise(resolve => { wakePlayback = resolve; });
    if (token !== generation) return;
    if (failure) throw failure;
    if (!blob) return;
    job.url = URL.createObjectURL(blob);
    job.audio = new Audio(job.url);
    job.speaking = true;
    changed();
    await new Promise((resolve, reject) => {
      job.resolve = resolve;
      job.reject = reject;
      job.audio.onended = resolve;
      job.audio.onerror = () => reject(new Error("无法播放语音播报"));
      job.audio.play().catch(failure => { if (!paused) reject(failure); });
    });
  }
  // While a clip plays, the next two are already being synthesized.
  function prefetch() {
    if (!paused) for (const next of queue.slice(0, 2)) prepare(next);
  }
  async function drain() {
    if (running || paused || halted || !ready() || document.hidden) return;
    running = true;
    try {
      while (queue.length && !paused && !halted && ready() && !document.hidden) {
        const token = generation;
        const job = current = queue.shift();
        // Once only auto read is left, the chained read is over.
        if (playlistScope && !playlistFetch && job.automatic && !queue.some(next => !next.automatic)) playlistScope = null;
        prepare(job);
        prefetch();
        changed();
        try {
          await play(job, token);
          if (token === generation) failures = 0;
        } catch (failure) {
          if (token === generation) {
            failures++;
            error(failures >= 3 ? `语音播报已停止：${failure.message}。请重新开启语音。` : failure.message);
            if (failures >= 3) { halted = true; stop(); }
          }
        } finally {
          finish(job);
          if (current === job) current = null;
          changed();
        }
      }
    } finally {
      running = false;
      if (paused && !current && !queue.length) { paused = false; changed(); }
      if (!queue.length && !playlistFetch && playlistScope) { playlistScope = null; changed(); }
    }
  }
  // Auto read follows the whole workspace, whichever conversation is open: each message is
  // read once, when it first becomes readable after the baseline.
  function ingest(chat, ctx, focus = null, epoch = baselineEpoch) {
    const baseline = skip || document.hidden || group !== chat.group || view !== focus || epoch !== baselineEpoch;
    if (group !== chat.group) {
      stop(false); observed = new Map(); group = chat.group;
    }
    view = focus;
    const answers = new Map(chat.messages.filter(m => m.kind === "reply" && eligible(m)).map(m =>
      [answerKey(m), { message: m, seen: observed.get(m.id) === true }]));
    for (const m of chat.messages) {
      const briefing = m.kind === "instruction" && answerKey(m);
      const answer = briefing ? answers.get(briefing) : null;
      const can = eligible(m) && (!briefing || Boolean(answer));
      const seen = observed.get(m.id) === true;
      if (!baseline && can && !seen && !answer?.seen && ready() && !halted && config.auto_read !== false)
        queue.push(item(m, ctx, true));
      observed.set(m.id, seen || can);
    }
    if (!document.hidden && epoch === baselineEpoch) skip = false;
    // A message picked by hand stays first.
    queue.sort((a, b) => Boolean(b.chosen) - Boolean(a.chosen) || a.time - b.time);
    queue = beforeAnswers(queue);
    if (current) prefetch();
    // Playback only updates its channel and message controls.
    queueMicrotask(drain);
    void reload().catch(failure => error(failure.message));
  }
  // The messages of one conversation a chained read follows, in the order they are shown.
  function playable(chat, target) {
    const ctx = { c: chat, desks: new Map([...(chat.desks || []), ...(chat.optimized?.desks || [])].map(d => [d.key, d])),
      rooms: new Map((chat.rooms || []).map(r => [r.id, r])) };
    // A briefing waits for its answer, as in auto read.
    const answered = new Set(chat.messages.filter(m => m.kind === "reply" && eligible(m)).map(answerKey).filter(Boolean));
    const list = chat.messages.filter(m => eligible(m) && (m.kind !== "instruction" || !answerKey(m) || answered.has(answerKey(m)))
      && (target.private ? privateMessage(m, target.id) : m.conversation === target.id)).map(m => item(m, ctx));
    list.sort((a, b) => a.time - b.time);
    return beforeAnswers(list);
  }
  // Reads the clicked message at once, then, with chaining on, the rest of its conversation after it.
  async function readFrom(m, chosen) {
    if (playlistScope) dropPlaylist();
    const queued = queue.find(job => job.id === chosen.id);
    queue = queue.filter(job => job !== queued);
    queue.unshift(Object.assign(queued || chosen, { chosen: true }));
    observed.set(chosen.id, true);
    interrupt();
    changed();
    void drain();
    const target = config?.continue_read === false ? null : roomView ? roomView
      : privateView ? { group: channelContext?.c.group || group, id: privateView, private: true }
        : m.conversation ? { group, id: m.conversation } : null;
    if (!target) return;
    const load = playlistFetch = new AbortController();
    playlistScope = target.id;
    try {
      const path = target.private ? "history" : `focus/${encodeURIComponent(target.id)}`;
      const response = await fetch(`/app/chats/${encodeURIComponent(target.group)}/${path}`,
        { cache: "no-store", signal: load.signal });
      if (!response.ok) throw new Error("无法读取完整对话消息");
      const chat = await response.json();
      if (playlistFetch !== load) return;
      const list = playable(chat, target);
      const start = list.findIndex(job => job.id === chosen.id);
      const known = new Set([current?.id, ...queue.map(job => job.id)]);
      const rest = start < 0 ? [] : list.slice(start + 1).filter(job => !known.has(job.id));
      for (const job of rest) observed.set(job.id, true);
      queue.push(...rest);
      queue.sort((a, b) => Boolean(b.chosen) - Boolean(a.chosen) || a.time - b.time);
      queue = beforeAnswers(queue);
      if (current) prefetch();
    } catch (failure) {
      if (playlistFetch === load) error(failure.message);
    } finally {
      if (playlistFetch === load) {
        playlistFetch = null;
        if (!queue.some(job => !job.automatic) && !current?.chosen) playlistScope = null;
        changed(); void drain();
      }
    }
  }
  function controls(visible, room = null, ctx = null, desk = null) {
    // A chained read belongs to the open conversation; auto read does not.
    if (playlistScope && (channelVisible !== visible || roomView?.id !== room?.id || roomView?.group !== room?.group || privateView !== desk))
      dropPlaylist();
    roomView = room;
    privateView = desk;
    channelVisible = visible;
    channelContext = ctx;
    updateChannel();
  }
  function updateChannel() {
    const ctx = channelContext;
    const c = ctx?.c;
    // The boss of this meeting or chat: whoever last spoke here, not who holds the workspace now.
    const said = [...(c?.messages || [])].reverse().find(m => m.speaker?.role === "manager"
      && (roomView ? m.conversation === roomView.id : privateView ? privateMessage(m, privateView) : true));
    const boss = current?.boss ? current : bossIdentity(said || null, c || {});
    let speaking = null;
    const people = [{ key: "@boss", provider: boss.provider, model: boss.model, label: "boss", host: true }];
    const room = roomView && ctx?.rooms?.get(roomView.id);
    const desks = room ? room.seats.map(s => ctx.desks.get(s.desk)).filter(Boolean)
      : privateView ? [ctx?.desks.get(privateView)].filter(Boolean) : [];
    for (const d of desks) people.push({ key: d.key, provider: d.provider, model: d.model, role: d.role, label: AN.deskName(d) });
    if (current && !current.boss && !people.some(p => p.key === current.desk))
      people.push({ key: current.desk || current.id, provider: current.provider, model: current.model, role: current.role, label: current.label });
    const keep = new Set();
    for (const person of people) {
      keep.add(person.key);
      const character = AN.art.character(person.provider, person.model);
      let rec = participants.get(person.key);
      if (!rec) {
        rec = { el: h("div.voice-person"), head: h("div.voice-person-head") };
        rec.el.append(rec.head); participants.set(person.key, rec);
      }
      if (rec.character !== character) { rec.head.replaceChildren(AN.art.head(character)); rec.character = character; }
      const active = current && (person.host ? current.boss : !current.boss && person.key === (current.desk || current.id));
      const name = person.host ? "boss" : person.role || person.label;
      if (active) speaking = person.key;
      if (active) statusName.textContent = paused ? "" : current.speaking ? name : `${name} 准备中`;
      rec.el.dataset.speaking = String(Boolean(active && current.speaking));
      rec.el.dataset.preparing = String(Boolean(active && !current.speaking));
      rec.el.setAttribute("aria-label", `${person.host ? "主持人" : "发言人"} ${name}${active ? paused ? "，已暂停" : current.speaking ? "，正在播放" : "，正在准备" : ""}`);
      rec.el.title = person.host || !person.role ? name : `${name} · ${person.label}`;
    }
    for (const [key, rec] of participants) if (!keep.has(key)) { rec.el.remove(); participants.delete(key); }
    // A long roster folds, but whoever is speaking always stays in view.
    const shown = people.length <= ROSTER_LIMIT ? people : people.slice(0, ROSTER_LIMIT - 1);
    if (speaking && !shown.some(p => p.key === speaking)) shown[shown.length - 1] = people.find(p => p.key === speaking);
    more.textContent = `+${people.length - shown.length}`;
    more.title = people.filter(p => !shown.includes(p)).map(p => p.host ? "boss" : p.role || p.label).join("、");
    AN.reconcile(roster, [...shown.map(p => participants.get(p.key).el), ...people.length > shown.length ? [more] : []]);
    const idle = !current && !queue.length;
    if (!current) statusName.textContent = "";
    actions.hidden = !channelVisible || !config?.enabled;
    actions.dataset.live = String(Boolean(current?.speaking));
    actions.dataset.active = String(!idle);
    status.hidden = !current;
    toggle.hidden = chain.hidden = actions.hidden;
    channelStop.textContent = paused ? "继续" : "暂停";
    channelStop.hidden = idle;
    channelStop.disabled = idle;
    channelStop.dataset.paused = String(paused);
  }
  channelStop.addEventListener("click", () => paused ? resume() : pause());
  function decorate(rec, m, ctx) {
    if (!rec.read) {
      rec.readLabel = h("span", {}, "读摘要");
      rec.read = h("button.voice-read", { type: "button" }, h("span.voice-waves", {}, h("i"), h("i"), h("i")), rec.readLabel);
      rec.read.addEventListener("click", event => {
        event?.stopPropagation();
        const same = current && current.id === rec.voiceItem?.id;
        if (same) { if (paused) void resume(); else pause(); return; }
        if (!ready() || halted) return;
        // The chosen message goes first; what auto read queued still follows it.
        void readFrom(m, { ...rec.voiceItem, controller: new AbortController() });
      });
      rec.bub.prepend(rec.read); // Keep the action inside the chat bubble, above its text.
    }
    rec.read.hidden = !eligible(m) || !config?.enabled;
    rec.read.disabled = !ready() || halted;
    if (eligible(m)) rec.voiceItem = item(m, ctx);
    const reading = current?.id === m.id;
    const idleLabel = "读摘要";
    rec.readLabel.textContent = reading ? paused ? "继续" : "暂停" : idleLabel;
    rec.read.title = reading ? paused ? "继续朗读" : "暂停朗读" : ready() ? idleLabel : "请在设置 → 语音中开启语音播报并配置音色引擎";
    rec.read.setAttribute("aria-label", rec.read.title);
    rec.read.setAttribute("aria-pressed", String(Boolean(reading)));
  }
  function switches() {
    toggle.setAttribute("aria-pressed", String(config.auto_read !== false));
    chain.setAttribute("aria-pressed", String(config.continue_read !== false));
    toggle.disabled = chain.disabled = !ready();
  }
  async function flip(field) {
    if (!config) return;
    const on = config[field] !== false;
    toggle.disabled = chain.disabled = true;
    if (field === "auto_read") {
      if (on) stop();
      // A poll finishing while this saves must not queue under the old setting.
      skip = true; baselineEpoch++;
    } else if (on) dropPlaylist(true);
    try {
      const response = await fetch("/app/voice", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body: JSON.stringify({ ...config, [field]: !on }) });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || "无法保存语音播报设置");
      config = data; failures = 0; halted = false;
      if (field === "auto_read") { skip = true; baselineEpoch++; }
      changed();
    } catch (failure) { error(failure.message); }
    finally { switches(); }
  }
  toggle.addEventListener("click", () => flip("auto_read"));
  chain.addEventListener("click", () => flip("continue_read"));
  document.addEventListener("visibilitychange", () => { skip = true; baselineEpoch++; stop(); });
  window.addEventListener("pagehide", stop);
  AN.voice = { ingest, decorate, pollToken: () => baselineEpoch, controls, stop, beforeAnswers };
  void reload().catch(failure => error(failure.message));
})();
