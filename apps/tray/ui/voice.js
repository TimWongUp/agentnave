"use strict";
// The office alone owns playback. A generation invalidates fetches when stopped.
{
  const { h, L = (zh) => zh } = AN;
  let config = null, group = null;
  let started = [], queue = [], current = null, generation = 0, failures = 0, halted = false, picks = 0;
  let running = false, loading = null, batch = null, lastAlbum = null, loadedAt = -Infinity;
  const completed = new Map();
  let roomView = null, playlistScope = null, playlistFetch = null;
  let privateView = null, deskShown = null, deskListenShown = null, channelVisible = false, paused = false;
  let recent = null, recentTimer = null, published = null, follow = null, leaving = false, posting = false;
  // Numbers start from the clock, so a reloaded page never counts below what the last one sent.
  let sequence = Date.now();
  // The office owns playback; the menu bar panel sends its clicks here too.
  const menu = AN.voiceMenu(document.querySelector?.("header.bar .bar-end"), command);
  const actions = document.getElementById("voice-actions");
  const roster = h("div.voice-roster");
  const more = h("div.voice-more");
  const sameListen = target => Boolean(target && follow && follow.group === target.group
    && follow.room === (target.room || null) && follow.desk === (target.desk || null));
  // Starts listening to `target()`, or stops when that is what is being listened to.
  function listenButton(zh, en, target) {
    const name = h("span");
    const button = h("button.listen-here", { type: "button", hidden: true, "aria-pressed": "false" }, name);
    button.addEventListener("click", event => {
      event?.stopPropagation();
      if (sameListen(target())) keepAfterStop(() => unlisten()); else void listen(target());
    });
    const draw = () => {
      const on = sameListen(target());
      button.hidden = !config?.enabled || !target();
      button.disabled = !ready();
      button.setAttribute("aria-pressed", String(on));
      name.textContent = on ? L("停止收听", "Stop listening") : L(zh, en);
    };
    return { el: button, draw };
  }
  const workspaceListen = listenButton("收听工作区", "Listen to workspace", () => group && { group });
  const columnListen = listenButton("收听此列消息", "Listen to this column", () => group && (roomView || privateView) && { group, room: roomView?.id, desk: privateView, shown: deskListenShown });
  actions.append(roster, columnListen.el);
  let channelContext = null;
  const participants = new Map();
  const ROSTER_LIMIT = 6;
  const REWIND_SECONDS = 2;
  const FOLLOW_EVERY = 2000;
  const MAX_WAIT = 5 * 60000;
  // The most a cover's two rows show.
  const COVER_FACES = 6;
  // A playlist row shows one line of a message.
  const EXCERPT_CHARS = 120;
  const RECENT_MS = 60e3;
  const ready = () => Boolean(config?.enabled && config.keys?.[config.engine]);
  const changed = () => { updateChannel(); window.dispatchEvent(new Event("agentnave:voice")); };
  let shownError = null;
  function error(message, source, job = null, retry = null) {
    shownError = { source, job, message, retry };
    publish();
    document.getElementById("announce").textContent = message;
  }
  function clearError() { shownError = null; publish(); }
  async function state() {
    const response = await fetch("/app/voice", { cache: "no-store" });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || L("无法读取语音播报设置", "Failed to load voice settings"));
    return data;
  }
  async function reload() {
    if (loading) return loading;
    loading = (async () => {
      const next = await state();
      loadedAt = Date.now();
      if (shownError?.source === "load") clearError();
      const previous = config;
      const different = !previous || JSON.stringify(previous) !== JSON.stringify(next);
      const usable = c => Boolean(c.enabled && c.keys?.[c.engine]);
      // Only a change that silences or redirects playback interrupts it; other settings
      // apply to the next clip.
      const shouldInterrupt = previous && different && (usable(previous) !== usable(next) || previous.engine !== next.engine);
      if (previous && different) {
        clearError();
        halted = false; failures = 0;
      }
      config = next;
      if (previous?.continue_read === true && next.continue_read !== true) dropPlaylist(true);
      if (previous && previous.read_scope !== next.read_scope) {
        const allowed = new Set(scopedRead([...(current ? [current] : []), ...queue]));
        queue = queue.filter(job => allowed.has(job) || (discard(job), false));
        if (current && !allowed.has(current)) interrupt();
      }
      publish();
      // Silenced voice leaves nothing to listen with.
      if (shouldInterrupt && !usable(next)) unlisten(false);
      if (shouldInterrupt) stop();
      else if (different) changed();
    })().finally(() => { loading = null; });
    return loading;
  }
  function eligible(m) {
    return Boolean(m.text?.trim()) && ["member", "manager"].includes(m.speaker?.role)
      && ["prompt", "instruction", "forward", "reply"].includes(m.kind)
      && !["running", "pending", "not_started", "review"].includes(m.status);
  }
  function inReadScope(boss, room) {
    switch (config?.read_scope) {
      case "boss": return boss;
      case "members": return !boss;
      case "meetings": return room;
      default: return true;
    }
  }
  const scopedMessage = m => inReadScope(m.speaker?.role === "manager", m.thread?.kind === "room");
  function scopedRead(list) {
    const inScope = job => inReadScope(job.boss, Boolean(job.room));
    const answered = new Set(list.filter(job => job.kind === "reply" && inScope(job)).map(answerKey).filter(Boolean));
    return list.filter(job => job.chosen || inScope(job)
      && (job.kind !== "instruction" || !answerKey(job) || answered.has(answerKey(job))));
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
  const albumKey = job => JSON.stringify([job.group, job.album.link]);
  const done = job => completed.get(albumKey(job))?.has(job.id);
  function remember(job) {
    const key = albumKey(job);
    if (!completed.has(key)) completed.set(key, new Set());
    completed.get(key).add(job.id);
  }
  // A briefing and its answer occupy one turn in an album's batch.
  function units(jobs) {
    const list = beforeAnswers(jobs.slice().sort((a, b) => a.time - b.time)), result = [];
    for (const job of list) {
      const prior = result.at(-1);
      if (prior && job.kind === "reply" && prior[0].kind === "instruction" && answerKey(prior[0]) === answerKey(job)) prior.push(job);
      else if (prior && job.kind === "instruction" && prior[0].kind === "instruction" && answerKey(prior[0]) === answerKey(job)) prior.push(job);
      else result.push([job]);
    }
    return result;
  }
  function nextBatch(jobs, previous) {
    const albums = new Map();
    for (const job of jobs) {
      const key = albumKey(job);
      if (!albums.has(key)) albums.set(key, []);
      albums.get(key).push(job);
    }
    const choices = [...albums].map(([key, list]) => ({ key, units: units(list) }));
    const others = choices.filter(choice => choice.key !== previous);
    const first = (others.length ? others : choices).sort((a, b) => a.units[0][0].time - b.units[0][0].time)[0];
    return first && { key: first.key, ids: first.units.slice(0, 3).flat().map(job => job.id) };
  }
  function arrange(startBatch = false) {
    const manual = beforeAnswers(queue.filter(job => job.chosen || job.chain).sort((a, b) =>
      (b.chosen || b.chain) - (a.chosen || a.chain) || a.time - b.time));
    const waiting = queue.filter(job => !job.chosen && !job.chain);
    if (batch && ![current, ...waiting].some(job => job && albumKey(job) === batch.key && batch.ids.includes(job.id))) {
      lastAlbum = batch.key; batch = null;
    }
    const turn = batch || nextBatch(waiting, lastAlbum);
    if (!batch && startBatch && !manual.length) batch = turn || null;
    const first = turn ? turn.ids.map(id => waiting.find(job => job.id === id && albumKey(job) === turn.key)).filter(Boolean) : [];
    const rest = waiting.filter(job => !first.includes(job));
    // Preview the following turn without admitting later arrivals to the active batch.
    const next = nextBatch(rest, turn?.key || lastAlbum);
    const ahead = next ? next.ids.map(id => rest.find(job => job.id === id && albumKey(job) === next.key)) : [];
    queue = [...manual, ...first, ...ahead, ...rest.filter(job => !ahead.includes(job)).sort((a, b) => a.time - b.time)];
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
  // How the summary model may call a member: the service's spoken name, numbered where alike
  // members would collide, its duty, and whether another seat of this meeting shares that name.
  function person(who, ctx, m) {
    const id = who.identity, name = id?.spoken_name || "";
    const seats = m?.thread?.kind === "room" ? ctx.rooms?.get(m.thread.id)?.seats || [] : [];
    const twin = Boolean(name) && seats.some(s => s.desk !== id.key && s.identity?.spoken_name === name);
    return { name, role: id?.role || "", twin };
  }
  // Whom a boss message is said to: its one employee in private, or the seats it names in a
  // meeting; nobody named means the whole meeting.
  function listeners(m, ctx, boss) {
    if (!boss || m.kind === "forward") return [];
    const keys = m.to?.map(t => t.key).filter(Boolean) || [];
    const meeting = m.thread?.kind === "room" && m.kind !== "instruction";
    const seats = meeting ? ctx.rooms?.get(m.thread.id)?.seats || [] : [];
    const named = meeting ? (keys.length && keys.length < seats.length ? keys : []) : m.desk ? [m.desk] : keys.slice(0, 1);
    return named.map(key => m.to?.find(t => t.key === key)).filter(Boolean).map(who => person(who, ctx, m));
  }
  // Whose words a boss forward relays. A private task's question and reply share one id, so the
  // last match is the reply; a forwarded forward relays its own source; a source out of view is
  // relayed from its unnamed original speaker, never read as boss speaking now.
  function source(m, ctx) {
    const id = m.forward?.message_id;
    if (!id) return null;
    const original = ctx.c.messages?.findLast(o => o.message_id === id);
    if (!original) return { name: "原发言人", role: "" };
    if (original.kind === "forward") return source(original, ctx);
    if (original.speaker?.role === "manager") return { name: "boss", role: "" };
    return person(original.speaker || {}, ctx, original);
  }
  // Who takes part in an album, whichever of its messages asks: the bosses and members who spoke
  // in it, and the meeting's seats or the one desk. Kept per message list: a long history asks
  // once per message.
  const casts = new WeakMap();
  function cast(room, desk, ctx) {
    const messages = ctx.c?.messages || [];
    if (!casts.has(messages)) casts.set(messages, new Map());
    const known = casts.get(messages), key = room ? `room:${room}` : `desk:${desk}`;
    if (known.has(key)) return known.get(key);
    const own = messages.filter(m => room ? m.conversation === room : privateMessage(m, desk));
    const member = d => ({ provider: d.provider, model: d.art_model || d.model });
    const seated = (room ? (ctx.rooms?.get(room)?.seats || []).map(s => ctx.desks?.get(s.desk)) : [ctx.desks?.get(desk)]).filter(Boolean).map(member);
    const bosses = own.filter(m => m.speaker?.role === "manager").map(m => bossIdentity(m, ctx.c));
    const spoke = own.filter(m => m.speaker?.role === "member").map(m => {
      const d = ctx.desks?.get(m.desk);
      return { provider: m.speaker.provider || d?.provider, model: m.speaker.model || d?.art_model || d?.model };
    });
    const faces = new Set();
    const people = [...bosses.length ? bosses : [bossIdentity(null, ctx.c || {})], ...seated, ...spoke]
      .filter(p => (face => !faces.has(face) && faces.add(face))(AN.art.character(p.provider, p.model))).slice(0, COVER_FACES);
    known.set(key, people);
    return people;
  }
  // A message's album: the meeting it belongs to, or the private chat with its desk. Its title,
  // its cover, and the link that opens it in the office.
  function album(m, room, ctx) {
    const meeting = room || (ctx.rooms?.has(m.conversation) ? m.conversation : null);
    const desk = meeting ? null : m.desk || m.thread?.key || m.to?.find(t => t.key)?.key || null;
    const d = desk && ctx.desks?.get(desk);
    const id = meeting || m.conversation;
    const title = (meeting && (ctx.c?.conversations?.find?.(c => c.id === id)?.title || ctx.rooms?.get(meeting)?.title))
      || (d && (d.role || AN.deskName(d))) || (!desk && ctx.c?.conversations?.find?.(c => c.id === id)?.title) || "";
    const cover = meeting || desk ? cast(meeting, desk, ctx) : [bossIdentity(m, ctx.c || {})];
    return { title, cover, link: meeting ? `conv:${meeting}` : desk ? `desk:${desk}` : id ? `conv:${id}` : null };
  }
  function item(m, ctx) {
    const d = m.desk && ctx.desks.get(m.desk);
    const boss = m.speaker?.role === "manager";
    const { provider, model } = boss ? bossIdentity(m, ctx.c)
      : { provider: m.speaker?.provider || d?.provider || ctx.c.host, model: m.speaker?.model || d?.art_model || d?.model || "" };
    const room = m.thread?.kind === "room" ? m.thread.id : null;
    return { id: m.id, group: ctx.c?.group ?? null, place: AN.chatName?.(ctx.c) || "", album: album(m, room, ctx), room, desk: m.desk,
      kind: m.kind, conversation: m.conversation, round: m.round,
      boss, family: Avatars.family(provider, model) || (provider === "deepseek" ? "deepseek" : null),
      provider, model, role: boss ? "" : m.speaker?.identity?.role || "", label: boss ? "boss" : AN.deskName(m.speaker || {}),
      name: boss ? "boss" : m.speaker?.identity?.spoken_name || "", to: listeners(m, ctx, boss),
      private: m.thread?.kind !== "room" || m.kind === "instruction" || m.kept_private === true,
      forward: boss && m.kind === "forward" ? source(m, ctx) : null,
      text: m.text, excerpt: (AN.plain?.(m.text) ?? m.text ?? "").replace(/\s+/g, " ").slice(0, EXCERPT_CHARS),
      time: Date.parse(m.time) || 0, controller: new AbortController() };
  }
  async function audio(job) {
    const response = await fetch("/app/voice/speak", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
      signal: job.controller.signal, body: JSON.stringify({ family: job.family, text: job.text, full: false, boss: job.boss,
        scene: { name: job.name, role: job.role, room: Boolean(job.room), private: job.private, to: job.to, forward: job.forward } }) });
    if (response.status === 204) return null;
    if (!response.ok) {
      const data = await response.json().catch(() => ({}));
      const message = data.error || L("语音播报合成失败", "Voice synthesis failed");
      throw Object.assign(new Error(message), { configuration: /未配置|尚未开启|密钥|API.?key|401|403|unauthorized|authentication/i.test(message) });
    }
    const notice = response.headers.get("X-AgentNave-Notice");
    if (notice) job.notice = decodeURIComponent(notice);
    return response.blob();
  }
  function prepare(job) {
    expire(); checkBacklog();
    if (paused || follow?.overloaded || job.controller.signal.aborted) return Promise.resolve({});
    if (!job.prepared) {
      const controller = job.controller;
      job.preparing = true;
      job.prepared = audio(job).then(blob => ({ blob }), failure => ({ failure })).then(result => {
        if (job.controller !== controller || controller.signal.aborted) return {};
        job.preparing = false;
        return result;
      });
    }
    return job.prepared;
  }
  function cancelPreparation(job) {
    if (!job?.preparing) return;
    job.controller.abort(); job.controller = new AbortController();
    job.preparing = false; job.prepared = null;
  }
  // Detach the playback waiter, retaining audio, position and any synthesis already sent.
  function suspendCurrent() {
    if (!current) return;
    const job = current;
    generation++;
    job.suspended = true;
    job.audio?.pause(); job.speaking = false;
    job.resolve?.(); job.cancelWait?.();
    job.resolve = null; job.reject = null; job.cancelWait = null;
    queue.push(job); current = null;
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
  function discard(job) {
    job.controller.abort(); job.cancelWait?.(); finish(job);
  }
  // A stop leaves what played on the player for a moment, from where it can play again.
  function keepAfterStop(run) {
    const last = current || queue[0] || started.at(-1);
    run();
    if (!last || current || queue.length || follow) return;
    started = [last];
    recent = last;
    clearTimeout(recentTimer);
    recentTimer = setTimeout(() => { recent = null; started = []; changed(); }, RECENT_MS);
    changed();
  }
  function stop(notify = true) {
    generation++;
    batch = null; lastAlbum = null;
    paused = false;
    started = [];
    playlistFetch?.abort(); playlistFetch = null; playlistScope = null;
    for (const job of queue) discard(job);
    queue = [];
    if (current) { discard(current); current = null; }
    if (notify) changed();
  }
  // Ends a chained read; what listening queued meanwhile stays queued.
  function dropPlaylist(keepCurrent = false) {
    const held = keepCurrent && !current && paused ? queue[0] : null;
    playlistFetch?.abort(); playlistFetch = null; playlistScope = null;
    queue = queue.filter(job => {
      if (job === held) return true;
      if (job.followed) { delete job.chosen; delete job.chain; return true; }
      discard(job); return false;
    });
    if (!keepCurrent && current && !current.followed) interrupt();
    // A pause between clips with nothing left to resume would leave the player stuck.
    if (!current && !queue.length && !follow) paused = false;
  }
  // A hand-picked clip keeps its listening ownership so expiry can still withdraw it.
  function playNow(job) {
    expire();
    job = queue.find(other => other.id === job.id) || job;
    queue = queue.filter(other => other !== job);
    if (job.controller.signal.aborted) return;
    if (current?.id === job.id) { changed(); return; }
    if (current?.followed) suspendCurrent();
    else if (current) interrupt();
    queue.unshift(Object.assign(job, { chosen: ++picks }));
    arrange();
    changed();
    void drain();
  }
  // Ends the clip in progress but keeps what is queued after it.
  function interrupt() {
    generation++;
    if (current) { discard(current); current = null; }
  }
  function waitingJobs() { return [...(current && !current.speaking ? [current] : []), ...queue].filter(job => job.followed && job.waited != null); }
  function freezeWaiting() {
    for (const job of waitingJobs()) if (job.waitSince != null) {
      job.waited += Date.now() - job.waitSince; job.waitSince = null;
    }
  }
  function checkBacklog() {
    expire();
    if (!follow) return;
    clearTimeout(follow.backlogTimer);
    if (paused || follow.overloaded) return;
    const waits = waitingJobs().map(job => MAX_WAIT - job.waited - (job.waitSince == null ? 0 : Date.now() - job.waitSince));
    if (!waits.length) return;
    const left = Math.min(...waits);
    if (left > 0) { follow.backlogTimer = setTimeout(checkBacklog, left); return; }
    follow.overloaded = true;
    for (const job of queue) cancelPreparation(job);
    if (!current?.speaking) { cancelPreparation(current); suspendCurrent(); pause(); }
    error(L("消息积压，已暂停收听", "Message backlog, listening paused"), "backlog");
    changed();
  }
  function pause() {
    if (!current && !queue.length && !follow) return;
    freezeWaiting(); paused = true;
    if (current?.audio) { current.audio.pause(); current.speaking = false; }
    else if (current) { cancelPreparation(current); suspendCurrent(); }
    for (const job of queue) cancelPreparation(job);
    if (follow) clearTimeout(follow.backlogTimer);
    arrange(); changed();
  }
  function unpause() {
    const reset = follow?.overloaded;
    if (follow) follow.overloaded = false;
    for (const job of waitingJobs()) { if (reset) job.waited = 0; job.waitSince = Date.now(); }
    paused = false;
    if (shownError?.source === "backlog") clearError();
    checkBacklog();
  }
  async function resume() {
    expire();
    if (!paused || !ready() || halted) return;
    unpause();
    if (current?.audio) {
      const job = current, token = generation;
      job.audio.currentTime = Math.max(0, job.audio.currentTime - REWIND_SECONDS);
      job.speaking = true;
      changed();
      try { await job.audio.play(); if (token === generation) prefetch(); }
      catch (failure) { if (token === generation && !paused) job.reject?.(failure); }
    } else { changed(); void drain(); }
  }
  async function play(job, token) {
    if (!job.audio) {
      const result = await Promise.race([prepare(job), new Promise(resolve => { job.cancelWait = () => resolve({}); })]);
      job.cancelWait = null;
      expire(); checkBacklog();
      if (token !== generation || paused || follow?.overloaded) return;
      if (result.failure) throw result.failure;
      if (!result.blob) return;
      if (job.notice) error(job.notice, "notice", job);
      job.url = URL.createObjectURL(result.blob);
      job.audio = new Audio(job.url);
    }
    await new Promise((resolve, reject) => {
      job.resolve = resolve; job.reject = reject;
      job.audio.onended = () => { job.completed = true; resolve(); };
      job.audio.onerror = () => reject(new Error(L("无法播放语音播报", "Failed to play voice audio")));
      job.audio.play().then(() => {
        if (token !== generation || paused) return;
        job.speaking = true; job.waited = null; job.waitSince = null;
        if (shownError?.source === "playback") clearError();
        changed(); prefetch();
      }, failure => { if (!paused) reject(failure); });
    });
    return job.completed;
  }
  // Only the next clip is prefetched, after the current clip has actually started.
  function prefetch() {
    expire(); checkBacklog();
    if (!paused && !follow?.overloaded && current?.speaking) {
      arrange();
      for (const job of queue.slice(1)) cancelPreparation(job);
      if (queue[0]) prepare(queue[0]);
    }
  }
  async function drain() {
    expire();
    checkBacklog();
    if (running || paused || follow?.overloaded || halted || !ready()) return;
    running = true;
    try {
      while (queue.length && !paused && !halted && ready()) {
        expire();
        if (!queue.length) break;
        checkBacklog();
        if (paused || follow?.overloaded) break;
        arrange(true);
        const token = generation;
        const job = current = queue.shift();
        if (started.at(-1)?.id !== job.id) started.push(job);
        job.suspended = false;
        // Once only listening is left, the chained read is over.
        if (playlistScope && !playlistFetch && job.followed && !queue.some(next => !next.followed)) playlistScope = null;
        changed();
        try {
          const played = await play(job, token);
          if (token === generation && played) {
            failures = 0; remember(job);
            if (shownError?.source === "playback" || shownError?.source === "notice" && shownError.job !== job) clearError();
          }
        } catch (failure) {
          if (token === generation) {
            failures++;
            error(failure.message, failure.configuration ? "configuration" : "playback", job);
            if (failures >= 3) { halted = true; unlisten(false); stop(); }
          }
        } finally {
          if (!job.suspended) finish(job);
          if (current === job) current = null;
          if (follow?.overloaded) pause();
          checkBacklog();
          changed();
        }
      }
    } finally {
      running = false;
      if (paused && !current && !queue.length && !follow) { paused = false; changed(); }
      if (!queue.length && !playlistFetch && playlistScope) { playlistScope = null; changed(); }
    }
  }
  // Office polls refresh settings; only an explicit listen follows new messages. Settings seldom
  // change and announce it (refresh()); the settings are read again while something is playing or
  // followed, when the window comes back, and otherwise every IDLE_RELOAD_MS.
  const IDLE_RELOAD_MS = 15000;
  window.addEventListener("focus", () => { loadedAt = -Infinity; });
  window.addEventListener("agentnave:focus", () => { loadedAt = -Infinity; });
  function ingest(chat) {
    group = chat.group;
    expire();
    queueMicrotask(drain);
    if (current || queue.length || follow || Date.now() - loadedAt >= IDLE_RELOAD_MS)
      void reload().catch(failure => error(failure.message, "load"));
  }
  // Hands each message to `take` once, when it first becomes readable in scope: a briefing only
  // with its answer. `seen` remembers what was readable already.
  function readable(messages, seen, take, column) {
    const answers = new Map(messages.filter(m => m.kind === "reply" && eligible(m)).map(m =>
      [answerKey(m), { message: m, seen: seen.get(m.id) === true }]));
    for (const m of messages) {
      const briefing = m.kind === "instruction" && answerKey(m);
      const answer = briefing ? answers.get(briefing) : null;
      const settled = eligible(m) && (!briefing || Boolean(answer));
      const can = settled && scopedMessage(m) && (!briefing || scopedMessage(answer.message));
      const was = seen.get(m.id) === true;
      if (can && !was && !answer?.seen) take(m);
      // A column has all its history to read; a workspace, whatever the scope, what comes next.
      seen.set(m.id, was || (column ? can : settled));
    }
  }
  const contextOf = chat => ({ c: chat, desks: new Map([...(chat.desks || []), ...(chat.optimized?.desks || [])].map(d => [d.key, d])),
    rooms: new Map((chat.rooms || []).map(r => [r.id, r])) });
  // Listening to a workspace, a meeting or a private chat, picked in the office or the menu bar
  // panel, even while that view or the office is hidden.
  function followedIn(chat, session) {
    const shown = session.shown && new Set(session.shown(chat).map(m => m.id));
    return m => chat.group === session.group && (shown ? shown.has(m.id) : session.desk ? privateMessage(m, session.desk) : !session.room || m.conversation === session.room);
  }
  async function loadFollowed(session, initial = false) {
    const path = `/app/chats/${encodeURIComponent(session.group)}${session.room ? `/focus/${encodeURIComponent(session.room)}` : initial && session.desk ? "/history" : ""}`;
    const response = await fetch(path, { cache: "no-store", signal: session.controller.signal });
    if (!response.ok) throw Object.assign(new Error(L("无法读取要收听的对话", "Failed to load the conversation to listen to")), { status: response.status });
    return response.json();
  }
  const followTitle = (chat, session) => (session.room && (chat.rooms?.find?.(r => r.id === session.room)?.title
    || chat.conversations?.find?.(c => c.id === session.room)?.title))
    || (session.desk && (d => d && AN.deskName(d))(chat.desks?.find?.(d => d.key === session.desk)))
    || AN.chatName?.(chat) || session.group;
  async function listen(target) {
    if (!target?.group || !ready()) return;
    // Asking to listen is asking to speak again after playback stopped on errors.
    halted = false; failures = 0;
    unlisten(false);
    if (!current && !queue.length) started = [];
    const session = follow = { group: target.group, room: target.room || null, desk: target.desk || null, shown: target.desk ? target.shown : null, title: "", expires_at: Date.now() + (config.listen_minutes ?? 60) * 60000, seen: new Map(), controller: new AbortController(), timer: null, expiryTimer: null, backlogTimer: null, overloaded: false };
    batch = null; lastAlbum = null;
    session.expiryTimer = setTimeout(expire, session.expires_at - Date.now());
    changed();
    let chat;
    try { chat = await loadFollowed(session, true); } catch (failure) { if (follow === session) { error(failure.message, "read", null, () => void listen(target)); unlisten(); } return; }
    expire();
    if (follow !== session) return;
    session.title = followTitle(chat, session);
    collectFollowed(chat, session, true);
    arrange(); changed(); void drain();
    session.timer = setTimeout(() => void heed(session), FOLLOW_EVERY);
  }
  function collectFollowed(chat, session, initial = false) {
    const ctx = contextOf(chat), inFollow = followedIn(chat, session);
    readable(chat.messages, session.seen, m => {
      if (!inFollow(m) || initial && !session.room && !session.desk || !ready() || halted) return;
      const job = item(m, ctx);
      if (done(job)) return;
      const known = [current, ...queue].find(other => other?.id === job.id && other.group === job.group);
      if (known) { known.followed = true; return; }
      Object.assign(job, { followed: true, waited: initial ? null : 0, waitSince: initial || paused ? null : Date.now() });
      queue.push(job);
    }, session.room || session.desk);
    checkBacklog();
  }
  async function heed(session) {
    expire();
    if (follow !== session) return;
    let chat = null;
    try { chat = await loadFollowed(session); } catch (failure) {
      // The workspace or meeting is gone: nothing more will be said there.
      if (follow === session && failure.status === 404) { unlisten(); return; }
    }
    expire();
    if (follow !== session) return;
    if (chat) {
      collectFollowed(chat, session);
      arrange();
      if (current) prefetch();
      changed();
      void drain();
    }
    session.timer = setTimeout(() => void heed(session), FOLLOW_EVERY);
  }
  // Expiry leaves only the clip already speaking; sleeping past the deadline cannot extend it.
  function expire() {
    if (!follow || Date.now() < follow.expires_at) return;
    unlisten(false, true);
    error(L("收听已结束", "Listening ended"), "notice", current);
    changed();
  }
  // Stops listening and what listening queued; anything picked by hand keeps playing.
  function unlisten(notify = true, keepPlaying = false) {
    if (!follow) return;
    clearTimeout(follow.timer);
    clearTimeout(follow.expiryTimer);
    clearTimeout(follow.backlogTimer);
    follow.controller.abort();
    follow = null; batch = null; lastAlbum = null;
    if (shownError?.source === "backlog") clearError();
    queue = queue.filter(job => {
      if (!job.followed) return true;
      if (job.chosen) { job.followed = false; return true; }
      discard(job); return false;
    });
    if (current?.followed) {
      // The finishing clip must not be queued again if a manual read cuts it short.
      if (current.chosen || (current.chain || keepPlaying) && current.speaking) current.followed = false;
      else interrupt();
    }
    if (!current && !queue.length && !follow) paused = false;
    if (notify) { changed(); void drain(); }
  }
  // The messages of one conversation a chained read follows, in the order they are shown.
  function playable(chat, target) {
    const ctx = contextOf(chat);
    // A briefing waits for its answer, as in listening.
    const answered = new Set(chat.messages.filter(m => m.kind === "reply" && eligible(m) && scopedMessage(m)).map(answerKey).filter(Boolean));
    const shown = target.shown && new Set(target.shown(chat).map(m => m.id));
    const list = chat.messages.filter(m => eligible(m) && (m.kind !== "instruction" || !answerKey(m) || answered.has(answerKey(m)))
      && (shown ? shown.has(m.id) : m.conversation === target.id)).map(m => item(m, ctx));
    list.sort((a, b) => a.time - b.time);
    return beforeAnswers(list);
  }
  // Reads the clicked message at once, then, with chaining on, the rest of its conversation after it.
  async function readFrom(m, chosen) {
    if (paused || follow?.overloaded) unpause();
    dropPlaylist();
    playNow(chosen);
    const picked = picks, listening = follow;
    const listened = new Set([current, ...queue].filter(job => job?.followed).map(job => job.id));
    const heard = new Set([...completed.values()].flatMap(ids => [...ids]));
    const atClick = channelContext?.c.messages || [m];
    const existing = new Map(atClick.map(message => [message.id, eligible(message)]));
    existing.set(m.id, true);
    const oldest = Math.min(chosen.time, ...atClick.map(message => Date.parse(message.time) || 0));
    const target = config?.continue_read !== true ? null : roomView ? roomView
      : privateView ? { group: channelContext?.c.group || group, id: privateView, shown: deskShown }
        : m.conversation ? { group, id: m.conversation } : null;
    if (!target) return;
    const load = playlistFetch = new AbortController();
    playlistScope = target.id;
    try {
      const path = target.shown ? "history" : `focus/${encodeURIComponent(target.id)}`;
      const response = await fetch(`/app/chats/${encodeURIComponent(target.group)}/${path}`,
        { cache: "no-store", signal: load.signal });
      if (!response.ok) throw new Error(L("无法读取完整对话消息", "Failed to load full conversation"));
      const chat = await response.json();
      if (playlistFetch !== load) return;
      const list = playable(chat, target).filter(job => existing.get(job.id) === true || !existing.has(job.id) && job.time < oldest);
      const start = list.findIndex(job => job.id === chosen.id);
      // A briefing still waiting for its answer is not in the list; the chain starts after it.
      const after = start < 0 ? list.filter(job => job.time > chosen.time) : list.slice(start + 1);
      const rest = scopedRead(after).filter(job => job.id !== current?.id && (!done(job) || heard.has(job.id))
        && (!listened.has(job.id) || follow === listening))
        .map(job => queue.find(other => other.id === job.id) || job);
      const ids = new Set(rest.map(job => job.id));
      queue = queue.filter(job => !ids.has(job.id));
      for (const job of rest) {
        Object.assign(job, { chain: picked });
        if (listened.has(job.id)) job.followed = true;
      }
      queue.push(...rest);
      arrange();
      if (shownError?.source === "read") clearError();
      if (current) prefetch();
    } catch (failure) {
      if (playlistFetch === load) error(failure.message, "read", null, () => void readFrom(m, chosen));
    } finally {
      if (playlistFetch === load) {
        playlistFetch = null;
        if (!queue.some(job => !job.followed) && !current?.chosen) playlistScope = null;
        changed(); void drain();
      }
    }
  }
  // Opening another view or closing the drawer leaves playback alone, a chained read included.
  function controls(visible, room = null, ctx = null, desk = null, shown = null, listenShown = shown) {
    roomView = room;
    privateView = desk;
    deskShown = shown; deskListenShown = listenShown;
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
    // Only a clip of this workspace and of this view is shown here; another's stays in the top bar.
    const here = current && current.group === c?.group
      && (roomView ? current.room === roomView.id : privateView ? current.desk === privateView : true) ? current : null;
    const boss = here?.boss ? here : bossIdentity(said || null, c || {});
    let speaking = null;
    const people = [{ key: "@boss", provider: boss.provider, model: boss.model, label: "boss", host: true }];
    const room = roomView && ctx?.rooms?.get(roomView.id);
    const desks = room ? room.seats.map(s => ctx.desks.get(s.desk)).filter(Boolean)
      : privateView ? [ctx?.desks.get(privateView)].filter(Boolean) : [];
    for (const d of desks) people.push({ key: d.key, provider: d.provider, model: d.art_model || d.model, role: d.role, label: AN.deskName(d) });
    if (here && !here.boss && !people.some(p => p.key === here.desk))
      people.push({ key: here.desk || here.id, provider: here.provider, model: here.model, role: here.role, label: here.label });
    const keep = new Set();
    for (const person of people) {
      keep.add(person.key);
      const character = AN.art.character(person.provider, person.model);
      let rec = participants.get(person.key);
      if (!rec) {
        rec = { el: h(person.host ? "div.voice-person.boss" : "div.voice-person"), head: h("div.voice-person-head") };
        rec.el.append(rec.head); participants.set(person.key, rec);
      }
      if (rec.character !== character) { rec.head.replaceChildren(AN.art.head(character)); rec.character = character; }
      const active = here && (person.host ? here.boss : !here.boss && person.key === (here.desk || here.id));
      const name = person.host ? "boss" : person.role || person.label;
      if (active) speaking = person.key;
      rec.el.dataset.speaking = String(Boolean(active && here.speaking));
      rec.el.dataset.preparing = String(Boolean(active && !here.speaking));
      rec.el.setAttribute("aria-label", `${person.host ? L("主持人", "Host") : L("发言人", "Speaker")} ${name}${active ? paused ? L("，已暂停", ", paused") : here.speaking ? L("，正在播放", ", playing") : L("，正在准备", ", preparing") : ""}`);
      rec.el.title = person.host || !person.role ? name : `${name} · ${person.label}`;
    }
    for (const [key, rec] of participants) if (!keep.has(key)) { rec.el.remove(); participants.delete(key); }
    // A long roster folds, but whoever is speaking always stays in view.
    const shown = people.length <= ROSTER_LIMIT ? people : people.slice(0, ROSTER_LIMIT - 1);
    if (speaking && !shown.some(p => p.key === speaking)) shown[shown.length - 1] = people.find(p => p.key === speaking);
    more.textContent = `+${people.length - shown.length}`;
    more.title = people.filter(p => !shown.includes(p)).map(p => p.host ? "boss" : p.role || p.label).join(L("、", ", "));
    AN.reconcile(roster, [...shown.map(p => participants.get(p.key).el), ...people.length > shown.length ? [more] : []]);
    actions.hidden = !channelVisible || !config?.enabled || !(roomView || privateView);
    actions.dataset.live = String(Boolean(here?.speaking));
    publish();
  }
  // What the top bar control draws, here and in the menu bar panel: the clip playing first, then
  // what is queued.
  function snapshot() {
    const jobs = [...(current ? [current] : []), ...queue];
    if (recent && (jobs.length || follow)) { clearTimeout(recentTimer); recent = null; }
    const view = (job, state) => ({
      id: job.id, group: job.group, place: job.place, album: job.album, conversation: job.conversation, provider: job.provider, model: job.model,
      who: (job.boss && AN.modelName(job.model)) || job.label,
      excerpt: job.excerpt, state,
    });
    return AN.voiceState({
      enabled: config ? Boolean(config.enabled) : shownError?.source === "load", ready: ready(), halted,
      sounding: Boolean(current?.speaking), backlog: Boolean(follow?.overloaded),
      error_source: shownError?.source || null, retryable: Boolean(shownError?.retry || shownError?.source === "playback" && shownError.job),
      can_previous: ready() && !halted && started.length > Number(Boolean(current)),
      can_next: ready() && !halted && jobs.length > 1,
      can_stop: Boolean(jobs.length || follow || playlistFetch || shownError || paused),
      continue_read: config?.continue_read === true,
      state: !jobs.length ? "idle" : paused ? "paused" : current?.speaking ? "speaking" : "preparing",
      playing: Boolean(current), paused, error: shownError?.message || null, follow: follow && { group: follow.group, room: follow.room, desk: follow.desk, title: follow.title, expires_at: follow.expires_at },
      recent: recent && view(recent, "stopped"),
      jobs: jobs.map(job => view(job,
        job !== current && !(paused && !current && job === jobs[0]) ? "queued" : paused ? "paused" : job.speaking ? "speaking" : "preparing")),
    });
  }
  function publish() {
    workspaceListen.draw();
    columnListen.draw();
    const snap = snapshot();
    menu.render(snap);
    // The panel needs only the album playing; queue changes alone are not sent.
    const text = JSON.stringify({ ...snap, jobs: snap.jobs.slice(0, 1) });
    // Before its settings load the office has nothing to tell; a waiting click needs them.
    if (leaving || !config && !shownError || text === published) return;
    published = text;
    // Once per turn, numbered, so a clip ending and the next starting arrive as one, in order.
    if (posting) return;
    posting = true;
    queueMicrotask(() => {
      posting = false;
      if (leaving) return;
      fetch("/app/voice/player", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
        body: JSON.stringify({ ...JSON.parse(published), seq: ++sequence }) }).catch(() => {});
    });
  }
  function openAlbum(job) {
    AN.officeSettings?.close();
    const path = `/chat/${encodeURIComponent(job.group)}${job.album?.link ? `#${encodeURIComponent(job.album.link)}` : ""}`;
    // The office's own navigation (chat-state.js) opens it in place; elsewhere, by address.
    if (typeof go === "function") go(path);
    else location.href = AN.href ? AN.href(path) : path;
  }
  async function flip(field) {
    if (!config) return;
    const on = config[field] === true;
    publish();
    if (on) dropPlaylist(true);
    try {
      const response = await fetch("/app/voice", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body: JSON.stringify({ ...config, [field]: !on }) });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || L("无法保存语音播报设置", "Failed to save voice settings"));
      config = data;
    } catch (failure) { error(failure.message, "save"); }
    finally { changed(); }
  }
  // The panel's click brings the window up as it arrives; a list rebuilt before that stays unpainted.
  function whenShown(run) {
    if (document.hidden) document.addEventListener("visibilitychange", () => whenShown(run), { once: true });
    else run();
  }
  // A click on the top bar control, here or in the menu bar panel.
  function command(kind, value) {
    if (kind === "toggle") void (paused ? resume() : pause());
    else if (kind === "next") next();
    else if (kind === "previous") previous();
    else if (kind === "flip" && value === "continue_read") void flip(value);
    else if (kind === "stop_all") keepAfterStop(() => { unlisten(false); stop(false); halted = false; failures = 0; clearError(); changed(); });
    else if (kind === "retry") retry();
    else if (kind === "settings") {
      if (AN.navigate) AN.navigate("/settings/#voice-settings");
      else location.href = AN.href ? AN.href("/settings/#voice-settings") : "/settings/#voice-settings";
    }
    else if (kind === "open") whenShown(() => openAlbum(value));
    else if (kind === "locate") whenShown(() => { AN.officeSettings?.close(); window.dispatchEvent(new CustomEvent("agentnave:voice-locate", { detail: value })); });
    else if (kind === "listen") void listen(value);
    else if (kind === "unlisten") keepAfterStop(() => unlisten());
    else if (kind === "play") {
      if (current?.id === value) return;
      const job = queue.find(other => other.id === value);
      if (job && ready() && !halted) {
        if (paused || follow?.overloaded) unpause();
        playNow(job);
      }
    }
  }
  window.addEventListener("agentnave:voice-command", ({ detail }) => command(detail?.kind, detail?.value));
  function decorate(rec, m, ctx) {
    if (!rec.read) {
      rec.readLabel = h("span", {}, L("读摘要", "Read summary"));
      rec.read = h("button.voice-read", { type: "button" }, h("span.voice-waves", {}, h("i"), h("i"), h("i")), rec.readLabel);
      rec.read.addEventListener("click", event => {
        event?.stopPropagation();
        const selected = current || paused && queue[0];
        const same = selected?.id === rec.voiceSource?.[0].id;
        if (same) { if (paused) void resume(); else pause(); return; }
        if (!ready() || halted) return;
        // The chosen message goes first; what listening queued still follows it.
        // Built at the click: rendering every message must not summarize each one.
        const [message, context] = rec.voiceSource;
        void readFrom(m, item(message, context));
      });
      rec.bub.prepend(rec.read); // Keep the action inside the chat bubble, above its text.
    }
    rec.read.hidden = !eligible(m) || !config?.enabled;
    rec.read.disabled = !ready() || halted;
    if (eligible(m)) rec.voiceSource = [m, ctx];
    const reading = (current || paused && queue[0])?.id === m.id;
    const idleLabel = L("读摘要", "Read summary");
    rec.readLabel.textContent = reading ? paused ? L("继续", "Resume") : L("暂停", "Pause") : idleLabel;
    rec.read.title = reading ? paused ? L("继续朗读", "Resume reading") : L("暂停朗读", "Pause reading") : ready() ? idleLabel : L("请在设置 → 语音中开启语音播报并配置音色引擎", "Enable voice readout and configure an engine in Settings → Voice");
    rec.read.setAttribute("aria-label", rec.read.title);
    rec.read.setAttribute("aria-pressed", String(Boolean(reading)));
    rec.read.dataset.sounding = String(Boolean(reading && !paused && current.speaking));
  }
  // Back one message; what it interrupts comes back after it.
  function previous() {
    expire();
    if (!ready() || halted || started.length <= Number(Boolean(current))) return;
    const keepPaused = paused || follow?.overloaded;
    if (current) { started.pop(); suspendCurrent(); }
    const job = started.pop();
    playNow({ ...job, controller: new AbortController(), audio: null, url: null, prepared: null, preparing: false, speaking: false,
      followed: false, suspended: false, resolve: null, reject: null, cancelWait: null });
    paused = Boolean(keepPaused);
  }
  function next() {
    expire();
    if (!ready() || halted || queue.length + Number(Boolean(current)) < 2) return;
    const keepPaused = paused || follow?.overloaded;
    if (current) interrupt();
    else discard(queue.shift());
    paused = Boolean(keepPaused);
    arrange(); changed();
    if (!paused) void drain();
  }
  function retry() {
    const problem = shownError;
    if (!problem || !ready() || problem.source === "configuration") return;
    halted = false; failures = 0; clearError();
    if (problem.retry) problem.retry();
    else if (problem.job) {
      const job = { ...problem.job, controller: new AbortController(), audio: null, url: null, prepared: null, preparing: false, completed: false };
      playNow(job);
    }
  }
  document.addEventListener("visibilitychange", expire);
  // Leaving the page publishes nothing more: the tray has already cleared what it showed.
  window.addEventListener("pagehide", () => { leaving = true; unlisten(false); stop(false); });
  AN.voice = { ingest, decorate, controls, stop, beforeAnswers, workspaceListen: workspaceListen.el,
    refresh: () => reload().catch(failure => error(failure.message, "load")) };
  void reload().catch(failure => error(failure.message, "load"));
}
