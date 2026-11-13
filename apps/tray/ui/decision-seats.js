"use strict";
{
  // Settings -> 编排 -> 决策模型 (experiment): the user's own seat catalog, which stands in for the
  // fixed seats on that mode's page, and the decision model's key. The catalog is a draft like the
  // rest of the page and saves from its one bar; the key is stored at once, as the voice keys are, and
  // never read back: the page only learns whether one is set. Which bosses use the catalog is their
  // mode, saved with the orchestration.

  const S = AN.settings ||= {};
  const { $, el, sameJson } = S;
  const L = AN.L || ((zh) => zh);

  const CUSTOM = "custom";
  const IMAGE = "image";
  const DIFFICULTIES = [["easy", L("简单", "Easy")], ["medium", L("中等", "Medium")], ["hard", L("困难", "Hard")]];
  const ACCESS = [["read", L("只读", "Read-only")], ["write", L("可写", "Writable")]];
  const accessName = (access) => ACCESS.find(([value]) => value === access)[1];
  const MAX_SEATS = 255;
  const ROLE_MAX = 40;

  let saved = null; // { document | null, error, key_set, key_error } from GET /app/decision-seats.
  let draft = null; // The cards as edited; null while they are what is saved.
  let prefilled = false; // The first catalog was already made in this draft.
  let keyDraft = "";
  let keyBusy = false;
  let keyMessage = null; // { text, error }
  let cardCount = 0;
  let rowCount = 0;

  // The file is a flat list, one seat per difficulty; the page shows one card for each kind and
  // specialty: { cid, kind, role, access, seats: { easy | medium | hard: { id, provider, model, effort? } } }.
  // A group of the role catalog is one card whatever its role says; custom cards go by role. All the
  // image seats are the rows of the one image card: { cid, kind: "image", rows: [{ rid, role, seat | null }] }.
  const catalogEntry = (kind) => S.roleCatalog().find((entry) => entry.group === kind);
  const levelsOf = () => DIFFICULTIES.map(([difficulty]) => difficulty);
  const seatsOfCard = (card) => (card.kind === IMAGE ? card.rows.map((row) => row.seat).filter(Boolean) : Object.values(card.seats));
  const bareSeat = ({ id, provider, model, effort }) => ({ id, provider, model, ...(effort ? { effort } : {}) });
  const accessOf = (card) => catalogEntry(card.kind)?.access || (card.kind === IMAGE ? "write" : card.access);

  function toCards(seats) {
    const cards = [];
    for (const seat of seats) {
      if (seat.kind === IMAGE) {
        let card = cards.find((one) => one.kind === IMAGE);
        if (!card) cards.push(card = { cid: `c${++cardCount}`, kind: IMAGE, role: "", access: "write", seats: {}, rows: [] });
        card.rows.push({ rid: `r${++rowCount}`, role: seat.role, seat: bareSeat(seat) });
        continue;
      }
      const fixed = seat.kind !== CUSTOM;
      let card = cards.find((one) => one.kind === seat.kind && (fixed || one.role === seat.role));
      if (!card) {
        card = { cid: `c${++cardCount}`, kind: seat.kind, role: seat.role, access: seat.access, description: seat.description, seats: {} };
        cards.push(card);
      }
      card.seats[seat.difficulty] = bareSeat(seat);
    }
    return cards;
  }

  function toSeats(cards) {
    return cards.flatMap((card) => {
      if (card.kind === IMAGE) {
        return card.rows.filter((row) => row.seat).map(({ role, seat }) => ({
          id: seat.id, kind: IMAGE, role: role.trim(), description: role.trim(), provider: seat.provider, model: seat.model,
          ...(seat.effort ? { effort: seat.effort } : {}), access: "write",
        }));
      }
      return levelsOf().filter((level) => card.seats[level]).map((level) => {
        const { id, provider, model, effort } = card.seats[level];
        const role = card.role.trim();
        const group = catalogEntry(card.kind);
        return {
          id, kind: card.kind, role, description: group?.scope_zh || (card.kind === CUSTOM ? role : card.description || role),
          provider, model, ...(effort ? { effort } : {}), access: accessOf(card), difficulty: level,
        };
      });
    });
  }

  // The saved file in the order and shape the page writes, to compare with the cards.
  const canon = (seats) => [...seats].sort((a, b) => (a.id < b.id ? -1 : 1)).map((seat) => ({
    id: seat.id, kind: seat.kind, role: seat.role, description: seat.description, provider: seat.provider, model: seat.model,
    ...(seat.effort ? { effort: seat.effort } : {}), access: seat.access, ...(seat.difficulty ? { difficulty: seat.difficulty } : {}),
  }));
  // The saved cards are made once per loaded file, so a card keeps its handle between draws.
  let base = { of: null, cards: [] };
  function cards() {
    if (draft) return draft;
    if (base.of !== saved) base = { of: saved, cards: toCards(saved?.document?.seats || []) };
    return base.cards;
  }
  const fileDocument = () => ({ version: 1, decision_model: saved?.document?.decision_model || "gemini-decision-1", seats: toSeats(cards()) });
  const changed = () => Boolean(draft) && Boolean(saved) && !saved.error
    && !sameJson(canon(toSeats(draft)), canon(saved.document?.seats || []));
  const locked = () => !saved || Boolean(saved.error) || !S.orchestration || Boolean(S.orchestration.error);

  const takenIds = () => new Set(cards().flatMap((card) => seatsOfCard(card).map((seat) => seat.id)));
  function freshId(taken) {
    let id;
    do id = `s-${Math.random().toString(16).slice(2, 8).padEnd(6, "0")}`;
    while (taken.has(id));
    return id;
  }
  const blankRow = (fields = {}) => ({ rid: `r${++rowCount}`, role: "", seat: null, ...fields });
  // The image card starts with one empty row.
  const newCard = (kind, fields = {}) => ({
    cid: `c${++cardCount}`, kind, role: catalogEntry(kind)?.name_zh || "", access: "write", seats: {},
    ...(kind === IMAGE ? { rows: [blankRow()] } : {}), ...fields,
  });

  // The fixed seats the user has bound make one card per group, easy and hard filled in, medium left
  // empty, and the fixed image seat a card of its own.
  function prefill() {
    const doc = S.orchestrationShown();
    const taken = new Set();
    const bound = (seat) => {
      if (!seat?.provider || !seat?.model) return null;
      const id = freshId(taken);
      taken.add(id);
      return { id, provider: seat.provider, model: seat.model, ...(seat.effort ? { effort: seat.effort } : {}) };
    };
    const made = S.roleCatalog().filter((entry) => S.liveSeats(doc).some(({ role }) => role === entry.group)).map((entry) => newCard(entry.group, {
      seats: Object.fromEntries(["easy", "hard"].map((difficulty) => [difficulty, bound(S.orchestrationSeat(doc, entry.group, difficulty))]).filter(([, seat]) => seat)),
    })).filter((card) => Object.keys(card.seats).length);
    const image = bound(doc.image_generation);
    if (image) made.push(newCard(IMAGE, { rows: [blankRow({ role: L("通用生图", "General image generation"), seat: image })] }));
    return made;
  }

  // The catalog stands in the orchestration card, so it is drawn with it.
  const render = () => S.renderOrchestration();

  function edit(next, rerender = true) {
    draft = next;
    if (rerender) render();
    S.advancedEdited();
  }

  // A boss put into the decision mode when no catalog was ever saved: the first one is made from the
  // fixed seats, once. The caller draws the page next.
  function entered() {
    if (locked() || saved.document || prefilled || cards().length) return;
    prefilled = true;
    edit(prefill(), false);
  }

  const editCard = (cid, patch, rerender = true) => edit(cards().map((card) => (card.cid === cid ? { ...card, ...patch } : card)), rerender);
  const removeCard = (cid) => edit(cards().filter((card) => card.cid !== cid));
  const editRow = (cid, rid, patch, rerender = true) => editCard(cid,
    { rows: cards().find((card) => card.cid === cid).rows.map((row) => (row.rid === rid ? { ...row, ...patch } : row)) }, rerender);
  function setRowSeat(cid, rid, next) {
    const row = cards().find((card) => card.cid === cid).rows.find((one) => one.rid === rid);
    editRow(cid, rid, { seat: !next?.provider || !next?.model ? null : { ...bareSeat({ id: row.seat?.id || freshId(takenIds()), ...next }) } });
  }
  const addRow = (cid) => editCard(cid, { rows: [...cards().find((card) => card.cid === cid).rows, blankRow()] });
  const removeRow = (cid, rid) => editCard(cid, { rows: cards().find((card) => card.cid === cid).rows.filter((row) => row.rid !== rid) });
  function setSeat(cid, level, next) {
    const card = cards().find((one) => one.cid === cid);
    const seats = { ...card.seats };
    if (!next?.provider || !next?.model) delete seats[level];
    else seats[level] = { id: seats[level]?.id || freshId(takenIds()), provider: next.provider, model: next.model, ...(next.effort ? { effort: next.effort } : {}) };
    editCard(cid, { seats });
  }
  // `what` is a group of the role catalog, "custom" or "image".
  function addCard(what) {
    if (locked() || (what !== CUSTOM && what !== IMAGE && !catalogEntry(what)) || cards().some((card) => card.kind === what && what !== CUSTOM)) return;
    edit([...cards(), newCard(what)]);
  }

  const IMAGE_NAME = () => L("生图工位", "Image seats");
  const cardName = (card) => {
    if (card.kind === IMAGE) return IMAGE_NAME();
    const group = catalogEntry(card.kind);
    return group ? S.roleName(group) : card.role.trim();
  };
  // What the card says the seats are for: the very text the decision model is sent.
  const scopeOf = (card) => {
    const group = catalogEntry(card.kind);
    return group ? L(group.scope_zh, group.scope_en || group.scope_zh) : "";
  };

  // What keeps the image card's rows from being saved, or null.
  function imageProblem(card) {
    if (!card.rows.length) return L("生图工位至少要有一行", "Image seats need at least one row");
    const seen = new Set();
    for (const [index, row] of card.rows.entries()) {
      const n = index + 1;
      const role = row.role.trim();
      if (!role) return L(`生图工位第 ${n} 行缺少专业`, `Image seats row ${n} has no specialty`);
      if (!row.seat) return L(`生图工位第 ${n} 行（${role}）还没选 CLI 和模型`, `Image seats row ${n} (${role}) has no CLI and model yet`);
      if (seen.has(role)) return L(`生图工位第 ${n} 行的专业“${role}”与前面重复`, `Image seats row ${n} repeats the specialty “${role}”`);
      seen.add(role);
    }
    return null;
  }

  // What keeps the draft from being saved, or null.
  function problem() {
    const list = cards();
    if (toSeats(list).length > MAX_SEATS) return L(`工位最多 ${MAX_SEATS} 个`, `At most ${MAX_SEATS} seats`);
    const named = new Set();
    for (const [index, card] of list.entries()) {
      const n = index + 1;
      if (card.kind === IMAGE) {
        const found = imageProblem(card);
        if (found) return found;
        continue;
      }
      const own = card.kind === CUSTOM;
      const title = own ? card.role.trim() : cardName(card);
      if (own && !title) return L(`第 ${n} 张卡片缺少专业`, `Card ${n} has no specialty`);
      if (own && named.has(title)) return L(`有两张自定义卡片的专业都是“${title}”`, `Two custom cards have the specialty “${title}”`);
      named.add(title);
      if (!Object.keys(card.seats).length) return L(`第 ${n} 张卡片（${title}）至少要绑定一档`, `Card ${n} (${title}) needs at least one level set`);
    }
    return null;
  }

  async function post(url, body) {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(body),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok || data.error) throw new Error(data.error || L(`保存失败（${response.status}）`, `Save failed (${response.status})`));
    return data;
  }

  async function load() {
    try {
      const response = await fetch("/app/decision-seats", { cache: "no-store" });
      if (!response.ok) throw new Error(L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
      const loaded = await response.json();
      const keep = changed();
      // A file error while an unsaved draft stands keeps the last good baseline.
      if (!(loaded.error && keep)) saved = loaded;
      if (loaded.error) S.showNotice(L(`${loaded.error}；请先修复或删除这个文件`, `${loaded.error}; fix or delete this file first`), true);
      if (!keep) {
        draft = null;
        prefilled = false;
      }
    } catch (error) {
      S.showNotice(L(`无法读取决策模型推荐工位设置：${error.message}`, `Failed to load decision-model seat settings: ${error.message}`), true);
    }
    render();
    S.renderAdvancedBar();
  }

  // Seats whose CLI and model are new here are listed in 员工 first, as the other seats are.
  async function save() {
    const doc = fileDocument();
    const before = new Map((saved?.document?.seats || []).map((seat) => [seat.id, seat]));
    for (const seat of doc.seats) {
      const old = before.get(seat.id);
      if (!old || old.provider !== seat.provider || old.model !== seat.model) await S.listSeatModel(seat);
    }
    await post("/app/decision-seats", doc);
    saved = { ...saved, document: doc, error: null };
    draft = null;
    render();
  }

  function discard() {
    draft = null;
    prefilled = false;
    keyDraft = "";
  }

  async function storeKey(key) {
    keyBusy = true;
    keyMessage = null;
    render();
    try {
      const data = await post("/app/decision-key", { key });
      saved.key_set = data.key_set;
      keyDraft = "";
      keyMessage = { text: key ? L("密钥已保存", "Key saved") : L("密钥已清除", "Key cleared"), error: false };
    } catch (error) {
      keyMessage = { text: error.message, error: true };
    }
    keyBusy = false;
    render();
  }

  function select(key, label, value, options, onPick) {
    return el("select", { dataset: { focusKey: key }, "aria-label": label, onChange: (e) => onPick(e.target.value) },
      options.map(([v, text]) => el("option", { value: v, selected: v === value }, text)));
  }

  function keyRow() {
    const label = L("Gemini 密钥", "Gemini API key");
    const save = el("button", { type: "button", disabled: keyBusy || !keyDraft.trim(), onClick: () => storeKey(keyDraft) }, L("保存", "Save"));
    const input = el("input", {
      type: "password", autocomplete: "off", spellcheck: "false", maxlength: "4096", "aria-label": label, disabled: keyBusy,
      dataset: { focusKey: "decision/key" },
      placeholder: saved.key_set ? L("已保存，粘贴新密钥可替换", "Saved; paste a new key to replace") : L("粘贴密钥", "Paste API key"),
      onInput: (e) => { keyDraft = e.target.value; save.disabled = keyBusy || !keyDraft.trim(); },
    });
    input.value = keyDraft;
    const clear = el("button", { type: "button", disabled: keyBusy || !saved.key_set, onClick: () => storeKey("") }, L("清除", "Clear"));
    return el("div", { className: "decision-key-row" },
      el("div", { className: "review-role" }, el("strong", {}, label)),
      el("div", { className: "voice-key" }, input, save, clear),
      saved.key_error && el("p", { className: "fine decision-note error" }, saved.key_error),
      keyMessage && el("p", { className: `fine decision-note${keyMessage.error ? " error" : ""}`, role: "status" }, keyMessage.text));
  }

  // One row of a card: the level's name and its CLI, model and effort.
  function levelRow(card, level, label, title) {
    const key = `decision/${card.cid}/${level}`;
    const row = S.seatSelects(key, `${title} · ${label}`, card.seats[level] || null,
      (next) => setSeat(card.cid, level, next), L("未设置", "Not set"));
    return el("div", { className: "review-row" }, el("div", { className: "review-role" }, label), ...row);
  }

  // The tiers a three-tier card has set; no tier means no seat to choose from.
  function tiersSet(card) {
    const n = Object.keys(card.seats).length;
    return el("span", { className: `ds-tiers${n ? "" : " unset"}` },
      n ? L(`已设 ${n} 档`, `${n} ${n === 1 ? "tier" : "tiers"} set`) : L("未设置", "Not set"));
  }

  // One image specialty: its name and its CLI, model and effort on one line, and its delete button.
  function imageRow(card, row, index) {
    const key = `decision/${card.cid}/${row.rid}`;
    const name = row.role.trim() || L(`生图工位第 ${index + 1} 行`, `Image seats row ${index + 1}`);
    const specialty = el("input", {
      type: "text", className: "ds-role", maxlength: String(ROLE_MAX), spellcheck: "false", value: row.role,
      "aria-label": L("专业", "Specialty"), placeholder: L("专业", "Specialty"), dataset: { focusKey: `${key}/role` },
      onInput: (e) => editRow(card.cid, row.rid, { role: e.target.value }, false),
    });
    const selects = S.seatSelects(key, name, row.seat, (next) => setRowSeat(card.cid, row.rid, next), L("未设置", "Not set"));
    const remove = el("button", { type: "button", className: "ds-remove", "aria-label": L(`删除${name}`, `Delete ${name}`), onClick: () => removeRow(card.cid, row.rid) }, L("删除", "Delete"));
    return el("div", { className: "ds-image-row" }, specialty, ...selects, remove);
  }

  // A group of the role catalog is named and fixed and says what it is for; a custom card asks for its
  // specialty; the image card has a row for each specialty.
  function seatCard(card, index) {
    const key = `decision/${card.cid}`;
    const own = card.kind === CUSTOM;
    const title = cardName(card) || L(`卡片 ${index + 1}`, `Card ${index + 1}`);
    const name = el(own ? "input" : "strong", own ? {
      type: "text", className: "ds-role", maxlength: String(ROLE_MAX), spellcheck: "false", value: card.role,
      "aria-label": L("专业", "Specialty"), placeholder: L("专业", "Specialty"), dataset: { focusKey: `${key}/role` },
      onInput: (e) => editCard(card.cid, { role: e.target.value }, false),
    } : {}, ...(own ? [] : [title]));
    const access = card.kind === CUSTOM
      ? select(`${key}/access`, L(`${title}的读写类型`, `Access of ${title}`), card.access, ACCESS, (value) => editCard(card.cid, { access: value }))
      : el("span", { className: "ds-access" }, accessName(accessOf(card)));
    const remove = el("button", { type: "button", className: "ds-remove", "aria-label": L(`删除${title}`, `Delete ${title}`), onClick: () => removeCard(card.cid) }, L("删除", "Delete"));
    const head = el("div", { className: "ds-head" }, name, access, card.kind === IMAGE ? el("span") : tiersSet(card), remove);
    const scope = scopeOf(card);
    const body = card.kind === IMAGE
      ? [...card.rows.map((row, at) => imageRow(card, row, at)),
        el("button", { type: "button", className: "ds-add-row", disabled: toSeats(cards()).length >= MAX_SEATS, onClick: () => addRow(card.cid) }, L("添加方向", "Add specialty"))]
      : DIFFICULTIES.map(([level, label]) => levelRow(card, level, label, title));
    return el("div", { className: "decision-seat" }, head, scope ? el("p", { className: "fine decision-note" }, scope) : null, ...body);
  }

  // The one way to add a card: a group of the role catalog not on a card yet, or a card of its own.
  function addMenu() {
    const groups = S.roleCatalog();
    const taken = new Set(cards().map((card) => card.kind));
    const option = (value, label) => el("option", { value, disabled: taken.has(value) && value !== CUSTOM }, label);
    const list = (title, entries) => entries.length ? el("optgroup", { label: title }, ...entries.map(([value, label]) => option(value, label))) : null;
    const named = (general) => groups.filter((entry) => !entry.fallback_group === general).map((entry) => [entry.group, S.roleName(entry)]);
    return el("select", {
      className: "decision-add", dataset: { focusKey: "decision/add" }, "aria-label": L("添加工位", "Add seat"),
      disabled: toSeats(cards()).length >= MAX_SEATS, onChange: (e) => e.target.value && addCard(e.target.value),
    }, el("option", { value: "", selected: true }, L("添加工位", "Add seat")),
    list(L("通用工位", "General seats"), named(true)), list(L("专业工位", "Specialist seats"), named(false)),
    list(L("自定义工位", "Custom seats"), [[CUSTOM, L("自定义工位", "Custom seat")]]),
    list(IMAGE_NAME(), [[IMAGE, IMAGE_NAME()]]));
  }

  // What stands in for the fixed seats on the decision mode's page, or null while the catalog cannot be
  // edited: where the data goes, the key and the catalog.
  // What happens on each dispatch, who does what, under one picture (the picture has no text).
  const FLOW = [
    [L("boss", "Boss"), L("写好任务，交给服务", "Writes the task and hands it to the service")],
    [L("服务", "Service"), L("筛出现在能派的工位", "Keeps the seats that can run now")],
    [L("决策模型", "Decision model"), L("只回答选哪个工位", "Only answers which seat to use")],
    [L("服务", "Service"), L("启动选中的工位，把选了谁告诉 boss", "Starts that seat and tells the boss which one")],
  ];
  function flow() {
    return el("figure", { className: "decision-flow" },
      el("img", { src: AN.art.url("office/decision-flow.webp"), alt: "", draggable: false }),
      el("figcaption", {}, ...FLOW.map(([who, what]) => el("span", {}, el("strong", {}, who), el("span", {}, what)))),
      el("p", { className: "fine decision-note" }, L(
        "决策模型不接触 CLI，也不改任务内容；任务跑完后，boss 照常自己去收结果。",
        "The decision model never touches a CLI or changes the task. When the work is done, the boss collects the result as usual.")));
  }

  function panel() {
    if (locked()) return null;
    return el("div", { className: "orchestration-section decision-seats" },
      flow(),
      el("p", { className: "fine decision-note" }, L(
        "派发任务时，派发内容（过长时只取开头和结尾）和各工位的名称、专业说明、难度、读写类型会发给 Google 的决策模型；Gemini 免费层的输入可能被用于改进产品；可用地区以 Google 公布的为准。",
        "When a task is dispatched, its prompt (only the beginning and end when long) and each seat's name, specialty, difficulty and access type are sent to Google's decision model. On the Gemini free tier, inputs may be used to improve Google's products. Availability by region follows what Google publishes.")),
      keyRow(),
      el("div", { className: "decision-catalog" }, ...cards().map(seatCard)),
      addMenu());
  }

  window.AgentNaveDecisionSeats = { load, panel, changed, problem, save, discard, entered, cards, document: fileDocument, prefill, addCard };
}
