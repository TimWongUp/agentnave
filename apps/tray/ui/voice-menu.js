"use strict";
// The voice player's top bar control in the office: a button showing what plays and, under it, the
// album playing, its songs and the switches that apply everywhere. It draws a snapshot of the
// office's player and hands each click to `act`; only the office plays.
(function () {
  const { h, L = zh => zh } = AN;
  const switchButton = label => h("button.voice-toggle", { type: "button", "aria-label": label, "aria-pressed": "false" }, h("span.voice-track"), label);

  // The album playing: a cover of who takes part, which plays or pauses at its centre, and its
  // title, which opens the conversation. The menu heads its songs with it; the menu bar panel
  // shows it alone.
  AN.voiceAlbum = function (act) {
    let snap = null;
    const coverHeads = h("span.voice-cover-heads");
    const coverPlay = h("button.voice-cover-play", { type: "button" });
    const cover = h("div.voice-cover", {}, coverHeads, coverPlay);
    const albumTitle = h("button.voice-album-title", { type: "button", title: L("打开会话", "Open conversation") }), albumPlace = h("span.voice-album-place");
    const albumHead = h("div.voice-album", { hidden: true }, cover, h("div.voice-album-meta", {}, albumTitle, albumPlace));
    coverPlay.addEventListener("click", () => act("toggle"));
    albumTitle.addEventListener("click", () => { const top = snap?.jobs[0]; if (top) act("open", top); });
    function draw(next) {
      snap = next;
      const top = snap?.jobs?.[0];
      albumHead.hidden = !top;
      if (!top) return;
      const key = JSON.stringify([top.group, top.album?.link]);
      if (albumHead.dataset.key !== key) {
        albumHead.dataset.key = key;
        // Each face once, though the boss and a seat, or two seats, may share one.
        const people = top.album?.cover?.length ? top.album.cover : [{ provider: top.provider, model: top.model }];
        const faces = [...new Set(people.map(p => AN.art.character(p.provider, p.model)))];
        coverHeads.dataset.count = String(faces.length);
        coverHeads.replaceChildren(...faces.map(face => AN.art.head(face)));
        albumTitle.textContent = top.album?.title || top.place || "";
        albumPlace.textContent = top.album?.title ? top.place || "" : "";
      }
      const playing = snap.playing && !snap.paused;
      const label = !snap.playing ? L("播放", "Play") : snap.paused ? L("继续", "Resume") : L("暂停", "Pause");
      coverPlay.textContent = playing ? "❚❚" : "▶";
      coverPlay.title = label;
      coverPlay.setAttribute("aria-label", label);
      coverPlay.disabled = !snap.ready || snap.halted || (!snap.playing && !snap.paused);
      // While it plays, pausing shows only under the pointer; paused, the play button stays.
      cover.dataset.playing = String(playing);
      albumTitle.disabled = !top.group;
    }
    return { el: albumHead, draw };
  };

  // What is being listened to, wherever its own button went, with the way to stop.
  AN.voiceFollow = function (act) {
    const title = h("span.voice-follow-title");
    const stop = h("button.voice-follow-stop", { type: "button", title: L("停止收听", "Stop listening"), "aria-label": L("停止收听", "Stop listening") }, "■");
    const el = h("div.voice-follow", { hidden: true }, h("span.voice-follow-label", {}, L("收听中", "Listening")), title, stop);
    stop.addEventListener("click", () => act("unlisten"));
    function draw(snap) {
      el.hidden = !snap?.follow;
      if (snap?.follow) title.textContent = snap.follow.title || "";
    }
    return { el, draw };
  };

  AN.voiceMenu = function (end, act) {
    let snap = null;
    const autoRead = switchButton(L("自动读当前工作区", "Auto-read open workspace")), chain = switchButton(L("点读后连播", "Continue reading"));
    // What is playing and what comes next, like a music player's queue; a row plays at a click.
    const playlist = h("ol.voice-queue", { "aria-label": L("播放列表", "Playlist") });
    // The album playing, fixed above its songs.
    const album = AN.voiceAlbum((kind, value) => { if (kind === "open") open(false); act(kind, value); });
    const follow = AN.voiceFollow(act);
    const menu = h("div.voice-menu", { id: "voice-menu", hidden: true }, follow.el, album.el, playlist, autoRead, chain);
    const button = h("button.bar-voice", { type: "button", "aria-haspopup": "true", "aria-expanded": "false", "aria-controls": "voice-menu" },
      h("span.voice-waves", {}, h("i"), h("i"), h("i")), L("语音", "Voice"));
    const wrap = h("div.bar-voice-wrap", { hidden: true }, button, menu);
    end?.prepend(wrap);
    function open(show) {
      menu.hidden = !show;
      button.setAttribute("aria-expanded", String(show));
      if (!show) return;
      drawMenu();
      // Centred under the button, but never past the window's right edge.
      if (!menu.getBoundingClientRect) return;
      menu.style.translate = "";
      const over = menu.getBoundingClientRect().right - (innerWidth - 8);
      if (over > 0) menu.style.translate = `${-over}px 0`;
    }
    button.addEventListener("click", () => open(menu.hidden));
    // The bar drags the window; a press inside the open menu must not.
    menu.addEventListener("mousedown", event => event.stopPropagation());
    menu.addEventListener("dblclick", event => event.stopPropagation());
    // Captured first: the office stops some clicks before they reach the document.
    document.addEventListener("pointerdown", event => { if (!menu.hidden && !wrap.contains(event.target)) open(false); }, true);
    document.addEventListener("keydown", event => { if (event.key === "Escape" && !menu.hidden) { open(false); button.focus(); } });
    autoRead.addEventListener("click", () => act("flip", "auto_read"));
    chain.addEventListener("click", () => act("flip", "continue_read"));

    const rows = new Map();
    function drawMenu() {
      const jobs = snap?.jobs || [];
      const top = jobs[0];
      follow.draw(snap);
      album.draw(snap);
      playlist.hidden = !jobs.length;
      const keep = new Set(jobs.map(job => job.id));
      for (const id of rows.keys()) if (!keep.has(id)) rows.delete(id);
      AN.reconcile(playlist, jobs.map(job => {
        let row = rows.get(job.id);
        if (!row) {
          // A song: the message is its title, the speaker its artist; the row plays it.
          const play = h("button.voice-queue-play", { type: "button", "aria-label": `${job.who}：${job.excerpt}` });
          const from = h("span.voice-queue-album");
          row = h("li.voice-queue-row", {}, play,
            h("span.voice-queue-head", {}, AN.art.head(AN.art.character(job.provider, job.model))),
            h("span.voice-queue-text", {},
              h("span.voice-queue-line", {}, job.excerpt),
              h("span.voice-queue-who", {}, h("span", {}, job.who), from)),
            h("span.voice-waves", { "aria-hidden": "true" }, h("i"), h("i"), h("i")));
          play.addEventListener("click", () => act("play", job.id));
          row.play = play;
          row.from = from;
          rows.set(job.id, row);
        }
        // A song from another album names it.
        const other = top && (job.group !== top.group || job.album?.link !== top.album?.link);
        row.from.textContent = other ? job.album?.title || job.place || "" : "";
        row.dataset.state = job.state;
        row.play.setAttribute("aria-current", String(job.state !== "queued"));
        return row;
      }));
      autoRead.setAttribute("aria-pressed", String(snap?.auto_read === true));
      chain.setAttribute("aria-pressed", String(snap?.continue_read === true));
      autoRead.disabled = chain.disabled = !snap?.ready || snap.saving === true;
    }
    function render(next) {
      snap = next;
      wrap.hidden = !snap?.enabled;
      if (wrap.hidden) open(false);
      wrap.dataset.state = snap?.state || "idle";
      if (!menu.hidden) drawMenu();
    }
    return { render, open };
  };
})();
