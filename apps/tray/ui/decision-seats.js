"use strict";
{
  // Settings -> 编排 -> 决策模型 (experiment): the user's own seat catalog, which stands in for the
  // fixed seats on that mode's page, and the decision source (ADR-0123): platform, model and, for a
  // local or custom endpoint, its address. The catalog and the source are a draft like the rest of the
  // page and save from its one bar; a key is stored at once, as the voice keys are, and never read
  // back: the page only learns whether one is set for each platform, and a masked preview of it. Model lists are read from the
  // platform only when it is chosen or refreshed. Which bosses use the catalog is their mode, saved
  // with the orchestration.

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

  let saved = null; // { document | null, error, key_set, keys, key_previews, key_error, providers } from GET /app/decision-seats.
  let loadSeq = 0; // Stale GET /app/decision-seats replies arriving after a newer load or save are dropped.
  let sourceDraft = null; // { provider, model, base_url } when the source differs from saved; else null.
  const providerDrafts = new Map(); // provider -> { provider, model, base_url } kept while browsing other platforms in this draft.
  const catalogs = new Map(); // `${provider}|${base_url}` -> { state: "loading" | "ready" | "error", models, error }
  let draft = null; // The cards as edited; null while they are what is saved.
  let randomDraft = null; // Boolean when the random-candidates switch differs from saved; else null.
  let prefilled = false; // The first catalog was already made in this draft.
  let keyDraft = "";
  let keyBusy = false;
  let confirmDeleteKey = false;
  let manualModel = false; // The model id is typed: chosen in the list's last entry, or typed since.
  let keyMessage = null; // { text, error }
  let cardCount = 0;
  let rowCount = 0;
  const expanded = new Set(); // Expanded card cids, kept in memory across re-renders.

  // The file is a flat list, one seat per difficulty; the page shows one card for each kind and
  // specialty: { cid, kind, role, access, seats: { easy | medium | hard: { id, provider, model, effort? } } }.
  // A group of the role catalog is one card whatever its role says; custom cards go by role. All the
  // image seats are the rows of the one image card: { cid, kind: "image", rows: [{ rid, role, seat | null }] }.
  const catalogEntry = (kind) => S.roleCatalog().find((entry) => entry.group === kind);
  const levelsOf = () => DIFFICULTIES.map(([difficulty]) => difficulty);
  const seatsOfCard = (card) => (card.kind === IMAGE ? card.rows.map((row) => row.seat).filter(Boolean) : Object.values(card.seats));
  const bareSeat = ({ id, provider, account, model, effort, fallbacks, description }) => ({ id, provider, ...(account ? { account } : {}), model, ...(effort ? { effort } : {}), ...(fallbacks?.length ? { fallbacks } : {}), ...(description ? { description } : {}) });
  const accessOf = (card) => catalogEntry(card.kind)?.access || (card.kind === IMAGE ? "write" : card.access);

  function toCards(seats) {
    const cards = [];
    for (const seat of seats) {
      if (seat.kind === IMAGE) {
        let card = cards.find((one) => one.kind === IMAGE);
        if (!card) cards.push(card = { cid: `c${++cardCount}`, kind: IMAGE, role: "", access: "write", seats: {}, rows: [] });
        card.rows.push({ rid: `r${++rowCount}`, role: seat.role, loadedRole: seat.role.trim(), description: seat.description, seat: bareSeat({ ...seat, description: "" }) });
        continue;
      }
      const fixed = seat.kind !== CUSTOM;
      let card = cards.find((one) => one.kind === seat.kind && (fixed || one.role === seat.role));
      if (!card) {
        card = { cid: `c${++cardCount}`, kind: seat.kind, role: seat.role, loadedRole: seat.role.trim(), access: seat.access, description: seat.description, seats: {} };
        cards.push(card);
      }
      // A custom seat keeps the description the file gave it, for as long as its specialty stays.
      card.seats[seat.difficulty] = bareSeat(seat.kind === CUSTOM ? seat : { ...seat, description: "" });
    }
    return cards;
  }

  function toSeats(cards) {
    return cards.flatMap((card) => {
      if (card.kind === IMAGE) {
        return card.rows.filter((row) => row.seat).map(({ role, loadedRole, description, seat }) => ({
          id: seat.id, kind: IMAGE, role: role.trim(), description: role.trim() === loadedRole ? description : role.trim(), provider: seat.provider, ...(seat.account ? { account: seat.account } : {}), model: seat.model,
          ...(seat.effort ? { effort: seat.effort } : {}), ...(seat.fallbacks?.length ? { fallbacks: seat.fallbacks } : {}), access: "write",
        }));
      }
      return levelsOf().filter((level) => card.seats[level]).map((level) => {
        const { id, provider, account, model, effort, fallbacks, description } = card.seats[level];
        const role = card.role.trim();
        const group = catalogEntry(card.kind);
        // A custom seat's own description goes back as it was while its specialty is the one it was read with.
        const kept = card.kind === CUSTOM && role === card.loadedRole ? description || card.description : "";
        return {
          id, kind: card.kind, role, description: group?.scope_zh || (card.kind === CUSTOM ? kept || role : card.description || role),
          provider, ...(account ? { account } : {}), model, ...(effort ? { effort } : {}), ...(fallbacks?.length ? { fallbacks } : {}), access: accessOf(card), difficulty: level,
        };
      });
    });
  }

  // The saved file in the order and shape the page writes, to compare with the cards.
  const canon = (seats) => [...seats].sort((a, b) => (a.id < b.id ? -1 : 1)).map((seat) => ({
    id: seat.id, kind: seat.kind, role: seat.role, description: seat.description, provider: seat.provider, ...(seat.account ? { account: seat.account } : {}), model: seat.model,
    ...(seat.effort ? { effort: seat.effort } : {}), ...(seat.fallbacks?.length ? { fallbacks: seat.fallbacks } : {}), access: seat.access, ...(seat.difficulty ? { difficulty: seat.difficulty } : {}),
  }));
  // The saved cards are made once per loaded file, so a card keeps its handle between draws.
  let base = { of: null, cards: [] };
  function cards() {
    if (draft) return draft;
    if (base.of !== saved) base = { of: saved, cards: toCards(saved?.document?.seats || []) };
    return base.cards;
  }
  const savedRandom = () => saved?.document?.random_candidates === true;
  const randomCandidates = () => (randomDraft === null ? savedRandom() : randomDraft);

  // The decision sources the service offers, as the tray described them; Gemini alone while it did not.
  const GEMINI = { id: "gemini", name_zh: "Google Gemini", name_en: "Google Gemini", key: "required", fixed_model: true, default_model: "gemini-decision-1", models: [], catalog: false, custom_url: false };
  const sources = () => (saved?.providers?.length ? saved.providers : [GEMINI]);
  const sourceMeta = (id) => sources().find((one) => one.id === id) || sources()[0];
  const sourceName = (meta) => L(meta.name_zh, meta.name_en);
  const savedSource = () => {
    const document = saved?.document;
    const meta = sourceMeta(document?.decision_provider || "gemini");
    return { provider: meta.id, model: meta.fixed_model ? meta.default_model : document?.decision_model || meta.default_model || "", base_url: document?.decision_base_url || "" };
  };
  const source = () => sourceDraft || savedSource();
  const keySet = (id) => (saved?.keys ? saved.keys[id] === true : id === "gemini" && Boolean(saved?.key_set));
  const sourceChanged = () => Boolean(sourceDraft) && !sameJson(sourceDraft, savedSource());
  const fileDocument = () => {
    const { provider, model, base_url } = source();
    const meta = sourceMeta(provider);
    const address = base_url.trim().replace(/\/+$/, "");
    return {
      version: 1,
      ...(provider === "gemini" ? {} : { decision_provider: provider }),
      decision_model: meta.fixed_model ? meta.default_model : model.trim(),
      ...(meta.custom_url && address ? { decision_base_url: address } : {}),
      ...(randomCandidates() ? { random_candidates: true } : {}),
      seats: toSeats(cards()),
    };
  };
  const cardsChanged = () => Boolean(draft) && !sameJson(canon(toSeats(draft)), canon(saved.document?.seats || []));
  const changed = () => Boolean(saved) && !saved.error && (cardsChanged() || sourceChanged() || randomCandidates() !== savedRandom());
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

  // The fixed seats the user has bound make one card per group with each bound difficulty filled in,
  // medium left empty where the group has none, and the fixed image seat a card of its own.
  function prefill() {
    const doc = S.orchestrationShown();
    const taken = new Set();
    const bound = (seat) => {
      if (!seat?.provider || !seat?.model) return null;
      const id = freshId(taken);
      taken.add(id);
      return { id, provider: seat.provider, ...(seat.account ? { account: seat.account } : {}), model: seat.model, ...(seat.effort ? { effort: seat.effort } : {}) };
    };
    const made = S.roleCatalog().filter((entry) => S.liveSeats(doc).some(({ role }) => role === entry.group)).map((entry) => newCard(entry.group, {
      seats: Object.fromEntries(["easy", "medium", "hard"].map((difficulty) => [difficulty, bound(S.orchestrationSeat(doc, entry.group, difficulty))]).filter(([, seat]) => seat)),
    })).filter((card) => Object.keys(card.seats).length);
    const image = bound(doc.image_generation);
    if (image) made.push(newCard(IMAGE, { rows: [blankRow({ role: L("通用生图", "General image generation"), seat: image })] }));
    return made;
  }

  // The catalog stands in the orchestration card, so it is drawn with it.
  const render = () => S.renderOrchestration();

  function setRandomCandidates(on) {
    randomDraft = Boolean(on) === savedRandom() ? null : Boolean(on);
    render();
    S.advancedEdited();
  }

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
  const removeCard = (cid) => {
    expanded.delete(cid);
    edit(cards().filter((card) => card.cid !== cid));
  };
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
    else seats[level] = { ...bareSeat({ id: seats[level]?.id || freshId(takenIds()), ...next }), ...(seats[level]?.description ? { description: seats[level].description } : {}) };
    editCard(cid, { seats });
  }
  // `what` is a group of the role catalog, "custom" or "image".
  function addCard(what) {
    if (locked() || (what !== CUSTOM && what !== IMAGE && !catalogEntry(what)) || cards().some((card) => card.kind === what && what !== CUSTOM)) return;
    const card = newCard(what);
    expanded.add(card.cid);
    edit([...cards(), card]);
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

  // What keeps an address from being saved, or null: HTTPS, or HTTP on this machine, no account,
  // query or fragment (not even an empty one), no zero or empty port, at most 512 characters.
  function addressProblem(address) {
    const text = address.trim();
    if (/[\x00-\x1f\x7f]/.test(text)) return L("API 地址无效", "The API address is not valid");
    let url;
    try { url = new URL(text); } catch { return L("API 地址无效", "The API address is not valid"); }
    const normalized = url.href.replace(/\/+$/, "");
    if (text.length > 512 || normalized.length > 512) return L("API 地址过长", "The API address is too long");
    if (url.username || url.password || /[?#]/.test(text) || /^[a-z][a-z0-9+.-]*:\/*[^/]*@/i.test(text)) return L("API 地址不能包含账号、查询参数或片段", "The API address cannot hold an account, query or fragment");
    const authority = text.match(/^[a-z][a-z0-9+.-]*:\/\/([^/?#]*)/i)?.[1];
    if (!authority || authority.includes("\\") || url.port === "0" || authority.endsWith(":")) return L("API 地址无效", "The API address is not valid");
    const host = url.hostname.replace(/^\[|\]$/g, "").toLowerCase();
    const loopback = host === "localhost" || /^127(\.\d{1,3}){3}$/.test(host) || host === "::1";
    if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) return L("API 只允许 HTTPS，或本机上的 HTTP", "The API takes HTTPS, or HTTP on this machine");
    return null;
  }

  // What keeps the source from being saved, or null.
  function sourceProblem() {
    const { provider, model, base_url } = source();
    const meta = sourceMeta(provider);
    if (meta.custom_url && (meta.url_required || base_url.trim())) {
      if (!base_url.trim()) return L("请填写 API 地址", "Enter the API address");
      const found = addressProblem(base_url);
      if (found) return found;
    }
    if (!meta.fixed_model && !model.trim()) return L("请填写决策模型", "Enter a decision model");
    if (!meta.fixed_model && !/^[\x21-\x7e]{1,200}$/.test(model.trim())) return L("决策模型只能是 1 到 200 个可见 ASCII 字符", "The decision model must be 1 to 200 visible ASCII characters");
    return null;
  }

  // What keeps the draft from being saved, or null.
  function problem() {
    const found = sourceProblem();
    if (found) return found;
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
    const seq = ++loadSeq;
    try {
      const response = await fetch("/app/decision-seats", { cache: "no-store" });
      if (!response.ok) throw new Error(L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
      const loaded = await response.json();
      if (seq !== loadSeq) return;
      const keep = changed();
      // A file error while an unsaved draft stands keeps the last good baseline.
      if (!(loaded.error && keep)) saved = loaded;
      if (loaded.error) S.showNotice(L(`${loaded.error}；请先修复或删除这个文件`, `${loaded.error}; fix or delete this file first`), true);
      if (!keep) {
        draft = null;
        sourceDraft = null;
        randomDraft = null;
        prefilled = false;
        manualModel = false;
        // The first list is asked for only while the decision panel itself is open, not from another tab or mode.
        loadVisibleCatalog();
      }
    } catch (error) {
      if (seq !== loadSeq) return;
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
      if (!old || old.provider !== seat.provider || old.model !== seat.model || !sameJson(old.fallbacks || [], seat.fallbacks || [])) await S.listSeatModel(seat);
    }
    await post("/app/decision-seats", doc);
    loadSeq++;
    saved = { ...saved, document: doc, error: null };
    draft = null;
    sourceDraft = null;
    providerDrafts.clear();
    randomDraft = null;
    manualModel = false;
    render();
  }

  function problemCids(list) {
    const bad = new Set();
    const seen = new Map();
    for (const card of list) {
      if (card.kind === IMAGE) {
        if (imageProblem(card)) bad.add(card.cid);
        continue;
      }
      const own = card.kind === CUSTOM;
      const title = own ? card.role.trim() : cardName(card);
      if (own && !title) bad.add(card.cid);
      if (own && title && seen.has(title)) {
        bad.add(card.cid);
        if (seen.get(title).kind === CUSTOM) bad.add(seen.get(title).cid);
      }
      if (title && !seen.has(title)) seen.set(title, card);
      if (!Object.keys(card.seats).length) bad.add(card.cid);
    }
    return bad;
  }

  function discard() {
    expanded.clear();
    draft = null;
    sourceDraft = null;
    providerDrafts.clear();
    randomDraft = null;
    prefilled = false;
    manualModel = false;
    keyDraft = "";
    confirmDeleteKey = false;
    keyMessage = null;
  }

  // The platform's own model list: read when it is chosen or refreshed, and kept until the page closes.
  const catalogKey = ({ provider, base_url }) => `${provider}|${base_url.trim()}`;
  const catalogOf = (one) => catalogs.get(catalogKey(one));
  const sameSource = (one) => catalogKey(one) === catalogKey(source());

  async function startCatalog(one, refresh) {
    const meta = sourceMeta(one.provider);
    const key = catalogKey(one);
    const before = catalogs.get(key);
    if (!meta.catalog || before?.state === "loading" || (before && !refresh)) return;
    catalogs.set(key, { state: "loading", models: before?.models || [], error: null });
    const address = one.base_url.trim() || meta.default_base_url || "";
    const query = `provider=${encodeURIComponent(one.provider)}${address ? `&base_url=${encodeURIComponent(address)}` : ""}${refresh ? "&refresh=1" : ""}`;
    try {
      const response = await fetch(`/app/decision-models?${query}`, { cache: "no-store" });
      const data = await response.json().catch(() => ({}));
      if (!response.ok || data.error) throw new Error(data.error || L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
      catalogs.set(key, { state: "ready", models: data.models || [], error: null });
    } catch (error) {
      catalogs.set(key, { state: "error", models: before?.models || [], error: error.message });
    }
    if (sameSource(one)) render();
  }

  // Platforms whose list comes with choosing them: the open ones, and TypeSafe once its key is saved.
  const listsOnChoice = (provider) => provider === "openrouter" || provider === "ollama" || (provider === "typesafe" && keySet(provider));
  const loadVisibleCatalog = () => {
    const current = savedSource();
    if (!locked() && !sourceDraft && S.viewingDecision?.() && listsOnChoice(current.provider)) startCatalog(current, false);
  };
  const viewed = () => loadVisibleCatalog();

  function chooseSource(next) {
    const meta = sourceMeta(next.provider);
    sourceDraft = sameJson(next, savedSource()) ? null : next;
    keyDraft = "";
    confirmDeleteKey = false;
    keyMessage = null;
    manualModel = false;
    const savedBase = next.provider === savedSource().provider ? savedSource().base_url : "";
    const addressUnedited = !meta.custom_url || next.base_url.trim() === savedBase;
    if (S.viewingDecision?.() && listsOnChoice(meta.id) && addressUnedited) startCatalog(next, false);
    render();
    S.advancedEdited();
  }
  const chooseProvider = (id) => {
    const current = source();
    if (sourceDraft) providerDrafts.set(current.provider, { ...current });
    else providerDrafts.delete(current.provider);
    const meta = sourceMeta(id);
    const recalled = providerDrafts.get(id);
    if (recalled) return chooseSource({ ...recalled });
    chooseSource(id === savedSource().provider ? savedSource() : { provider: id, model: meta.default_model || "", base_url: "" });
  };
  const chooseModel = (model, rerender = true) => {
    sourceDraft = { ...source(), model };
    if (sameJson(sourceDraft, savedSource())) sourceDraft = null;
    if (rerender) render();
    S.advancedEdited();
  };

  async function storeKey(key) {
    const target = { ...source() };
    const provider = target.provider;
    keyBusy = true;
    confirmDeleteKey = false;
    keyMessage = null;
    render();
    try {
      const data = await post("/app/decision-key", { provider, key });
      saved.keys = data.keys || { ...(saved.keys || { gemini: Boolean(saved.key_set) }), [provider]: data.key_set };
      saved.key_previews = data.key_previews || {};
      saved.key_set = data.key_set;
      if (source().provider === provider) {
        keyDraft = "";
        keyMessage = { text: key ? L("密钥已保存", "Key saved") : L("密钥已删除", "Key deleted"), error: false };
        if (key && provider === "typesafe" && S.viewingDecision?.()) startCatalog(target, true);
      }
    } catch (error) {
      if (source().provider === provider) keyMessage = { text: error.message, error: true };
    }
    keyBusy = false;
    render();
  }

  // Where the dispatch goes, said for the platform in use: the tip of the platform's row.
  function sentTip(meta) {
    const kept = L("决策模型不接触 CLI，也不改任务内容；任务跑完后，boss 照常自己去收结果。", "The decision model never touches a CLI or changes the task; when the work is done, the boss collects the result as usual.");
    const what = L("派发任务时，派发内容（过长时只取开头和结尾）和各工位的名称、专业说明、难度、读写类型会", "When a task is dispatched, its prompt (only the beginning and end when long) and each seat's name, specialty, difficulty and access type are ");
    const where = {
      gemini: [
        L("发给 Google 的决策模型；Gemini 免费层的输入可能被用于改进产品；可用地区以 Google 公布的为准。", "sent to Google's decision model. On the Gemini free tier, inputs may be used to improve Google's products. Availability by region follows what Google publishes."),
      ],
      openai: [L("发给 OpenAI 的决策模型。", "sent to OpenAI's decision model.")],
      typesafe: [L("发给 TypeSafe AI 的决策模型。", "sent to TypeSafe AI's decision model.")],
      openrouter: [L("发给 OpenRouter，由它转给你选的决策模型的服务方。", "sent to OpenRouter, which passes them on to the provider of the decision model you chose.")],
      ollama: [L("只发给上面填的 Ollama 地址；填的是本机地址且运行本地模型时，内容不出本机。", "sent only to the Ollama address above; with a local address and a locally run model, nothing leaves this machine.")],
      custom: [L("发给上面填的自定义地址对应的服务。", "sent to the service at the custom address above.")],
    }[meta.id];
    return `${what}${where[0]}${kept}`;
  }

  function sourceRow() {
    const meta = sourceMeta(source().provider);
    const select = el("select", {
      id: "decision-source", "aria-label": L("决策平台", "Decision platform"), dataset: { focusKey: "decision/provider" },
      onChange: (e) => chooseProvider(e.target.value),
    }, sources().map((one) => el("option", { value: one.id, selected: one.id === meta.id }, sourceName(one))));
    return el("div", { className: "s-row" },
      el("label", { className: "s-label", htmlFor: "decision-source" }, L("决策平台", "Decision platform"), AN.settings.settingTip(sentTip(meta))),
      el("div", { className: "s-field" }, select));
  }

  function addressRow(meta) {
    const input = el("input", {
      type: "text", id: "decision-address", maxlength: "512", spellcheck: "false", autocorrect: "off", autocapitalize: "off", autocomplete: "off",
      value: source().base_url, "aria-label": L("API 地址", "API address"), dataset: { focusKey: "decision/address" },
      placeholder: meta.default_base_url || "https://api.example.com/v1",
      onInput: (e) => {
        sourceDraft = { ...source(), base_url: e.target.value };
        if (sameJson(sourceDraft, savedSource())) sourceDraft = null;
        S.advancedEdited();
      },
    });
    return el("div", { className: "s-row" },
      el("label", { className: "s-label", htmlFor: "decision-address" }, L("API 地址", "API address")),
      el("div", { className: "s-field" }, input));
  }

  function modelOption(model, typing) {
    return el("option", { value: model.id, selected: !typing && model.id === source().model }, model.name && model.name !== model.id ? `${model.name} · ${model.id}` : model.id);
  }

  const MANUAL = "__manual__";

  // A platform with one model shows it. The others offer a choice from the built-in picks and the
  // platform's own list, with a refresh for that list; the model's id is typed only after the list's
  // last entry asks for it or when the model is not in the list, and is the whole row while there is
  // no list.
  function modelRow(meta) {
    const current = source();
    if (meta.fixed_model) {
      return el("div", { className: "s-row" },
        el("span", { className: "s-label" }, L("决策模型", "Decision model")),
        el("code", { className: "s-value is-mono" }, meta.default_model));
    }
    const listed = catalogOf(current);
    const recommended = meta.models || [];
    const others = (listed?.models || []).filter((model) => !recommended.some((one) => one.id === model.id));
    const known = [...recommended, ...others].some((model) => model.id === current.model);
    const hasList = recommended.length + others.length > 0;
    const typing = hasList && (manualModel || (current.model.trim() !== "" && !known));
    const choose = hasList ? el("select", {
      id: "decision-model", "aria-label": L("决策模型", "Decision model"), dataset: { focusKey: "decision/model" },
      onChange: (e) => {
        manualModel = e.target.value === MANUAL;
        if (manualModel) {
          render();
          document.getElementById("decision-model-id")?.focus?.();
        } else if (e.target.value) chooseModel(e.target.value);
        else render();
      },
    },
    el("option", { value: "", disabled: true, selected: !known && !typing }, L("选择模型", "Choose a model")),
    recommended.length ? el("optgroup", { label: L("推荐", "Recommended") }, recommended.map((model) => modelOption(model, typing))) : null,
    others.length ? el("optgroup", { label: L("可用模型", "Available") }, others.map((model) => modelOption(model, typing))) : null,
    el("option", { value: MANUAL, selected: typing }, L("手动输入模型 ID…", "Enter model ID manually…"))) : null;
    const refresh = el("button", {
      type: "button", dataset: { focusKey: "decision/model-refresh" },
      disabled: listed?.state === "loading",
      onClick: () => {
        // What is typed since this row was drawn counts; a custom endpoint is nowhere to ask until its address is typed.
        const now = source();
        if (meta.url_required && !now.base_url.trim()) catalogs.set(catalogKey(now), { state: "error", models: [], error: L("请填写 API 地址", "Enter the API address") });
        else startCatalog(now, true);
        render();
      },
    }, L("刷新", "Refresh"));
    const typed = hasList && !typing ? null : el("input", {
      type: "text", id: "decision-model-id", maxlength: "200", spellcheck: "false", autocorrect: "off", autocapitalize: "off", autocomplete: "off",
      value: current.model, "aria-label": L("模型 ID", "Model ID"), placeholder: L("模型 ID", "Model ID"), dataset: { focusKey: "decision/model-id" },
      onInput: (e) => {
        if (hasList) manualModel = true;
        chooseModel(e.target.value, false);
      },
    });
    return el("div", { className: "s-row" },
      el("label", { className: "s-label", htmlFor: hasList ? "decision-model" : "decision-model-id" }, L("决策模型", "Decision model")),
      el("div", { className: "s-stack" },
        el("div", { className: "s-field" }, hasList ? choose : typed, refresh),
        hasList && typed && el("div", { className: "s-field" }, typed),
        listed?.state === "loading" && el("p", { className: "s-muted", role: "status" }, L("正在读取模型列表…", "Loading models…")),
        listed?.state === "error" && el("p", { className: "s-muted is-bad", role: "status" }, listed.error)));
  }

  function keyRow(meta) {
    const name = sourceName(meta);
    const label = L(`${name} 密钥${meta.key === "optional" ? "（可选）" : ""}`, `${name} API key${meta.key === "optional" ? " (optional)" : ""}`);
    const set = keySet(meta.id);
    const preview = set ? saved.key_previews?.[meta.id] || "" : "";
    const save = el("button", { type: "button", disabled: keyBusy || !keyDraft.trim(), onClick: () => storeKey(keyDraft) }, L("保存", "Save"));
    const input = el("input", {
      type: "password", autocomplete: "off", spellcheck: "false", autocorrect: "off", autocapitalize: "off", maxlength: "4096", "aria-label": label, disabled: keyBusy,
      dataset: { focusKey: "decision/key" }, id: "decision-key",
      placeholder: set ? (preview ? L(`${preview} · 粘贴新密钥可替换`, `${preview} · Paste a new key to replace`) : L("已保存，粘贴新密钥可替换", "Saved; paste a new key to replace")) : L("粘贴密钥", "Paste API key"),
      title: preview || null,
      onInput: (e) => { keyDraft = e.target.value; save.disabled = keyBusy || !keyDraft.trim(); },
      onKeydown: (e) => {
        if (e.key !== "Enter" || keyBusy || !keyDraft.trim()) return;
        e.preventDefault();
        void storeKey(keyDraft);
      },
    });
    input.value = keyDraft;
    const confirming = confirmDeleteKey && set;
    const remove = el("button", { type: "button", className: "danger key-delete", disabled: keyBusy || !set || confirming, onClick: () => { confirmDeleteKey = true; render(); } }, L("删除", "Delete"));
    let confirmBox = null;
    if (confirming) {
      const keep = el("button", { type: "button", disabled: keyBusy, onClick: () => { confirmDeleteKey = false; render(); } }, L("保留", "Keep"));
      const confirmBtn = el("button", { type: "button", className: "danger", disabled: keyBusy, onClick: () => { confirmDeleteKey = false; void storeKey(""); } }, L("确认删除", "Delete"));
      confirmBox = el("div", { className: "disable-confirm", role: "group", "aria-label": L(`确认删除 ${name} 密钥`, `Confirm deleting ${name} API key`) },
        el("p", {}, L(`删除后将移除已保存的 ${name} 密钥，无法撤销。`, `Deleting removes the saved ${name} API key and cannot be undone.`)),
        el("div", { className: "btn-group" }, keep, confirmBtn));
      keep.focus?.({ preventScroll: true });
    }
    return el("div", { className: "s-row" },
      el("label", { className: "s-label", htmlFor: "decision-key" }, label),
      el("div", { className: "s-stack" },
        el("div", { className: "s-field" }, input, save, remove),
        confirmBox,
        saved.key_error && el("p", { className: "s-muted is-bad" }, saved.key_error),
        keyMessage && el("p", { className: `s-muted${keyMessage.error ? " is-bad" : ""}`, role: "status" }, keyMessage.text)));
  }

  // The platform, its address and model, and its key, which a local Ollama has none of.
  function sourceRows() {
    const meta = sourceMeta(source().provider);
    return [sourceRow(), meta.custom_url && addressRow(meta), modelRow(meta), meta.key !== "none" && keyRow(meta)];
  }

  // One row of a card: the level's name and its CLI, model and effort.
  function levelRow(card, level, label, title) {
    const key = `decision/${card.cid}/${level}`;
    const seat = card.seats[level] || null;
    const set = (next) => setSeat(card.cid, level, next);
    const row = S.seatSelects(key, `${title} · ${label}`, seat, set, L("未设置", "Not set"));
    return S.seatBlock(el("div", { className: "s-row is-sub" }, el("span", { className: "s-label" }, label), el("div", { className: "s-seat" }, ...row)), key, `${title} · ${label}`, seat, set);
  }

  const modelDisplay = (seat) => (seat?.model ? (AN.modelName ? AN.modelName(seat.model) : seat.model) : "");

  function cardSummary(card) {
    if (card.kind === IMAGE) return S.summarizeNames(card.rows.map((row) => row.role));
    return S.summarizeTiers(DIFFICULTIES.map(([level, label]) => [label, modelDisplay(card.seats[level])]));
  }

  // One image specialty: its name and its CLI, model and effort on one line, and its delete button.
  function imageRow(card, row, index, helpers) {
    const key = `decision/${card.cid}/${row.rid}`;
    const name = row.role.trim() || L(`生图工位第 ${index + 1} 行`, `Image seats row ${index + 1}`);
    const specialty = el("input", {
      type: "text", className: "ds-role", maxlength: String(ROLE_MAX), spellcheck: "false", autocorrect: "off", autocapitalize: "off", autocomplete: "off", value: row.role,
      "aria-label": L("专业", "Specialty"), placeholder: L("专业", "Specialty"), dataset: { focusKey: `${key}/role` },
      onInput: (e) => {
        editRow(card.cid, row.rid, { role: e.target.value }, false);
        const current = cards().find((one) => one.cid === card.cid);
        if (current) {
          const summary = cardSummary(current);
          helpers.setSummary(summary.text, summary.unset);
        }
      },
    });
    const set = (next) => setRowSeat(card.cid, row.rid, next);
    const selects = S.seatSelects(key, name, row.seat, set, L("未设置", "Not set"));
    const remove = el("button", { type: "button", className: "ds-remove", "aria-label": L(`删除${name}`, `Delete ${name}`), onClick: () => removeRow(card.cid, row.rid) }, L("删除", "Delete"));
    // The add-fallback button goes in before the delete button, which stays in the last column.
    const seatControl = el("div", { className: "s-seat" }, ...selects);
    const main = el("div", { className: "s-row is-sub" }, el("div", { className: "s-label" }, specialty), seatControl);
    const block = S.seatBlock(main, key, name, row.seat, set);
    seatControl.appendChild(remove);
    return block;
  }

  function customHeadRow(card, key, title, helpers) {
    return S.customSeatHeadRow({
      key, role: card.role, access: card.access, title, accessOptions: ACCESS, roleMax: ROLE_MAX,
      onRoleInput: (value) => {
        const prevBad = problemCids(cards());
        editCard(card.cid, { role: value }, false);
        const nextBad = problemCids(cards());
        if ([...nextBad].some((id) => id !== card.cid && !prevBad.has(id) && !expanded.has(id))) return render();
        helpers.setName(value.trim() || L("自定义工位", "Custom seat"), !value.trim());
      },
      onAccessChange: (value) => editCard(card.cid, { access: value }),
    });
  }

  function seatRowDescriptor(card, index, invalidSet) {
    const key = `decision/${card.cid}`;
    const own = card.kind === CUSTOM;
    const named = cardName(card);
    const title = named || L(`卡片 ${index + 1}`, `Card ${index + 1}`);
    const summary = cardSummary(card);
    return {
      cid: card.cid,
      focusPrefix: key,
      name: named || L("自定义工位", "Custom seat"),
      nameUnset: own && !named,
      ariaTitle: title,
      tip: scopeOf(card),
      access: accessName(accessOf(card)),
      summary: summary.text,
      summaryUnset: summary.unset,
      invalid: invalidSet.has(card.cid),
      isInvalid: () => problemCids(cards()).has(card.cid),
      onRemove: () => removeCard(card.cid),
      renderBody: (helpers) => (card.kind === IMAGE
        ? [...card.rows.map((row, at) => imageRow(card, row, at, helpers)),
          el("div", { className: "s-row is-sub is-stack" }, el("button", { type: "button", className: "ds-add-row", disabled: toSeats(cards()).length >= MAX_SEATS, onClick: () => addRow(card.cid) }, L("添加方向", "Add specialty")))]
        : [...(own ? [customHeadRow(card, key, title, helpers)] : []),
          ...DIFFICULTIES.map(([level, label]) => levelRow(card, level, label, title))]),
    };
  }

  function seatRowsCard() {
    const list = cards();
    const invalidSet = problemCids(list);
    return S.renderSeatRows(list.map((card, index) => seatRowDescriptor(card, index, invalidSet)), {
      expanded,
      onToggle: () => render(),
      groupClass: "decision-seat-list",
    });
  }

  // The one way to add a card: a group of the role catalog not on a card yet, or a card of its own.
  function addMenu() {
    return S.seatAddMenu({
      id: "decision-add", focusKey: "decision/add", disabled: toSeats(cards()).length >= MAX_SEATS,
      taken: new Set(cards().map((card) => card.kind)), groups: S.roleCatalog(), includeImage: true, onAdd: addCard,
    });
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
      el("figcaption", {}, ...FLOW.map(([who, what]) => el("span", {}, el("strong", {}, who), el("span", {}, what)))));
  }

  function randomRow() {
    return el("div", { className: "s-row" },
      el("label", { className: "s-label", htmlFor: "decision-random" }, L("同一档的候选随机选，分摊额度", "Pick randomly among a tier's candidates to spread quota")),
      el("input", {
        type: "checkbox", id: "decision-random", className: "switch-input", dataset: { focusKey: "decision/random-candidates" },
        checked: randomCandidates(), onChange: (e) => setRandomCandidates(e.target.checked),
      }));
  }

  function panel() {
    if (locked()) return null;
    return el("div", { className: "s-wrap decision-seats" },
      el("div", { className: "s-group" }, el("div", { className: "s-card" },
        el("div", { className: "s-row is-stack" }, flow()), ...sourceRows(), randomRow())),
      seatRowsCard(),
      el("div", { className: "s-group" }, el("div", { className: "s-card" },
        el("div", { className: "s-row" }, el("label", { className: "s-label", htmlFor: "decision-add" }, L("添加工位", "Add seat")), addMenu()))));
  }

  const hasKeyDraft = () => Boolean(keyDraft.trim());
  const discardKeyDraft = () => {
    keyDraft = "";
    confirmDeleteKey = false;
    render();
  };

  window.AgentNaveDecisionSeats = { load, panel, changed, problem, save, discard, entered, viewed, cards, document: fileDocument, prefill, addCard, hasKeyDraft, discardKeyDraft };
}
