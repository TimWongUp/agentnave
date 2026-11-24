"use strict";
{
  // Settings -> 编排 -> T3 Code (ADR-0121): independent T3 orchestration switch, random-candidates
  // switch and T3 task seat cards. The T3 review roles sit on Settings -> 评审设置, after the seats
  // used outside T3 Code. All saved to t3-seats.json from the shared save bar; never writes T3 Code's own files.

  const S = AN.settings ||= {};
  const { $, el, sameJson } = S;
  const L = AN.L || ((zh) => zh);

  const CUSTOM = "custom";
  const IMAGE = "image";
  const DIFFICULTIES = [["easy", L("简单", "Easy")], ["medium", L("中等", "Medium")], ["hard", L("困难", "Hard")]];
  const ACCESS = [["read", L("只读", "Read-only")], ["write", L("可写", "Writable")]];
  const accessName = (access) => ACCESS.find(([value]) => value === access)[1];
  const MAX_SEATS = 255;
  const MAX_CANDIDATES = 4;
  const ROLE_MAX = 40;
  const AGY_LEVEL_RE = /^(.+)-(low|medium|high|xhigh|max)$/;

  let saved = null; // { document | null, error | null, t3: { detected, instances } }
  let baseline = null; // Normalized editor state of saved.document, or first-time prefill baseline.
  let draft = null; // Edited state; null while identical to baseline.
  let prefilled = false;
  let cardCount = 0;
  const expanded = new Set(); // Expanded card cids, kept in memory across re-renders.

  const catalogEntry = (kind) => S.roleCatalog().find((entry) => entry.group === kind);
  const reviewRoles = () => S.REVIEW_ROLES || [];
  const levelsOf = () => DIFFICULTIES.map(([difficulty]) => difficulty);
  const accessOf = (card) => catalogEntry(card.kind)?.access || card.access;
  const pairKey = (instance, model) => `${instance}\u0000${model}`;
  const cloneCandidate = (c) => ({
    instance: c.instance,
    model: c.model,
    options: (c.options || []).map(({ id, value }) => ({ id, value })),
  });
  const cloneSeat = (seat) => ({
    id: seat.id,
    candidates: (seat.candidates || []).map(cloneCandidate),
    ...(seat.description ? { description: seat.description } : {}),
  });
  const cloneCard = (card) => ({
    ...card,
    seats: Object.fromEntries(Object.entries(card.seats).map(([lvl, seat]) => [lvl, cloneSeat(seat)])),
  });
  const cloneState = (state) => ({
    enabled: state.enabled,
    random_candidates: state.random_candidates,
    cards: state.cards.map(cloneCard),
    review: Object.fromEntries(Object.entries(state.review).map(([r, list]) => [r, list.map(cloneCandidate)])),
    preservedSeats: state.preservedSeats.map((seat) => structuredClone(seat)),
  });

  function freshId(taken) {
    let id;
    do id = `t3-${Math.random().toString(16).slice(2, 8).padEnd(6, "0")}`;
    while (taken.has(id));
    taken.add(id);
    return id;
  }

  function toCards(seats) {
    const cards = [];
    const preservedSeats = [];
    for (const seat of seats || []) {
      if (seat.kind === IMAGE) {
        preservedSeats.push(structuredClone(seat));
        continue;
      }
      const fixed = seat.kind !== CUSTOM;
      let card = cards.find((one) => one.kind === seat.kind && (fixed || one.role === seat.role));
      if (!card) {
        card = {
          cid: `t3c${++cardCount}`,
          kind: seat.kind,
          role: seat.role,
          loadedRole: seat.role.trim(),
          access: seat.access,
          description: seat.description,
          seats: {},
        };
        cards.push(card);
      }
      card.seats[seat.difficulty] = {
        id: seat.id,
        candidates: (seat.candidates || []).map(cloneCandidate),
        ...(seat.kind === CUSTOM && seat.description ? { description: seat.description } : {}),
      };
    }
    return { cards, preservedSeats };
  }

  function fromDocument(doc) {
    const { cards, preservedSeats } = toCards(doc?.seats || []);
    const review = {};
    for (const [role, list] of Object.entries(doc?.review || {})) {
      if (Array.isArray(list) && list.length) review[role] = list.map(cloneCandidate);
    }
    return {
      enabled: doc?.enabled === true,
      random_candidates: doc?.random_candidates === true,
      cards,
      review,
      preservedSeats,
    };
  }

  function toSeats(cards) {
    return cards.flatMap((card) =>
      levelsOf().filter((level) => Object.hasOwn(card.seats, level)).map((level) => {
        const seat = card.seats[level];
        const role = card.role.trim();
        const group = catalogEntry(card.kind);
        const kept = card.kind === CUSTOM && role === card.loadedRole ? seat.description || card.description : "";
        return {
          id: seat.id,
          kind: card.kind,
          role,
          description: group?.scope_zh || (card.kind === CUSTOM ? kept || role : card.description || role),
          access: accessOf(card),
          difficulty: level,
          candidates: (seat.candidates || []).map(cloneCandidate),
        };
      }),
    );
  }

  function toDocument(state) {
    if (!state) return { version: 1, enabled: false, random_candidates: false, seats: [], review: {} };
    const review = {};
    for (const [role, list] of Object.entries(state.review || {})) {
      if (list?.length) review[role] = list.map(cloneCandidate);
    }
    return {
      version: 1,
      enabled: Boolean(state.enabled),
      random_candidates: Boolean(state.random_candidates),
      seats: [...toSeats(state.cards), ...state.preservedSeats.map((s) => structuredClone(s))],
      review,
    };
  }

  const canonCandidate = (c) => ({
    instance: c.instance,
    model: c.model,
    options: (c.options || []).map(({ id, value }) => ({ id, value })),
  });
  function canon(doc) {
    const seats = [...(doc?.seats || [])]
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
      .map((s) => ({
        id: s.id, kind: s.kind, role: (s.role || "").trim(), description: (s.description || "").trim(),
        access: s.access, ...(s.difficulty ? { difficulty: s.difficulty } : {}),
        candidates: (s.candidates || []).map(canonCandidate),
      }));
    const review = Object.fromEntries(
      Object.entries(doc?.review || {})
        .filter(([, list]) => Array.isArray(list) && list.length)
        .sort(([a], [b]) => (a < b ? -1 : 1))
        .map(([role, list]) => [role, list.map(canonCandidate)]),
    );
    return { version: 1, enabled: doc?.enabled === true, random_candidates: doc?.random_candidates === true, seats, review };
  }

  const canonCard = (card) => ({
    kind: card.kind,
    role: (card.role || "").trim(),
    access: accessOf(card),
    seats: Object.fromEntries(
      levelsOf()
        .filter((level) => Object.hasOwn(card.seats, level))
        .map((level) => [level, {
          id: card.seats[level].id,
          candidates: (card.seats[level].candidates || []).map(canonCandidate),
        }]),
    ),
  });
  const canonState = (state) => ({
    doc: canon(toDocument(state)),
    cards: (state?.cards || []).map(canonCard),
  });

  const shownState = () => draft || baseline;
  const instances = () => saved?.t3?.instances || [];
  const eligibleInstances = () => instances().filter((inst) => inst.available && !inst.stale);
  const visible = () => Boolean(saved && (saved.t3?.detected || saved.document || saved.error));
  const locked = () => !saved || Boolean(saved.error);
  const fileDocument = () => toDocument(shownState());
  const changed = () => Boolean(saved && !saved.error && draft && baseline)
    && !sameJson(canonState(draft), canonState(baseline));
  // The two pages the settings sit on, so each can mark its own unsaved edits.
  const reviewChanged = () => changed() && !sameJson(canon(toDocument(draft)).review, canon(toDocument(baseline)).review);
  const seatsChanged = () => changed()
    && !sameJson({ ...canonState(draft), doc: { ...canonState(draft).doc, review: {} } },
      { ...canonState(baseline), doc: { ...canonState(baseline).doc, review: {} } });

  function hasCandidates(doc = fileDocument()) {
    return (doc.seats || []).some((seat) => (seat.candidates || []).length > 0)
      || Object.values(doc.review || {}).some((list) => Array.isArray(list) && list.length > 0);
  }

  function savedReferencedInstances() {
    const refs = new Set();
    const doc = saved?.document;
    for (const seat of doc?.seats || []) for (const c of seat.candidates || []) refs.add(c.instance);
    for (const list of Object.values(doc?.review || {})) for (const c of list || []) refs.add(c.instance);
    return refs;
  }

  function takenIds() {
    const state = shownState();
    if (!state) return new Set();
    const ids = state.cards.flatMap((card) => Object.values(card.seats).map((seat) => seat.id));
    for (const seat of state.preservedSeats) ids.push(seat.id);
    return new Set(ids);
  }

  // ---- First-time prefill (Item 7) ----

  function targetInstancesForPick(pick, pool) {
    if (!pick?.provider || !pick?.model) return [];
    switch (pick.provider) {
      case "claude":
        // Named AgentNave Claude accounts are not guessed against T3 instances.
        return pick.account ? [] : pool.filter((inst) => inst.driver === "claudeAgent");
      case "codex":
        return pool.filter((inst) => inst.driver === "codex");
      case "grok":
        return pool.filter((inst) => inst.driver === "grok");
      case "antigravity":
        return pool.filter((inst) => inst.driver === "antigravity");
      case "codebuddy":
        return pool.filter((inst) => inst.driver === "acpRegistry" && inst.instance.startsWith("acpRegistry_codebuddy"));
      default:
        return [];
    }
  }

  function matchModelInInstance(inst, provider, modelName, effort) {
    const models = inst.models || [];
    let matched = models.find((m) => m.slug === modelName)
      || models.find((m) => Array.isArray(m.aliases) && m.aliases.includes(modelName));
    let resolvedEffort = effort || "";
    if (!matched && provider === "antigravity") {
      const split = AGY_LEVEL_RE.exec(modelName);
      if (split) {
        const [, base, level] = split;
        const baseModel = models.find((m) => m.slug === base);
        if (baseModel) {
          matched = baseModel;
          resolvedEffort = level;
        }
      }
    }
    if (!matched) return null;
    const options = [];
    const reasoning = matched.reasoning;
    if (resolvedEffort && reasoning?.id && reasoning.options?.some((opt) => opt.value === resolvedEffort)) {
      options.push({ id: reasoning.id, value: resolvedEffort });
    }
    return { instance: inst.instance, model: matched.slug, options };
  }

  function convertPicksToT3(seat, pool) {
    if (!seat?.provider || !seat?.model) return [];
    const picks = [seat, ...(seat.fallbacks || [])];
    const out = [];
    const seen = new Set();
    for (const pick of picks) {
      for (const inst of targetInstancesForPick(pick, pool)) {
        const candidate = matchModelInInstance(inst, pick.provider, pick.model, pick.effort);
        if (!candidate) continue;
        const key = pairKey(candidate.instance, candidate.model);
        if (seen.has(key)) continue;
        seen.add(key);
        out.push(candidate);
        if (out.length >= MAX_CANDIDATES) return out;
      }
    }
    return out;
  }

  function starterCandidates(pool) {
    const out = [];
    const seen = new Set();
    for (const inst of pool) {
      const models = inst.models || [];
      const pick = models.find((m) => m.default) || models.find((m) => !m.legacy && !m.hidden);
      if (!pick) continue;
      const key = pairKey(inst.instance, pick.slug);
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({ instance: inst.instance, model: pick.slug, options: [] });
      if (out.length >= MAX_CANDIDATES) break;
    }
    return out;
  }

  function prefill(pool = eligibleInstances()) {
    const taken = new Set();
    const decisionSeats = (window.AgentNaveDecisionSeats?.document?.()?.seats || [])
      .filter((seat) => seat.kind !== IMAGE);
    let taskSeats = [];
    if (decisionSeats.length) {
      taskSeats = decisionSeats.map((seat) => ({
        id: freshId(taken),
        kind: seat.kind,
        role: seat.role,
        description: seat.description,
        access: seat.access,
        difficulty: seat.difficulty,
        candidates: convertPicksToT3(seat, pool),
      }));
    } else {
      const orch = S.orchestrationShown?.();
      if (orch && S.roleCatalog?.().length) {
        const live = S.liveSeats(orch);
        for (const entry of S.roleCatalog()) {
          if (!live.some(({ role }) => role === entry.group)) continue;
          for (const difficulty of levelsOf()) {
            const seat = S.orchestrationSeat(orch, entry.group, difficulty);
            if (!seat?.provider || !seat?.model) continue;
            taskSeats.push({
              id: freshId(taken),
              kind: entry.group,
              role: entry.name_zh,
              description: entry.scope_zh || entry.name_zh,
              access: entry.access,
              difficulty,
              candidates: convertPicksToT3(seat, pool),
            });
          }
        }
      }
    }
    const reviewSource = S.seatsShown?.() || S.reviewSeats || {};
    const review = {};
    for (const [role] of reviewRoles()) {
      const seat = reviewSource[role];
      if (!seat?.provider || !seat?.model) continue;
      const converted = convertPicksToT3(seat, pool);
      if (converted.length) review[role] = converted;
    }
    if (!taskSeats.length) {
      const defaults = starterCandidates(pool);
      const explorer = catalogEntry("explorer");
      const worker = catalogEntry("worker");
      taskSeats = [
        {
          id: freshId(taken), kind: "explorer", role: explorer?.name_zh || "通用探索",
          description: explorer?.scope_zh || "通用探索", access: "read", difficulty: "medium",
          candidates: defaults.map(cloneCandidate),
        },
        {
          id: freshId(taken), kind: "worker", role: worker?.name_zh || "通用执行",
          description: worker?.scope_zh || "通用执行", access: "write", difficulty: "medium",
          candidates: defaults.map(cloneCandidate),
        },
      ];
    }
    return fromDocument({ version: 1, enabled: false, random_candidates: false, seats: taskSeats, review });
  }

  // ---- Draft mutations ----

  function mutate(updater, rerender = true) {
    if (locked() || !baseline) return;
    const next = cloneState(shownState());
    updater(next);
    draft = sameJson(canonState(next), canonState(baseline)) ? null : next;
    if (rerender) render();
    S.advancedEdited?.();
  }

  const newCard = (kind) => ({
    cid: `t3c${++cardCount}`,
    kind,
    role: catalogEntry(kind)?.name_zh || "",
    loadedRole: "",
    access: catalogEntry(kind)?.access || "write",
    description: catalogEntry(kind)?.scope_zh || "",
    seats: {},
  });

  function addCard(kind) {
    if (locked() || (kind !== CUSTOM && !catalogEntry(kind))) return;
    if (kind !== CUSTOM && shownState().cards.some((card) => card.kind === kind)) return;
    const card = newCard(kind);
    expanded.add(card.cid);
    mutate((state) => { state.cards.push(card); });
  }

  function removeCard(cid) {
    expanded.delete(cid);
    mutate((state) => { state.cards = state.cards.filter((card) => card.cid !== cid); });
  }

  function editCard(cid, patch, rerender = true) {
    mutate((state) => {
      const card = state.cards.find((one) => one.cid === cid);
      if (card) Object.assign(card, patch);
    }, rerender);
  }

  function setTierCandidates(cid, level, nextList) {
    mutate((state) => {
      const card = state.cards.find((one) => one.cid === cid);
      if (!card) return;
      if (!nextList.length) {
        delete card.seats[level];
      } else {
        const existing = card.seats[level];
        const id = existing?.id || freshId(takenIds());
        card.seats[level] = {
          id,
          candidates: nextList.map(cloneCandidate),
          ...(existing?.description ? { description: existing.description } : {}),
        };
      }
    });
  }

  function setReviewCandidates(role, nextList) {
    mutate((state) => {
      if (!nextList.length) delete state.review[role];
      else state.review[role] = nextList.map(cloneCandidate);
    });
  }

  // ---- Validation ----

  const cardName = (card) => {
    const group = catalogEntry(card.kind);
    return group ? S.roleName(group) : card.role.trim();
  };
  const scopeOf = (card) => {
    const group = catalogEntry(card.kind);
    return group ? L(group.scope_zh, group.scope_en || group.scope_zh) : "";
  };

  function problem() {
    const state = shownState();
    if (!state) return null;
    const doc = toDocument(state);
    if (doc.seats.length > MAX_SEATS) return L(`工位最多 ${MAX_SEATS} 个`, `At most ${MAX_SEATS} seats`);
    const named = new Set();
    for (const [index, card] of state.cards.entries()) {
      const n = index + 1;
      const own = card.kind === CUSTOM;
      const title = own ? card.role.trim() : cardName(card);
      if (own && !title) return L(`T3 工位第 ${n} 张卡片缺少专业`, `T3 seat card ${n} has no specialty`);
      if (own && named.has(title)) return L(`T3 工位有两张自定义卡片的专业都是“${title}”`, `Two custom T3 cards have the specialty “${title}”`);
      if (own) named.add(title);
      if (!Object.keys(card.seats).length) return L(`T3 工位第 ${n} 张卡片（${title}）至少要绑定一档`, `T3 seat card ${n} (${title}) needs at least one level set`);
    }
    if (doc.enabled && !hasCandidates(doc)) {
      return L("开启 T3 编排前，至少要为一个任务工位或评审角色选定候选模型", "Pick at least one candidate model for a task seat or review role before turning on T3 orchestration");
    }
    return null;
  }

  // ---- Network I/O ----

  async function load(waitFor) {
    try {
      const [response] = await Promise.all([
        fetch("/app/t3-seats", { cache: "no-store" }),
        Promise.resolve(waitFor).catch(() => {}),
      ]);
      if (!response.ok) throw new Error(L(`读取失败（${response.status}）`, `Load failed (${response.status})`));
      const loaded = await response.json();
      const keep = changed();
      if (!(loaded.error && keep)) saved = loaded;
      if (loaded.error) S.showNotice(L(`${loaded.error}；请先修复或删除这个文件`, `${loaded.error}; fix or delete this file first`), true);
      if (!keep) {
        draft = null;
        if (saved.document) {
          baseline = fromDocument(saved.document);
          prefilled = false;
        } else if (!saved.error && saved.t3?.detected) {
          if (!prefilled || !baseline) {
            baseline = prefill();
            prefilled = true;
          }
        } else {
          baseline = null;
          prefilled = false;
        }
      }
    } catch (error) {
      S.showNotice(L(`无法读取 T3 Code 设置：${error.message}`, `Failed to load T3 Code settings: ${error.message}`), true);
    }
    render();
    S.renderAdvancedBar?.();
  }

  async function save() {
    const doc = fileDocument();
    const response = await fetch("/app/t3-seats", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-AgentNave-Action": "1" },
      body: JSON.stringify(doc),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok || data.error) throw new Error(data.error || L(`保存失败（${response.status}）`, `Save failed (${response.status})`));
    saved = { ...saved, document: doc, error: null };
    baseline = fromDocument(doc);
    draft = null;
    prefilled = false;
    if (S.orchestration && data.files) {
      S.orchestration.files = data.files;
      if (data.bosses) S.orchestration.bosses = data.bosses;
      S.renderRuleFiles?.();
    }
    render();
    const failed = data.failures || [];
    if (failed.length) {
      throw new Error(L(`已保存，但 ${failed.map((f) => `${f.path}（${f.error}）`).join("、")} 未能写入`, `Saved, but failed to write ${failed.map((f) => `${f.path} (${f.error})`).join(", ")}`));
    }
  }

  function problemCids(list) {
    const bad = new Set();
    const seen = new Map();
    for (const card of list) {
      const own = card.kind === CUSTOM;
      const title = own ? card.role.trim() : cardName(card);
      if (own && !title) bad.add(card.cid);
      if (own && title) {
        if (seen.has(title)) {
          bad.add(card.cid);
          bad.add(seen.get(title));
        } else {
          seen.set(title, card.cid);
        }
      }
      if (!Object.keys(card.seats).length) bad.add(card.cid);
    }
    return bad;
  }

  function discard() {
    expanded.clear();
    draft = null;
    render();
  }

  // ---- Dropdown & row rendering ----

  const unlistedText = (text) => L(`${text}（T3 未列出）`, `${text} (Not listed in T3)`);

  function visibleModelsOf(inst, currentSlug) {
    return (inst?.models || []).filter((m) => (!m.legacy && !m.hidden) || m.slug === currentSlug);
  }

  function freeModelsOf(inst, taken) {
    const visible = (inst?.models || []).filter((m) => !m.legacy && !m.hidden);
    const free = visible.filter((m) => !taken.has(pairKey(inst.instance, m.slug)));
    return free.length ? free : (inst?.models || []).filter((m) => !taken.has(pairKey(inst.instance, m.slug)));
  }

  function preferredModel(inst, taken) {
    const free = freeModelsOf(inst, taken);
    if (!free.length) return null;
    return free.find((m) => m.default && !m.legacy && !m.hidden)
      || free.find((m) => !m.legacy && !m.hidden)
      || free[0];
  }

  function freshCandidate(taken, ownInstance) {
    const pool = [...eligibleInstances()].sort((a, b) => (a.instance === ownInstance) - (b.instance === ownInstance));
    for (const inst of pool) {
      const model = preferredModel(inst, taken);
      if (model) return { instance: inst.instance, model: model.slug, options: [] };
    }
    return null;
  }

  function makeSelect(key, label, value, options, onPick, disabled = false) {
    return el("select", {
      dataset: { focusKey: key }, "aria-label": label, disabled,
      onChange: (e) => onPick(e.target.value),
    }, options.map(([v, text, off]) => el("option", { value: v, selected: v === value, disabled: Boolean(off) }, text)));
  }

  function candidateControls(key, rowName, candidate, taken, onChange, unsetLabel) {
    const all = instances();
    const savedRefs = savedReferencedInstances();
    const curInstId = candidate?.instance || "";
    const curInst = curInstId ? all.find((x) => x.instance === curInstId) : null;
    const listedInstances = all.filter((inst) => !inst.stale || savedRefs.has(inst.instance) || inst.instance === curInstId);

    const instOptions = [];
    if (unsetLabel !== null) instOptions.push(["", unsetLabel, false]);
    for (const inst of listedInstances) {
      const isCurrent = inst.instance === curInstId;
      const suffix = inst.stale ? L("（T3 未列出）", " (Not listed in T3)")
        : !inst.available ? L("（不可用）", " (unavailable)") : "";
      const unusable = inst.stale || !inst.available || !freeModelsOf(inst, taken).length;
      instOptions.push([inst.instance, `${inst.name}${suffix}`, unusable && !isCurrent]);
    }
    if (curInstId && !curInst) instOptions.push([curInstId, unlistedText(curInstId), false]);

    const onPickInstance = (nextId) => {
      if (!nextId) return onChange(null);
      const inst = all.find((x) => x.instance === nextId);
      const model = inst ? preferredModel(inst, taken) : null;
      if (!model) return;
      onChange({ instance: nextId, model: model.slug, options: [] });
    };

    const curModelSlug = candidate?.model || "";
    const curModelMeta = curInst?.models?.find((m) => m.slug === curModelSlug) || null;
    const modelOptions = [];
    if (!candidate) {
      modelOptions.push(["", "—", false]);
    } else if (!curInst) {
      modelOptions.push([curModelSlug, unlistedText(curModelSlug), false]);
    } else {
      if (!curModelMeta) modelOptions.push([curModelSlug, unlistedText(curModelSlug), false]);
      for (const m of visibleModelsOf(curInst, curModelSlug)) {
        const disabled = taken.has(pairKey(curInst.instance, m.slug)) && m.slug !== curModelSlug;
        modelOptions.push([m.slug, m.name || m.slug, disabled]);
      }
    }

    const onPickModel = (nextSlug) => {
      if (!candidate || !nextSlug) return;
      const nextMeta = curInst?.models?.find((m) => m.slug === nextSlug);
      const prevOpt = candidate.options?.[0];
      const keepOpt = prevOpt && nextMeta?.reasoning?.id === prevOpt.id
        && nextMeta.reasoning.options?.some((opt) => opt.value === prevOpt.value);
      onChange({ instance: candidate.instance, model: nextSlug, options: keepOpt ? [{ ...prevOpt }] : [] });
    };

    const controls = [
      makeSelect(`${key}/instance`, L(`${rowName}的 T3 账号`, `T3 account for ${rowName}`), curInstId, instOptions, onPickInstance),
      makeSelect(`${key}/model`, L(`${rowName}的模型`, `Model for ${rowName}`), curModelSlug, modelOptions, onPickModel, !candidate),
    ];

    const reasoning = curModelMeta?.reasoning || null;
    const currentOpt = candidate?.options?.[0] || null;
    if (candidate && (reasoning?.options?.length || currentOpt)) {
      const currentVal = currentOpt?.value || "";
      const reasoningOptions = [["", L("默认", "Default"), false]];
      for (const opt of reasoning?.options || []) reasoningOptions.push([opt.value, opt.label || opt.value, false]);
      if (currentVal && !(reasoning?.options || []).some((opt) => opt.value === currentVal)) {
        reasoningOptions.push([currentVal, unlistedText(currentVal), false]);
      }
      const onPickReasoning = (nextVal) => {
        if (!nextVal) return onChange({ ...candidate, options: [] });
        const optId = reasoning?.id || currentOpt?.id;
        onChange({ ...candidate, options: optId ? [{ id: optId, value: nextVal }] : [] });
      };
      controls.push(makeSelect(
        `${key}/reasoning`,
        L(`${rowName}的推理档`, `Reasoning for ${rowName}`),
        currentVal,
        reasoningOptions,
        onPickReasoning,
      ));
    }
    return controls;
  }

  function candidateBlock(key, labelNode, rowName, list, onSetList, unsetLabel, sub = true) {
    const takenBy = (skip) => new Set(list.filter((_, i) => i !== skip).map((c) => pairKey(c.instance, c.model)));
    const main = list[0] || null;
    const setMain = (next) => {
      if (!next) onSetList([]);
      else onSetList([next, ...list.slice(1)]);
    };
    const mainControls = candidateControls(`${key}/c0`, rowName, main, takenBy(0), setMain, unsetLabel);
    if (main && list.length < MAX_CANDIDATES) {
      const fresh = freshCandidate(takenBy(-1), main.instance);
      mainControls.push(el("button", {
        type: "button", className: "t3-seat-action", disabled: !fresh,
        dataset: { focusKey: `${key}/add` },
        onClick: () => fresh && onSetList([...list, fresh]),
      }, L("添加备选", "Add fallback")));
    }
    const primaryRow = el("div", { className: `s-row${sub ? " is-sub" : ""}` },
      labelNode,
      el("div", { className: "s-seat" }, ...mainControls));
    if (list.length <= 1) return primaryRow;
    const fallbackRows = list.slice(1).map((cand, offset) => {
      const index = offset + 1;
      const fbLabel = L(`备选 ${index}`, `Fallback ${index}`);
      const setFb = (next) => next && onSetList(list.map((one, i) => (i === index ? next : one)));
      const remove = el("button", {
        type: "button", className: "t3-seat-action",
        dataset: { focusKey: `${key}/c${index}/remove` },
        "aria-label": L(`移除${rowName}的${fbLabel}`, `Remove ${fbLabel} of ${rowName}`),
        onClick: () => onSetList(list.filter((_, i) => i !== index)),
      }, L("移除", "Remove"));
      return el("div", { className: "s-row is-sub", dataset: { fallback: "true" } },
        el("span", { className: "s-label" }, fbLabel),
        el("div", { className: "s-seat" },
          ...candidateControls(`${key}/c${index}`, `${rowName} · ${fbLabel}`, cand, takenBy(index), setFb, null),
          remove));
    });
    return el("div", { className: "s-wrap" }, primaryRow, ...fallbackRows);
  }

  function primaryModelName(seat) {
    const first = seat?.candidates?.[0];
    if (!first) return "";
    const inst = instances().find((x) => x.instance === first.instance);
    const meta = inst?.models?.find((m) => m.slug === first.model);
    return meta?.name || (AN.modelName ? AN.modelName(first.model) : first.model);
  }

  function cardSummary(card) {
    return S.summarizeTiers(DIFFICULTIES.map(([level, label]) => [label, primaryModelName(card.seats[level])]));
  }

  function customHeadRow(card, key, title, helpers) {
    return S.customSeatHeadRow({
      key, role: card.role, access: card.access, title, accessOptions: ACCESS, roleMax: ROLE_MAX,
      onRoleInput: (value) => {
        const prevBad = problemCids(shownState()?.cards || []);
        editCard(card.cid, { role: value }, false);
        const nextBad = problemCids(shownState()?.cards || []);
        if ([...nextBad].some((id) => id !== card.cid && !prevBad.has(id) && !expanded.has(id))) return render();
        helpers.setName(value.trim() || L("自定义工位", "Custom seat"), !value.trim());
      },
      onAccessChange: (value) => editCard(card.cid, { access: value }),
    });
  }

  function seatRowDescriptor(card, index, invalidSet) {
    const key = `t3/${card.cid}`;
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
      isInvalid: () => problemCids(shownState()?.cards || []).has(card.cid),
      onRemove: () => removeCard(card.cid),
      renderBody: (helpers) => [
        ...(own ? [customHeadRow(card, key, title, helpers)] : []),
        ...DIFFICULTIES.map(([level, label]) => candidateBlock(
          `${key}/${level}`,
          el("span", { className: "s-label" }, label),
          `${title} · ${label}`,
          card.seats[level]?.candidates || [],
          (nextList) => setTierCandidates(card.cid, level, nextList),
          L("未设置", "Not set"),
          true,
        )),
      ],
    };
  }

  function addMenu(state) {
    return S.seatAddMenu({
      id: "t3-seat-add", focusKey: "t3/add", disabled: toSeats(state.cards).length >= MAX_SEATS,
      taken: new Set(state.cards.map((card) => card.kind)), groups: S.roleCatalog(), onAdd: addCard,
    });
  }

  function errorAlert() {
    return el("div", { className: "s-alert is-bad" },
      el("div", { className: "s-alert-body" },
        el("strong", {}, L("t3-seats.json 存在错误", "t3-seats.json has errors")),
        el("p", {}, saved.error)));
  }

  // The review roles sit on the review page, toggled by the shared General / T3 Code segment control.
  function renderReview() {
    const box = $("t3-review-seats");
    if (!box) return;
    if (!visible()) {
      box.hidden = true;
      box.replaceChildren();
      S.syncScopeViews?.();
      return;
    }
    const focusKey = document.activeElement?.dataset?.focusKey;
    if (saved.error) {
      box.replaceChildren(
        el("h2", { className: "s-title" }, L("评审工位", "Review seats")),
        errorAlert(),
      );
      S.syncScopeViews?.();
      return;
    }
    const state = shownState();
    if (!state) {
      box.replaceChildren();
      S.syncScopeViews?.();
      return;
    }
    const rows = reviewRoles().map(([role, name, hint, unset]) => candidateBlock(
      `t3/review/${role}`,
      el("span", { className: "s-label" }, name, S.settingTip(hint)),
      L(`T3 ${name}评审`, `T3 ${name} review`),
      state.review[role] || [],
      (nextList) => setReviewCandidates(role, nextList),
      unset || L("未设置", "Not set"),
      false,
    ));
    box.replaceChildren(
      el("h2", { className: "s-title" }, L("评审工位", "Review seats")),
      el("div", { className: "s-card" }, ...rows),
    );
    S.syncScopeViews?.();
    if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  function render() {
    renderReview();
    const box = $("t3-seats-section");
    if (!box) return;
    if (!visible()) {
      box.hidden = true;
      box.replaceChildren();
      S.syncScopeViews?.();
      return;
    }
    const focusKey = document.activeElement?.dataset?.focusKey;
    if (saved.error) {
      box.replaceChildren(el("div", { className: "s-group t3-seats" }, errorAlert()));
      S.syncScopeViews?.();
      return;
    }
    const state = shownState();
    if (!state) {
      box.replaceChildren();
      S.syncScopeViews?.();
      return;
    }
    const canEnable = state.enabled || hasCandidates(toDocument(state));
    const switchesCard = el("div", { className: "s-group t3-seats" },
      el("div", { className: "s-card" },
        el("div", { className: "s-row" },
          el("label", { className: "s-label", htmlFor: "t3-enabled" }, L("在 T3 Code 里由 T3 派发", "Dispatch via T3 in T3 Code")),
          el("input", {
            type: "checkbox", id: "t3-enabled", className: "switch-input",
            dataset: { focusKey: "t3/enabled" }, checked: state.enabled, disabled: !canEnable,
            onChange: (e) => mutate((next) => { next.enabled = e.target.checked; }),
          })),
        el("div", { className: "s-row" },
          el("label", { className: "s-label", htmlFor: "t3-random" }, L("同档候选随机选", "Pick randomly among a tier’s candidates")),
          el("input", {
            type: "checkbox", id: "t3-random", className: "switch-input",
            dataset: { focusKey: "t3/random" }, checked: state.random_candidates,
            onChange: (e) => mutate((next) => { next.random_candidates = e.target.checked; }),
          }))));
    const invalidSet = problemCids(state.cards);
    const seatRows = S.renderSeatRows(
      state.cards.map((card, index) => seatRowDescriptor(card, index, invalidSet)),
      { expanded, onToggle: () => render(), groupClass: "t3-seat-list" },
    );
    const addCardGroup = el("div", { className: "s-group" },
      el("div", { className: "s-card" },
        el("div", { className: "s-row" },
          el("label", { className: "s-label", htmlFor: "t3-seat-add" }, L("添加工位", "Add seat")),
          addMenu(state))));
    box.replaceChildren(...[switchesCard, seatRows, addCardGroup].filter(Boolean));
    S.syncScopeViews?.();
    if (focusKey) box.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`)?.focus({ preventScroll: true });
  }

  window.AgentNaveT3Seats = {
    load, render, renderReview, changed, seatsChanged, reviewChanged, problem, save, discard, addCard, prefill,
    document: fileDocument, hasCandidates, visible,
  };
}
