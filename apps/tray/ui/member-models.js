"use strict";
const isRestricted = (scene) => Object.hasOwn(currentDoc.modes || {}, scene);

// The CLI's one model list, shared by its scenes and seats. Once present it replaces the scene
// lists of older files; null means any model, now or later.
const hasPool = (provider) => Object.hasOwn(currentDoc.clis?.[provider] || {}, "allowed_models");
const listedPool = (provider) => currentDoc.clis?.[provider]?.allowed_models || null;
const outOfPool = (provider, model) => Boolean(listedPool(provider)) && !listedPool(provider).includes(model);

// A model the App starts configuring keeps the CLI's own default effort until one is chosen (ADR-0028).
function ensureCliModel(provider, modelId) {
  if (!currentDoc.clis) currentDoc.clis = {};
  if (!currentDoc.clis[provider]) currentDoc.clis[provider] = { models: {} };
  if (!currentDoc.clis[provider].models) currentDoc.clis[provider].models = {};
  if (!Object.hasOwn(currentDoc.clis[provider].models, modelId)) {
    Object.defineProperty(currentDoc.clis[provider].models, modelId, {
      value: {}, enumerable: true, configurable: true, writable: true,
    });
  }
}

// What the CLI itself uses when the App sets nothing: its own default model and effort, if known.
function nativeDefault(provider) {
  const native = detected[provider]?.native || {};
  return { model: native.model || null, effort: native.effort || null };
}

// The level the CLI runs one model at when a call names none, if known.
function cliEffort(provider, modelId) {
  const listed = (detected[provider]?.models || []).find((m) => m.id === modelId);
  if (listed?.effort) return listed.effort;
  return modelId === nativeDefault(provider).model ? nativeDefault(provider).effort : null;
}

const EFFORT_ORDER = ["off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

// A model's effort levels from low to high: its own detected levels, else its CLI's, plus any
// configured value a probe no longer lists.
function effortLevels(provider, modelId) {
  // A model's own list, even an empty one, is what its CLI offers; the CLI's list is the fallback.
  const own = (detected[provider]?.models || []).find((m) => m.id === modelId)?.efforts;
  const detectedEfforts = Array.isArray(own) ? own : detected[provider]?.efforts || [];
  const shared = currentDoc.clis?.[provider]?.models?.[modelId] || {};
  const overrides = Object.values(currentDoc.modes || {})
    .map((providers) => providers?.[provider]?.overrides?.[modelId])
    .filter(Boolean);
  const levels = new Set(detectedEfforts);
  for (const config of [shared, ...overrides]) {
    for (const effort of [...(config.efforts || []), config.effort]) {
      if (typeof effort === "string" && effort) levels.add(effort);
    }
  }
  const rank = (effort) => {
    const index = EFFORT_ORDER.indexOf(effort);
    return index < 0 ? EFFORT_ORDER.length : index;
  };
  return [...levels].sort((a, b) => rank(a) - rank(b));
}

// The level a chosen model is saved with: the one wanted when it has it, else the CLI's current
// one, else its only one; "" when it has no levels or none can be told, so the user picks.
function savedEffort(provider, modelId, wanted) {
  const levels = modelId ? effortLevels(provider, modelId) : [];
  if (levels.includes(wanted)) return wanted;
  const cli = cliEffort(provider, modelId);
  if (levels.includes(cli)) return cli;
  return levels.length === 1 ? levels[0] : "";
}

function getAllModelIds(provider) {
  const ids = new Set((detected[provider]?.models || []).map((m) => m.id));
  for (const id of Object.keys(currentDoc.clis?.[provider]?.models || {})) ids.add(id);
  for (const selection of Object.values(currentDoc.modes || {})) {
    for (const id of selection?.[provider]?.models || []) ids.add(id);
  }
  return [...ids].filter((id) => typeof id === "string" && id.trim() && !hiddenModel(provider, id));
}

const hiddenModel = (provider, id) => provider === "devin" && /^fusion-/i.test(id);

// A scene's effort for one model: its override, else the shared level; "" follows the CLI.
function sceneEffort(scene, provider, modelId) {
  if (!modelId) return "";
  const override = currentDoc.modes?.[scene]?.[provider]?.overrides?.[modelId];
  if (override && Object.hasOwn(override, "effort")) return override.effort;
  return currentDoc.clis?.[provider]?.models?.[modelId]?.effort || "";
}

// What one scene runs for one CLI: whether it is used, the default model ("" follows the CLI),
// the models boss may pick (null: any), and the default model's effort ("" follows the CLI).
function slot(scene, provider) {
  const selection = isRestricted(scene) ? currentDoc.modes[scene][provider] : null;
  // An open scene runs what restrict() would freeze it to, so editing another CLI changes nothing here.
  const enabled = isRestricted(scene) ? Boolean(selection)
    : Boolean(detected[provider]?.installed || currentDoc.defaults?.[scene]?.[provider]);
  const model = (selection ? selection.default : currentDoc.defaults?.[scene]?.[provider]) || "";
  const pool = hasPool(provider) ? listedPool(provider) : selection?.models;
  const allowed = hasPool(provider) && Array.isArray(pool) ? [...pool] : pool?.length ? [...pool] : null;
  const effort = enabled ? sceneEffort(scene, provider, model || nativeDefault(provider).model) : "";
  return { enabled, model: enabled ? model : "", allowed, effort };
}

// The pool as the card shows it: an older file's scene lists together, unless a scene is open.
function shownPool(provider) {
  if (hasPool(provider)) return listedPool(provider);
  const lists = SCENE_IDS.map((s) => slot(s, provider)).filter((s) => s.enabled).map((s) => s.allowed);
  return lists.length && lists.every(Boolean) ? [...new Set(lists.flat())] : null;
}

// Scenes that show the same model and effort, followed or chosen, need only one row.
function shownSlot(scene, provider) {
  const current = slot(scene, provider);
  // A model list without a default runs nothing until one is chosen.
  const model = current.model || (current.allowed ? "" : nativeDefault(provider).model) || "";
  return { ...current, model, effort: current.effort || (model && cliEffort(provider, model)) || "" };
}
const splitCards = new Set(); // Cards split by hand while both scenes still match.
const sceneOverrides = (scene, provider) => JSON.stringify(currentDoc.modes?.[scene]?.[provider]?.overrides || {});
const isSplit = (provider) => splitCards.has(provider) ||
  JSON.stringify(shownSlot("task", provider)) !== JSON.stringify(shownSlot("discussion", provider)) ||
  sceneOverrides("task", provider) !== sceneOverrides("discussion", provider);

// Keep saved employees in place while their scene membership is being edited.
const known = (provider, saved) => detected[provider]?.installed ||
  SCENE_IDS.some((s) => saved?.modes?.[s]?.[provider] || currentDoc.modes?.[s]?.[provider] || currentDoc.defaults?.[s]?.[provider]);

// Handwritten unrestricted scenes remain readable; the App always saves explicit lists.
function restrict(scene, doc = currentDoc) {
  doc.modes ||= {};
  if (Object.hasOwn(doc.modes, scene)) return doc.modes[scene];
  const kept = doc.defaults?.[scene] || {};
  const selection = {};
  for (const p of PROVIDERS) {
    if (!detected[p]?.installed && !kept[p]) continue;
    selection[p] = { models: [], overrides: {} };
    if (kept[p]) selection[p].default = kept[p];
  }
  doc.modes[scene] = selection;
  if (doc.defaults) {
    delete doc.defaults[scene];
    if (!Object.keys(doc.defaults).length) delete doc.defaults;
  }
  return selection;
}

function confirmScenes(doc = currentDoc) {
  for (const scene of SCENE_IDS) restrict(scene, doc);
  delete doc.defaults;
  return doc;
}

// A model list needs a default model, so following the CLI pins the CLI's own model.
function writeSlot(scene, provider, next) {
  if (!next.enabled) {
    delete restrict(scene)[provider];
    return;
  }
  const native = nativeDefault(provider).model;
  const model = next.model || (next.allowed ? (next.allowed.includes(native) ? native : next.allowed[0]) : "");
  const selection = restrict(scene);
  const conf = (selection[provider] ||= { models: [], overrides: {} });
  conf.models = next.allowed && !hasPool(provider) ? [...new Set([...next.allowed, model])] : [];
  if (model) conf.default = model;
  else delete conf.default;
  for (const m of [model, ...(next.allowed || [])]) if (m) ensureCliModel(provider, m);
}

// A scene that follows the CLI names the CLI's model once a pool has it, as a model list needs a
// default. Any other default stays as it is, reported until the user changes it.
function pinFollowed(provider, conf) {
  const native = nativeDefault(provider).model;
  if (conf.default || !listedPool(provider)?.includes(native)) return;
  conf.default = native;
  const shared = currentDoc.clis[provider].models[native];
  const effort = savedEffort(provider, native, "");
  if (effort && !shared.effort) shared.effort = effort;
}

// Editing the pool takes over from the scene lists, so they cannot narrow it a second time.
function writePool(provider, allowed) {
  for (const m of allowed || []) ensureCliModel(provider, m);
  const cli = ((currentDoc.clis ||= {})[provider] ||= { models: {} });
  if (allowed) cli.allowed_models = [...allowed];
  else delete cli.allowed_models;
  for (const scene of SCENE_IDS) {
    const conf = restrict(scene)[provider];
    if (!conf) continue;
    conf.models = [];
    pinFollowed(provider, conf);
  }
  edited();
}

// One model's effort in both scenes: shared when they agree, else a scene override. An override
// cannot unset the shared level, so a scene that follows the CLI holds the shared one.
function writeEfforts(provider, modelId, wants) {
  if (!modelId) return;
  ensureCliModel(provider, modelId);
  const shared = currentDoc.clis[provider].models[modelId];
  let base = wants.task === wants.discussion || !wants.task ? "task"
    : !wants.discussion || isRestricted("task") ? "discussion" : "task";
  const fits = (level) => !level || !shared.efforts?.length || shared.efforts.includes(level);
  const other = base === "task" ? "discussion" : "task";
  if (!fits(wants[base]) && fits(wants[other])) base = other;
  // A level only a scene's own range has stays with the scenes.
  const sharable = fits(wants[base]);
  if (sharable && wants[base]) shared.effort = wants[base];
  else if (sharable) delete shared.effort;
  for (const scene of SCENE_IDS) {
    if (wants[scene] !== wants[base] || !sharable) {
      const conf = restrict(scene)[provider];
      if (!conf) continue;
      conf.overrides ||= {};
      conf.overrides[modelId] = { ...conf.overrides[modelId], effort: wants[scene] };
      continue;
    }
    const overrides = currentDoc.modes?.[scene]?.[provider]?.overrides;
    if (!overrides?.[modelId]) continue;
    delete overrides[modelId].effort;
    if (!Object.keys(overrides[modelId]).length) delete overrides[modelId];
  }
}

function edited() {
  confirmScenes();
  incDocVersion();
  renderCards();
}

// Change one scene, or both while the card is not split.
function changeSlot(provider, scene, change) {
  if (pendingDisable === provider) pendingDisable = null;
  const scenes = scene ? [scene] : SCENE_IDS;
  // A joined card shows the private-task row, so both scenes take it.
  const shown = slot(scene || "task", provider);
  for (const s of scenes) {
    const next = { ...shown, ...change };
    writeSlot(s, provider, next);
    if (Object.hasOwn(change, "effort")) {
      const modelId = next.model || nativeDefault(provider).model;
      const other = SCENE_IDS.find((o) => o !== s);
      const follows = !scene || !slot(other, provider).enabled;
      writeEfforts(provider, modelId, {
        [s]: change.effort,
        [other]: follows ? change.effort : sceneEffort(other, provider, modelId),
      });
    }
  }
  edited();
}

function setProviderEnabled(provider, enabled, confirmed = false) {
  if (!enabled) {
    const settings = SCENE_IDS.some((s) => {
      const conf = currentDoc.modes?.[s]?.[provider];
      return conf && (conf.models?.length || conf.default || Object.keys(conf.overrides || {}).length);
    }) || SCENE_IDS.some((s) => currentDoc.defaults?.[s]?.[provider]);
    if (settings && !confirmed) {
      pendingDisable = provider;
      return renderCards();
    }
  }
  pendingDisable = null;
  for (const scene of SCENE_IDS) {
    if (!enabled) delete restrict(scene)[provider];
    else pinFollowed(provider, restrict(scene)[provider] ||= { models: [], overrides: {} });
  }
  edited();
}

// Joining the scenes again gives meetings what private tasks have.
function setSplit(provider, split) {
  if (split) {
    splitCards.add(provider);
    return renderCards();
  }
  splitCards.delete(provider);
  writeSlot("task", provider, slot("task", provider));
  const task = slot("task", provider);
  writeSlot("discussion", provider, task);
  const overrides = currentDoc.modes?.task?.[provider]?.overrides || {};
  const joined = task.enabled && Object.keys(overrides).length
    ? restrict("discussion")[provider]
    : currentDoc.modes?.discussion?.[provider];
  if (joined) joined.overrides = structuredClone(overrides);
  const modelId = task.model || nativeDefault(provider).model;
  if (task.enabled) writeEfforts(provider, modelId, { task: task.effort, discussion: task.effort });
  pickerOpen = null;
  edited();
}

let pickerOpen = null; // "provider/pool" whose model list is open
