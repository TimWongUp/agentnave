"use strict";
// The office alone owns playback. A generation invalidates fetches when stopped.
{
  const { h, L = (zh) => zh } = AN;
  let config = null, group = null, view = null, observed = new Map(), skip = true;
  let queue = [], current = null, generation = 0, failures = 0, halted = false, speakingGroup = null, picks = 0;
  let running = false, loading = null, baselineEpoch = 0;
  let roomView = null, playlistScope = null, playlistFetch = null;
  let privateView = null, deskShown = null, channelVisible = false, paused = false, wakePlayback = null;
  let saving = false, published = null, follow = null, leaving = false, posting = false;
  // Numbers start from the clock, so a reloaded page never counts below what the last one sent.
  let sequence = Date.now();
  const switchButton = label => h("button.voice-toggle", { type: "button", "aria-label": label, "aria-pressed": "false" }, h("span.voice-track"), label);
  // The top bar's voice control holds what applies everywhere, so it is reachable without the
  // drawer: pause, auto read of the whole workspace, and chaining a clicked message down its
  // conversation. The menu bar panel shows the same control and sends its clicks here.
  const menu = AN.voiceMenu(document.querySelector?.("header.bar .bar-end"), command);
  // The drawer's channel: who is here, and auto read of only the list it shows, kept per viewer.
  const LIST_AUTO_KEY = "agentnave.voice.list-auto";
  let listAuto = false;
  try { listAuto = localStorage.getItem(LIST_AUTO_KEY) === "1"; } catch { /* storage may be unavailable */ }
  const listToggle = switchButton(L("本列表自动读", "Auto-read this list"));
  const actions = document.getElementById("voice-actions");
  const roster = h("div.voice-roster");
  const more = h("div.voice-more");
  actions.append(roster, listToggle);
  let channelContext = null;
  const participants = new Map();
  const ROSTER_LIMIT = 6;
  const REWIND_SECONDS = 2;
  const FOLLOW_EVERY = 2000;
  // A playlist row shows one line of a message.
  const EXCERPT_CHARS = 120;
  const ready = () => Boolean(config?.enabled && config.keys?.[config.engine]);
  const changed = () => { updateChannel(); window.dispatchEvent(new Event("agentnave:voice")); };
  // Its own banner: the chat refresh hides the connection banner on every poll.
  const banner = document.getElementById("voice-banner");
  let shownError = null;
  function syncBannerHeight() {
    document.documentElement.style.setProperty("--voice-banner-h", `${banner.hidden ? 0 : banner.getBoundingClientRect().height}px`);
  }
  new ResizeObserver(syncBannerHeight).observe(banner);
  function error(message, source, job = null) {
    document.getElementById("voice-banner-text").textContent = message;
    banner.hidden = false;
    syncBannerHeight();
    shownError = { source, job, message };
    publish();
    document.getElementById("announce").textContent = message;
  }
  function clearError() { banner.hidden = true; shownError = null; syncBannerHeight(); publish(); }
  document.getElementById("voice-banner-close").addEventListener("click", clearError);
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
      if (shownError?.source === "load") clearError();
      const previous = config;
      const different = !previous || JSON.stringify(previous) !== JSON.stringify(next);
      const usable = c => Boolean(c.enabled && c.keys?.[c.engine]);
      // Only a change that silences or redirects playback interrupts it; other settings
      // apply to the next clip.
      const shouldInterrupt = previous && different && (usable(previous) !== usable(next) || previous.engine !== next.engine);
      const autoOff = previous?.auto_read === true && next.auto_read !== true;
      if (previous && different) {
        clearError();
        halted = false; failures = 0;
        if (previous.enabled !== next.enabled || previous.auto_read !== next.auto_read || usable(previous) !== usable(next)) { skip = true; baselineEpoch++; }
      }
      config = next;
      if (previous && previous.read_scope !== next.read_scope) {
        const allowed = new Set(scopedRead([...(current ? [current] : []), ...queue]));
        queue = queue.filter(job => allowed.has(job) || (job.controller.abort(), false));
        if (current && !allowed.has(current)) interrupt();
      }
      switches();
      // Silenced voice leaves nothing to listen with.
      if (shouldInterrupt && !usable(next)) unlisten(false);
      if (shouldInterrupt) stop();
      else if (autoOff) dropAutomatic();
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
  // One boss at a time: the message picked by hand last and its chain go first, then the rest of
  // the workspace speaking now, then each other workspace from its oldest message, in time order.
  function arrange() {
    const oldest = new Map();
    for (const job of queue) if (!(oldest.get(job.group) <= job.time)) oldest.set(job.group, job.time);
    const speaking = current?.group ?? speakingGroup;
    const turn = job => job.group === speaking ? -Infinity : oldest.get(job.group);
    queue.sort((a, b) => (b.chosen || b.chain || 0) - (a.chosen || a.chain || 0)
      || (a.group === b.group ? a.time - b.time : turn(a) - turn(b) || String(a.group).localeCompare(String(b.group))));
    queue = beforeAnswers(queue);
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
    // A desk on its CLI's default model knows the model only from what the CLI reported.
    const familyOf = desk => Avatars.family(desk.provider, desk.display_model || desk.model);
    const family = familyOf(d);
    const seats = m?.thread?.kind === "room" ? ctx.rooms?.get(m.thread.id)?.seats || [] : [];
    const twin = Boolean(family) && seats.some(s => s.desk !== d.key && (t => t && familyOf(t) === family)(ctx.desks.get(s.desk)));
    return { name: spokenName(d.provider, d.display_model || d.model), role: d.role || "", twin };
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
  // The conversation a message belongs to, as a playlist's album: its title, and the link that
  // opens it in the office (a meeting or a task by its conversation, else the desk).
  function album(m, room, d, ctx) {
    const id = room || m.conversation;
    const title = ctx.c?.conversations?.find?.(c => c.id === id)?.title
      || (room && ctx.rooms?.get(room)?.title) || (d && AN.deskName(d)) || "";
    // Its cover: the boss, then the meeting's seats or the one desk, at most four.
    const boss = bossIdentity(m, ctx.c || {});
    const seats = room ? (ctx.rooms?.get(room)?.seats || []).map(s => ctx.desks?.get(s.desk)) : [d];
    const faces = new Set();
    const cover = [boss, ...seats.filter(Boolean).map(x => ({ provider: x.provider, model: x.display_model || x.model }))]
      .filter(p => (face => !faces.has(face) && faces.add(face))(AN.art.character(p.provider, p.model))).slice(0, 4);
    return { title, cover, link: id ? `conv:${id}` : m.desk ? `desk:${m.desk}` : null };
  }
  function item(m, ctx, automatic = false) {
    const d = m.desk && ctx.desks.get(m.desk);
    const boss = m.speaker?.role === "manager";
    const { provider, model } = boss ? bossIdentity(m, ctx.c)
      : { provider: m.speaker?.provider || d?.provider || ctx.c.host, model: m.speaker?.model || d?.display_model || d?.model || "" };
    const room = m.thread?.kind === "room" ? m.thread.id : null;
    return { id: m.id, group: ctx.c?.group ?? null, place: AN.chatName?.(ctx.c) || "", album: album(m, room, d, ctx), room, desk: m.desk,
      kind: m.kind, conversation: m.conversation, round: m.round,
      boss, family: Avatars.family(provider, model) || null,
      provider, model, role: boss ? "" : d?.role || m.speaker?.desk_role || "", label: boss ? "boss" : d ? AN.deskName(d) : m.speaker?.label || provider,
      name: boss ? "boss" : spokenName(provider, model), to: listeners(m, ctx, boss),
      private: m.thread?.kind !== "room" || m.kind === "instruction" || m.kept_private === true,
      forward: boss && m.kind === "forward" ? source(m, ctx) : null,
      text: m.text, excerpt: (AN.plain?.(m.text) ?? m.text ?? "").replace(/\s+/g, " ").slice(0, EXCERPT_CHARS),
      time: Date.parse(m.time) || 0, automatic, controller: new AbortController() };
  }
  async function audio(job) {
    const response = await fetch("/app/voice/speak", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" },
      signal: job.controller.signal, body: JSON.stringify({ family: job.family, text: job.text, full: false, boss: job.boss,
        scene: { name: job.name, role: job.role, room: Boolean(job.room), private: job.private, to: job.to, forward: job.forward } }) });
    if (response.status === 204) return null;
    if (!response.ok) {
      const data = await response.json().catch(() => ({}));
      throw new Error(data.error || L("语音播报合成失败", "Voice synthesis failed"));
    }
    const notice = response.headers.get("X-AgentNave-Notice");
    if (notice) job.notice = decodeURIComponent(notice);
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
    // A pause between clips with nothing left to resume would leave the player stuck.
    if (!current && !queue.length) paused = false;
  }
  // Plays a job at once, ahead of the queue. An auto read cut short starts over after it
  // instead of being lost.
  function playNow(job) {
    job = queue.find(other => other.id === job.id) || job;
    queue = queue.filter(other => other.id !== job.id || (other !== job && other.controller.abort(), false));
    if (current?.id === job.id) { changed(); return; }
    queue.unshift(Object.assign(job, { chosen: ++picks, automatic: false }));
    if (current?.automatic) {
      const { controller, prepared, audio, url, speaking, resolve, reject, notice, chosen, ...copy } = current;
      queue.push({ ...copy, automatic: true, controller: new AbortController() });
      arrange();
    }
    interrupt();
    changed();
    void drain();
  }
  // Auto read turned off: what it queued goes, what was picked by hand stays.
  function dropAutomatic() {
    const auto = job => job.automatic && !job.followed;
    queue = queue.filter(job => !auto(job) || (job.controller.abort(), false));
    if (current && auto(current)) interrupt();
    if (!current && !queue.length) paused = false;
    changed();
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
      // Back two seconds, so the sentence that was cut is heard again in context.
      job.audio.currentTime = Math.max(0, job.audio.currentTime - REWIND_SECONDS);
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
    if (job.notice) error(job.notice, "notice", job);
    job.url = URL.createObjectURL(blob);
    job.audio = new Audio(job.url);
    job.speaking = true;
    changed();
    await new Promise((resolve, reject) => {
      job.resolve = resolve;
      job.reject = reject;
      job.audio.onended = resolve;
      job.audio.onerror = () => reject(new Error(L("无法播放语音播报", "Failed to play voice audio")));
      job.audio.play().catch(failure => { if (!paused) reject(failure); });
    });
    return true;
  }
  // While a clip plays, the next two are already being synthesized.
  function prefetch() {
    if (!paused) for (const next of queue.slice(0, 2)) prepare(next);
  }
  async function drain() {
    if (running || paused || halted || !ready()) return;
    running = true;
    try {
      while (queue.length && !paused && !halted && ready()) {
        const token = generation;
        const job = current = queue.shift();
        speakingGroup = job.group;
        // Once only auto read is left, the chained read is over.
        if (playlistScope && !playlistFetch && job.automatic && !queue.some(next => !next.automatic)) playlistScope = null;
        prepare(job);
        prefetch();
        changed();
        try {
          const played = await play(job, token);
          if (token === generation && played) {
            failures = 0;
            if (shownError?.source === "playback" || shownError?.source === "notice" && shownError.job !== job) clearError();
          }
        } catch (failure) {
          if (token === generation) {
            failures++;
            error(failures >= 3 ? L(`语音播报已停止：${failure.message}。请重新开启语音。`, `Voice readout stopped: ${failure.message}. Re-enable voice to continue.`) : failure.message, "playback");
            if (failures >= 3) { halted = true; unlisten(false); stop(); }
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
    // Another workspace starts its own auto read; what is already queued plays on.
    if (group !== chat.group) {
      observed = new Map(); group = chat.group;
    }
    view = focus;
    const everywhere = config?.auto_read === true;
    const shown = !everywhere && listAuto && channelVisible && privateView && deskShown ? new Set(deskShown(chat).map(m => m.id)) : null;
    // Auto read of the drawer's list: the open meeting, what the open desk shows, or the whole
    // workspace from its directory.
    const listed = m => listAuto && channelVisible && (roomView ? m.conversation === roomView.id
      : privateView ? (shown ? shown.has(m.id) : privateMessage(m, privateView)) : true);
    // What a workspace or meeting being listened to says is read by its own follower.
    readable(chat.messages, observed, m => {
      if (!baseline && ready() && !halted && !followed(chat.group, m) && (everywhere || listed(m))) queue.push(item(m, ctx, true));
    });
    if (!document.hidden && epoch === baselineEpoch) skip = false;
    // A message picked by hand stays first.
    arrange();
    if (current) prefetch();
    // Playback only updates its channel and message controls.
    queueMicrotask(drain);
    void reload().catch(failure => error(failure.message, "load"));
  }
  // Hands each message to `take` once, when it first becomes readable in scope: a briefing only
  // with its answer. `seen` remembers what was readable already.
  function readable(messages, seen, take, scoped = scopedMessage) {
    const answers = new Map(messages.filter(m => m.kind === "reply" && eligible(m)).map(m =>
      [answerKey(m), { message: m, seen: seen.get(m.id) === true }]));
    for (const m of messages) {
      const briefing = m.kind === "instruction" && answerKey(m);
      const answer = briefing ? answers.get(briefing) : null;
      const can = eligible(m) && (!briefing || Boolean(answer && scoped(answer.message)));
      const was = seen.get(m.id) === true;
      if (can && scoped(m) && !was && !answer?.seen) take(m);
      seen.set(m.id, was || can);
    }
  }
  const contextOf = chat => ({ c: chat, desks: new Map([...(chat.desks || []), ...(chat.optimized?.desks || [])].map(d => [d.key, d])),
    rooms: new Map((chat.rooms || []).map(r => [r.id, r])) });
  // Listening to a workspace or one meeting, picked in the menu bar panel: its latest message
  // plays at once, then whatever it says next, open in the office or not and hidden or not.
  const followed = (g, m) => Boolean(follow && g === follow.group && (!follow.room || m.conversation === follow.room));
  async function loadFollowed(session) {
    const path = `/app/chats/${encodeURIComponent(session.group)}${session.room ? `/focus/${encodeURIComponent(session.room)}` : ""}`;
    const response = await fetch(path, { cache: "no-store" });
    if (!response.ok) throw Object.assign(new Error(L("无法读取要收听的对话", "Failed to load the conversation to listen to")), { status: response.status });
    return response.json();
  }
  const followTitle = (chat, session) => (session.room && (chat.rooms?.find?.(r => r.id === session.room)?.title
    || chat.conversations?.find?.(c => c.id === session.room)?.title)) || AN.chatName?.(chat) || session.group;
  async function listen(target) {
    if (!target?.group || !ready()) return;
    // Asking to listen is asking to speak again after playback stopped on errors.
    halted = false; failures = 0;
    unlisten(false);
    const session = follow = { group: target.group, room: target.room || null, title: "", seen: new Map(), timer: null };
    const picked = picks;
    changed();
    let chat;
    try { chat = await loadFollowed(session); } catch (failure) { if (follow === session) { error(failure.message, "read"); unlisten(); } return; }
    if (follow !== session) return;
    session.title = followTitle(chat, session);
    const ctx = contextOf(chat), list = [];
    // The first is picked by hand, so the read scope does not hide it; what follows keeps to it.
    readable(chat.messages, session.seen, m => { if (followed(chat.group, m)) list.push(item(m, ctx)); }, () => true);
    const latest = beforeAnswers(list.sort((a, b) => a.time - b.time)).at(-1);
    if (latest) {
      latest.followed = true;
      const key = latest.kind === "reply" && answerKey(latest);
      latest.briefings = key ? list.filter(job => job.kind === "instruction" && answerKey(job) === key).map(job => job.id) : [];
    }
    // A message picked in the office meanwhile keeps playing; the latest one waits behind it.
    if (latest && picks === picked) playNow(latest);
    else { if (latest) { queue.push(latest); arrange(); } changed(); void drain(); }
    session.timer = setTimeout(() => void heed(session), FOLLOW_EVERY);
  }
  async function heed(session) {
    let chat = null;
    try { chat = await loadFollowed(session); } catch (failure) {
      // The workspace or meeting is gone: nothing more will be said there.
      if (follow === session && failure.status === 404) { unlisten(); return; }
    }
    if (follow !== session) return;
    if (chat) {
      const ctx = contextOf(chat);
      const known = new Set([current?.id, ...queue.map(job => job.id)]);
      readable(chat.messages, session.seen, m => {
        if (followed(chat.group, m) && ready() && !halted && !known.has(m.id)) queue.push(Object.assign(item(m, ctx, true), { followed: true }));
      });
      arrange();
      if (current) prefetch();
      changed();
      void drain();
    }
    session.timer = setTimeout(() => void heed(session), FOLLOW_EVERY);
  }
  // Stops listening and what listening queued; anything else keeps playing.
  function unlisten(notify = true) {
    if (!follow) return;
    clearTimeout(follow.timer);
    follow = null;
    // Office polls also observe messages handed to the follower. Removing an unplayed clip
    // must release its read markers there, including a latest reply's omitted briefing.
    const pending = [...queue, ...(current?.followed && !current.audio ? [current] : [])];
    for (const job of pending) if (job.followed && job.group === group) {
      observed.delete(job.id);
      for (const id of job.briefings || []) observed.delete(id);
    }
    queue = queue.filter(job => !job.followed || (job.controller.abort(), false));
    if (current?.followed) interrupt();
    if (!current && !queue.length) paused = false;
    if (notify) { changed(); void drain(); }
  }
  // The messages of one conversation a chained read follows, in the order they are shown.
  function playable(chat, target) {
    const ctx = contextOf(chat);
    // A briefing waits for its answer, as in auto read.
    const answered = new Set(chat.messages.filter(m => m.kind === "reply" && eligible(m) && scopedMessage(m)).map(answerKey).filter(Boolean));
    const shown = target.shown && new Set(target.shown(chat).map(m => m.id));
    const list = chat.messages.filter(m => eligible(m) && (m.kind !== "instruction" || !answerKey(m) || answered.has(answerKey(m)))
      && (shown ? shown.has(m.id) : m.conversation === target.id)).map(m => item(m, ctx));
    list.sort((a, b) => a.time - b.time);
    return beforeAnswers(list);
  }
  // Reads the clicked message at once, then, with chaining on, the rest of its conversation after it.
  async function readFrom(m, chosen) {
    if (playlistScope) dropPlaylist();
    observed.set(chosen.id, true);
    playNow(queue.find(job => job.id === chosen.id) || chosen);
    const picked = picks;
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
      const list = playable(chat, target);
      const start = list.findIndex(job => job.id === chosen.id);
      // A briefing still waiting for its answer is not in the list; the chain starts after it.
      const after = start < 0 ? list.filter(job => job.time > chosen.time) : list.slice(start + 1);
      const rest = scopedRead(after).filter(job => job.id !== current?.id)
        .map(job => queue.find(other => other.id === job.id) || job);
      const ids = new Set(rest.map(job => job.id));
      queue = queue.filter(job => !ids.has(job.id));
      for (const job of rest) {
        observed.set(job.id, true);
        Object.assign(job, { chain: picked, automatic: false });
      }
      queue.push(...rest);
      arrange();
      if (shownError?.source === "read") clearError();
      if (current) prefetch();
    } catch (failure) {
      if (playlistFetch === load) error(failure.message, "read");
    } finally {
      if (playlistFetch === load) {
        playlistFetch = null;
        if (!queue.some(job => !job.automatic) && !current?.chosen) playlistScope = null;
        changed(); void drain();
      }
    }
  }
  // Opening another view or closing the drawer leaves playback alone, a chained read included.
  function controls(visible, room = null, ctx = null, desk = null, shown = null) {
    roomView = room;
    privateView = desk;
    deskShown = shown;
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
    for (const d of desks) people.push({ key: d.key, provider: d.provider, model: d.display_model || d.model, role: d.role, label: AN.deskName(d) });
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
    actions.hidden = !channelVisible || !config?.enabled;
    actions.dataset.live = String(Boolean(here?.speaking));
    publish();
  }
  // What the top bar control draws, here and in the menu bar panel: the clip playing first, then
  // what is queued.
  function snapshot() {
    const jobs = [...(current ? [current] : []), ...queue];
    return {
      enabled: Boolean(config?.enabled), ready: ready(), halted, saving,
      auto_read: config?.auto_read === true, continue_read: config?.continue_read === true,
      state: !jobs.length ? "idle" : paused ? "paused" : current?.speaking ? "speaking" : "preparing",
      playing: Boolean(current), paused, error: shownError?.message || null, follow: follow && { group: follow.group, room: follow.room, title: follow.title },
      jobs: jobs.map(job => ({
        id: job.id, group: job.group, place: job.place, album: job.album, provider: job.provider, model: job.model,
        who: job.boss ? "boss" : job.role || job.name || job.label,
        excerpt: job.excerpt,
        state: job !== current ? "queued" : paused ? "paused" : job.speaking ? "speaking" : "preparing",
      })),
    };
  }
  function publish() {
    const snap = snapshot();
    menu.render(snap);
    // The panel needs only the album playing; queue changes alone are not sent.
    const text = JSON.stringify({ ...snap, jobs: snap.jobs.slice(0, 1) });
    // Before its settings load the office has nothing to tell; a waiting click needs them.
    if (leaving || !config || text === published) return;
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
    const path = `/chat/${encodeURIComponent(job.group)}${job.album?.link ? `#${encodeURIComponent(job.album.link)}` : ""}`;
    // The office's own navigation (chat-state.js) opens it in place; elsewhere, by address.
    if (typeof go === "function") go(path);
    else location.href = AN.href ? AN.href(path) : path;
  }
  // A click on the top bar control, here or in the menu bar panel.
  function command(kind, value) {
    if (kind === "toggle") void (paused ? resume() : pause());
    else if (kind === "flip" && ["auto_read", "continue_read"].includes(value)) void flip(value);
    else if (kind === "open") openAlbum(value);
    else if (kind === "listen") void listen(value);
    else if (kind === "unlisten") unlisten();
    else if (kind === "play") {
      if (current?.id === value) return command("toggle");
      const job = queue.find(other => other.id === value);
      if (job && ready() && !halted) playNow(job);
    }
  }
  window.addEventListener("agentnave:voice-command", ({ detail }) => command(detail?.kind, detail?.value));
  function decorate(rec, m, ctx) {
    if (!rec.read) {
      rec.readLabel = h("span", {}, L("读摘要", "Read summary"));
      rec.read = h("button.voice-read", { type: "button" }, h("span.voice-waves", {}, h("i"), h("i"), h("i")), rec.readLabel);
      rec.read.addEventListener("click", event => {
        event?.stopPropagation();
        const same = current && current.id === rec.voiceSource?.[0].id;
        if (same) { if (paused) void resume(); else pause(); return; }
        if (!ready() || halted) return;
        // The chosen message goes first; what auto read queued still follows it.
        // Built at the click: rendering every message must not summarize each one.
        const [message, context] = rec.voiceSource;
        void readFrom(m, item(message, context));
      });
      rec.bub.prepend(rec.read); // Keep the action inside the chat bubble, above its text.
    }
    rec.read.hidden = !eligible(m) || !config?.enabled;
    rec.read.disabled = !ready() || halted;
    if (eligible(m)) rec.voiceSource = [m, ctx];
    const reading = current?.id === m.id;
    const idleLabel = L("读摘要", "Read summary");
    rec.readLabel.textContent = reading ? paused ? L("继续", "Resume") : L("暂停", "Pause") : idleLabel;
    rec.read.title = reading ? paused ? L("继续朗读", "Resume reading") : L("暂停朗读", "Pause reading") : ready() ? idleLabel : L("请在设置 → 语音中开启语音播报并配置音色引擎", "Enable voice readout and configure an engine in Settings → Voice");
    rec.read.setAttribute("aria-label", rec.read.title);
    rec.read.setAttribute("aria-pressed", String(Boolean(reading)));
  }
  function switches() {
    const everywhere = config.auto_read === true;
    // Reading the whole workspace already reads this list.
    listToggle.setAttribute("aria-pressed", String(everywhere || listAuto));
    listToggle.disabled = !ready() || everywhere;
    publish();
  }
  async function flip(field) {
    if (!config) return;
    const on = config[field] === true;
    saving = true; listToggle.disabled = true;
    publish();
    if (field === "auto_read") {
      if (on) dropAutomatic();
      // A poll finishing while this saves must not queue under the old setting.
      skip = true; baselineEpoch++;
    } else if (on) dropPlaylist(true);
    try {
      const response = await fetch("/app/voice", { method: "POST", headers: { "X-AgentNave-Action": "1", "Content-Type": "application/json" }, body: JSON.stringify({ ...config, [field]: !on }) });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || L("无法保存语音播报设置", "Failed to save voice settings"));
      config = data; failures = 0; halted = false; clearError();
      if (field === "auto_read") { skip = true; baselineEpoch++; }
      changed();
    } catch (failure) { error(failure.message, "save"); }
    finally { saving = false; switches(); }
  }
  listToggle.addEventListener("click", () => {
    listAuto = !listAuto;
    try { localStorage.setItem(LIST_AUTO_KEY, listAuto ? "1" : "0"); } catch { /* kept for this window only */ }
    if (!listAuto && config?.auto_read !== true) dropAutomatic();
    switches();
  });
  // A hidden window keeps speaking; what arrives meanwhile is not read when it shows again.
  document.addEventListener("visibilitychange", () => { skip = true; baselineEpoch++; });
  // Leaving the page publishes nothing more: the tray has already cleared what it showed.
  window.addEventListener("pagehide", () => { leaving = true; stop(false); });
  AN.voice = { ingest, decorate, pollToken: () => baselineEpoch, controls, stop, beforeAnswers };
  void reload().catch(failure => error(failure.message, "load"));
}
