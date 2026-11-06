"use strict";
// Shared player presentation; only the office owns playback and the snapshot's decisions.
(function () {
  const { h, L = zh => zh } = AN;
  AN.voiceState = function (snap) {
    const unconfigured = snap.enabled && !snap.ready && snap.error_source !== "load";
    const blocking = Boolean(snap.halted || ["load", "configuration", "read"].includes(snap.error_source));
    const retryable = Boolean(snap.retryable && !unconfigured && snap.error_source !== "configuration");
    const settings = unconfigured || ["load", "configuration"].includes(snap.error_source);
    const status = unconfigured ? L("语音未配置", "Voice not configured")
      : snap.backlog ? L("消息积压，已暂停收听", "Message backlog, listening paused")
        : snap.halted ? L(`播放已停下：${snap.error}`, `Playback stopped: ${snap.error}`)
          : blocking && snap.error ? L(`播放已停下：${snap.error}`, `Playback stopped: ${snap.error}`)
          : snap.error_source === "playback" ? L("本条朗读失败，已跳过", "Reading failed, message skipped")
            : snap.error ? snap.error : snap.paused ? L("已暂停", "Paused")
              : snap.sounding ? L("正在播放", "Playing") : snap.state === "preparing" ? L("准备中", "Preparing")
                : snap.follow ? L("等待新消息", "Waiting for new messages") : L("暂无播放", "Nothing playing");
    const bar = unconfigured ? L("未配置", "Not configured") : blocking ? L("出错", "Error")
      : snap.paused || snap.backlog ? L("已暂停", "Paused") : snap.sounding ? L("语音", "Voice")
        : snap.state === "preparing" ? L("准备中", "Preparing") : snap.follow ? L("收听中", "Listening") : L("语音", "Voice");
    // The controls already say playing or paused; the line is for what they cannot show.
    const quiet = Boolean((snap.jobs?.length || snap.recent) && !unconfigured && !snap.backlog && !snap.halted && !snap.error);
    return { ...snap, status, quiet, bar_label: bar, blocking, retryable,
      primary: { command: settings ? "settings" : retryable ? "retry" : snap.recent ? "previous" : "toggle",
        icon: settings ? "settings" : retryable ? "retry" : snap.paused || snap.recent ? "play" : "pause",
        label: settings ? L("语音设置", "Voice settings") : retryable ? L("重试", "Retry") : snap.recent ? L("播放", "Play") : snap.paused ? L("继续", "Resume") : L("暂停", "Pause"),
        disabled: !settings && !retryable && (!snap.ready || !(snap.jobs?.length || snap.follow || snap.recent)) } };
  };

  AN.voiceAlbum = function (act) {
    let snap = null;
    const heads = h("span.voice-cover-heads"), cover = h("div.voice-cover", {}, heads);
    const title = h("button.voice-album-title", { type: "button", title: L("打开会话", "Open conversation") });
    const who = h("button.voice-album-who", { type: "button", title: L("跳到正在播放的消息", "Go to the message playing") });
    const el = h("div.voice-album", { hidden: true }, cover, h("div.voice-album-meta", {}, title, h("span.voice-album-line", {}, who)));
    // What plays, or for a moment after stopping, what played last.
    const shown = () => snap?.jobs?.[0] || snap?.recent;
    title.addEventListener("click", () => { if (shown()) act("open", shown()); });
    who.addEventListener("click", () => { if (shown()) act("locate", shown()); });
    function draw(next) {
      snap = next;
      const top = shown();
      el.hidden = !top;
      if (!top) return;
      const people = top.album?.cover?.length ? top.album.cover : [{ provider: top.provider, model: top.model }];
      const faces = [...new Set(people.map(p => AN.art.character(p.provider, p.model)))];
      const key = JSON.stringify([top.group, top.album?.link, faces]);
      if (el.dataset.key !== key) {
        el.dataset.key = key;
        heads.dataset.count = String(faces.length);
        heads.replaceChildren(...faces.map(face => AN.art.head(face)));
      }
      who.textContent = top.who || "";
      title.textContent = top.album?.title || top.place || L("会话", "Conversation");
      title.disabled = !top.group;
    }
    return { el, draw };
  };

  // Controls are icons, as in a music player; the name is the tooltip and what a reader says.
  const iconButton = (tag, icon, label) => {
    const button = h(tag, { type: "button", title: label, "aria-label": label });
    button.dataset.icon = icon;
    return button;
  };
  AN.voicePlayer = function (act) {
    let snap = null;
    const status = h("p.voice-status", { role: "status" });
    const album = AN.voiceAlbum(act);
    const main = iconButton("button.voice-primary", "pause", L("暂停", "Pause"));
    const previous = iconButton("button.voice-previous", "previous", L("上一条", "Previous"));
    const next = iconButton("button.voice-next", "next", L("下一条", "Next"));
    const stop = iconButton("button.voice-stop-all", "stop", L("停止全部", "Stop all"));
    const chain = iconButton("button.voice-chain", "chain", L("点读后连播", "Continue reading"));
    const controls = h("div.voice-controls", {}, chain, previous, main, next, stop);
    const el = h("div.voice-player", {}, status, album.el, controls);
    main.addEventListener("click", () => { if (!main.disabled) act(snap.primary.command); });
    previous.addEventListener("click", () => { if (!previous.disabled) act("previous"); });
    next.addEventListener("click", () => { if (!next.disabled) act("next"); });
    stop.addEventListener("click", () => { if (!stop.disabled) act("stop_all"); });
    chain.addEventListener("click", () => { if (!chain.disabled) act("flip", "continue_read"); });
    function draw(value) {
      snap = value;
      status.textContent = snap?.status || "";
      status.hidden = !snap?.status || snap.quiet === true;
      status.dataset.error = String(Boolean(snap?.blocking));
      album.draw(snap);
      main.dataset.icon = snap?.primary?.icon || "pause";
      main.title = snap?.primary?.label || L("暂停", "Pause");
      main.setAttribute("aria-label", main.title);
      main.disabled = snap?.primary?.disabled !== false;
      previous.disabled = !snap?.can_previous;
      next.disabled = !snap?.can_next;
      stop.disabled = !snap?.can_stop;
      chain.setAttribute("aria-pressed", String(snap?.continue_read === true));
      // Only a snapshot from the office knows the setting, and only the office can change it.
      chain.disabled = !snap?.ready || typeof snap.continue_read !== "boolean";
      controls.hidden = !snap?.enabled && !snap?.can_stop;
    }
    return { el, draw };
  };

  AN.voiceMenu = function (end, act) {
    let snap = null;
    const playlist = h("ol.voice-queue", { "aria-label": L("播放列表", "Playlist") });
    const player = AN.voicePlayer((kind, value) => { if (["open", "locate", "settings"].includes(kind)) open(false); act(kind, value); });
    const upNext = h("h3.voice-next-title", { hidden: true }, L("接下来", "Up Next"));
    const menu = h("div.voice-menu", { id: "voice-menu", hidden: true }, player.el, upNext, playlist);
    const label = h("span.bar-voice-label", {}, L("语音", "Voice"));
    const button = h("button.bar-voice", { type: "button", "aria-haspopup": "true", "aria-expanded": "false", "aria-controls": "voice-menu" },
      h("span.voice-waves", { "aria-hidden": "true" }, h("i"), h("i"), h("i")), label);
    const wrap = h("div.bar-voice-wrap", { hidden: true }, button, menu);
    end?.prepend(wrap);
    function open(show) {
      menu.hidden = !show;
      button.setAttribute("aria-expanded", String(show));
      if (!show) return;
      drawMenu();
      if (!menu.getBoundingClientRect) return;
      menu.style.translate = "";
      const over = menu.getBoundingClientRect().right - (innerWidth - 8);
      if (over > 0) menu.style.translate = `${-over}px 0`;
    }
    button.addEventListener("click", () => open(menu.hidden));
    menu.addEventListener("mousedown", event => event.stopPropagation());
    menu.addEventListener("dblclick", event => event.stopPropagation());
    document.addEventListener("pointerdown", event => { if (!menu.hidden && !wrap.contains(event.target)) open(false); }, true);
    document.addEventListener("keydown", event => { if (event.key === "Escape" && !menu.hidden) { open(false); button.focus(); } });
    const rows = new Map();
    function drawMenu() {
      const jobs = snap?.jobs || [], top = jobs[0];
      player.draw(snap);
      playlist.hidden = !jobs.length;
      upNext.hidden = jobs.length < 2;
      const keep = new Set(jobs.map(job => job.id));
      for (const id of rows.keys()) if (!keep.has(id)) rows.delete(id);
      AN.reconcile(playlist, jobs.map(job => {
        let row = rows.get(job.id);
        if (!row) {
          const play = h("button.voice-queue-play", { type: "button", "aria-label": `${job.who}：${job.excerpt}` });
          const from = h("span.voice-queue-album"), state = h("span.voice-queue-status");
          row = h("li.voice-queue-row", {}, play, h("span.voice-queue-head", {}, AN.art.head(AN.art.character(job.provider, job.model))),
            h("span.voice-queue-text", {}, h("span.voice-queue-line", {}, job.excerpt),
              h("span.voice-queue-who", {}, h("span", {}, job.who), from)), state);
          play.addEventListener("click", () => { if (!play.disabled) act("play", job.id); });
          Object.assign(row, { play, from, state }); rows.set(job.id, row);
        }
        const other = top && (job.group !== top.group || job.album?.link !== top.album?.link);
        row.from.textContent = other ? job.album?.title || job.place || "" : "";
        row.dataset.state = job.state;
        row.play.disabled = job.state !== "queued";
        row.play.setAttribute("aria-current", String(job.state !== "queued"));
        row.state.textContent = job.state === "queued" ? "" : job.state === "paused" ? L("已暂停", "Paused") : job.state === "speaking" ? L("正在播放", "Playing") : L("准备中", "Preparing");
        return row;
      }));
    }
    function render(next) {
      snap = next;
      wrap.hidden = !snap?.enabled;
      if (wrap.hidden) open(false);
      wrap.dataset.state = snap?.state || "idle";
      wrap.dataset.sounding = String(Boolean(snap?.sounding));
      label.textContent = snap?.bar_label || L("语音", "Voice");
      if (!menu.hidden) drawMenu();
    }
    return { render, open };
  };
})();
