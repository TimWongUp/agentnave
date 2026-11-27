"use strict";


// Every <select> opens its list right below itself, whatever is chosen, instead of the macOS
// menu that lays the chosen item over the box. The <select> still holds the value and fires
// "change", so pages keep using it as before.
{
  const TYPE_MS = 500; // typed characters closer together than this spell one prefix
  let open = null; // { select, menu, items, active }
  let typed = "", typedTimer = 0;
  let menus = 0;
  const ids = new WeakMap(); // select -> its list's id, the same each time it opens
  const leaving = new WeakMap(); // select -> { menu, anim } currently fading out

  const disabled = (item) => item.getAttribute("aria-disabled") === "true";

  function forgetTyped() {
    clearTimeout(typedTimer);
    typed = "";
  }

  function clearLeaving(select) {
    const prev = leaving.get(select);
    if (!prev) return;
    leaving.delete(select);
    prev.anim?.cancel?.();
    prev.menu.remove();
  }

  function close(instant = false) {
    if (!open) return;
    const { select, menu } = open;
    forgetTyped();
    for (const name of ["aria-expanded", "aria-controls", "aria-activedescendant"]) select.removeAttribute(name);
    open = null;
    const motion = globalThis.ANMotion;
    if (instant || typeof menu.animate !== "function" || !motion?.visibleAndFocused?.()) {
      menu.remove();
      return;
    }
    clearLeaving(select);
    menu.removeAttribute("id");
    menu.removeAttribute("role");
    menu.setAttribute("aria-hidden", "true");
    menu.inert = true;
    menu.style.pointerEvents = "none";
    const dur = motion.ms("--dur-out", 140);
    const anim = menu.animate([{ opacity: 1 }, { opacity: 0 }], { duration: dur || 90, easing: motion.EASE_OUT, fill: "forwards" });
    const entry = { menu, anim };
    leaving.set(select, entry);
    const finish = () => {
      if (leaving.get(select) === entry) leaving.delete(select);
      menu.remove();
    };
    anim.onfinish = finish;
    anim.oncancel = finish;
  }

  function choose(index) {
    const { select, items } = open;
    close();
    if (index < 0 || disabled(items[index]) || index === select.selectedIndex) return;
    select.selectedIndex = index;
    select.dispatchEvent(new Event("input", { bubbles: true }));
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function highlight(index) {
    open.active = index;
    open.items.forEach((item, i) => item.classList.toggle("active", i === index));
    const item = open.items[index];
    if (!item) return open.select.removeAttribute("aria-activedescendant");
    open.select.setAttribute("aria-activedescendant", item.id);
    item.scrollIntoView({ block: "nearest" });
  }

  // Typed characters move the highlight as in a native list; only Enter or a click chooses.
  function typeahead(character) {
    clearTimeout(typedTimer);
    typed += character.toLocaleLowerCase();
    typedTimer = setTimeout(forgetTyped, TYPE_MS);
    // One letter typed again and again steps through the options starting with it.
    const [first, ...rest] = typed;
    const repeated = rest.every((c) => c === first);
    const prefix = repeated ? first : typed;
    const from = Math.max(0, open.active + (repeated ? 1 : 0));
    const count = open.items.length;
    for (let step = 0; step < count; step++) {
      const index = (from + step) % count, item = open.items[index];
      if (!disabled(item) && item.textContent.trim().toLocaleLowerCase().startsWith(prefix)) return highlight(index);
    }
  }

  function createOption(select, option, group, index, id) {
    const item = document.createElement("div");
    item.className = "select-option";
    item.id = `${id}-option-${index}`;
    item.setAttribute("role", "option");
    item.textContent = option.textContent;
    if (index === select.selectedIndex) item.setAttribute("aria-selected", "true");
    if (option.disabled || group?.disabled) item.setAttribute("aria-disabled", "true");
    item.addEventListener("mousedown", (e) => e.preventDefault());
    item.addEventListener("click", () => choose(index));
    item.addEventListener("mousemove", () => open && open.active !== index && highlight(index));
    return item;
  }

  function createGroup(label) {
    const box = document.createElement("div");
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", label);
    const heading = document.createElement("div");
    heading.className = "select-group-label";
    heading.setAttribute("aria-hidden", "true");
    heading.textContent = label;
    box.append(heading);
    return box;
  }

  // The options in the box's order, each <optgroup> under its heading. Only options are items,
  // so keys and the pointer pass over headings.
  function fill(menu, select) {
    const items = [];
    const add = (parent, option, group) => {
      const item = createOption(select, option, group, items.length, menu.id);
      items.push(item);
      parent.append(item);
    };
    for (const child of select.children) {
      if (child.tagName === "OPTION") add(menu, child);
      if (child.tagName !== "OPTGROUP") continue;
      const box = createGroup(child.label);
      for (const option of child.children) add(box, option, child);
      menu.append(box);
    }
    return items;
  }

  function show(select) {
    close();
    clearLeaving(select);
    if (!ids.has(select)) ids.set(select, `select-menu-${++menus}`);
    const menu = document.createElement("div");
    menu.className = "select-menu";
    menu.id = ids.get(select);
    menu.setAttribute("role", "listbox");
    const items = fill(menu, select);
    // A modal dialog sits in the top layer: a list outside it would be drawn under it.
    (select.closest("dialog[open]") || document.body).append(menu);
    const box = select.getBoundingClientRect();
    const below = innerHeight - box.bottom - 8;
    const above = box.top - 8;
    // Below the box unless it fits only above; never over the box itself.
    const up = menu.offsetHeight > below && above > below;
    menu.setAttribute("data-side", up ? "up" : "down");
    menu.style.minWidth = `${box.width}px`;
    menu.style.maxHeight = `${Math.max(120, up ? above : below) - 4}px`;
    menu.style.left = `${Math.max(8, Math.min(box.left, innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = up ? `${box.top - 4 - menu.offsetHeight}px` : `${box.bottom + 4}px`;
    select.setAttribute("aria-expanded", "true");
    select.setAttribute("aria-controls", menu.id);
    open = { select, menu, items, active: select.selectedIndex };
    highlight(select.selectedIndex);
    // Native macOS menu cadence: crisp opacity appearance, no heavy bounce or lag.
    if (typeof menu.animate === "function" && globalThis.ANMotion?.visibleAndFocused?.()) {
      menu.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 80, easing: globalThis.ANMotion.EASE_OUT });
    }
  }

  const usable = (target) => target instanceof HTMLSelectElement && !target.disabled && !target.multiple && target.size <= 1;

  document.addEventListener("mousedown", (e) => {
    // Keep focus on the box, so its scrollbar can be dragged.
    if (open && open.menu.contains(e.target)) return e.preventDefault();
    if (!usable(e.target) || e.button !== 0) return close();
    e.preventDefault();
    e.target.focus({ preventScroll: true });
    if (open?.select === e.target) close();
    else show(e.target);
  }, true);

  document.addEventListener("keydown", (e) => {
    if (!open) {
      if (usable(e.target) && [" ", "Enter", "ArrowDown", "ArrowUp"].includes(e.key)) {
        e.preventDefault();
        show(e.target);
      }
      return;
    }
    if (e.target && e.target !== open.select && (e.target.isContentEditable || ["INPUT", "TEXTAREA"].includes(e.target.tagName))) {
      close();
      return;
    }
    const enabled = open.items.map((_, i) => i).filter((i) => !disabled(open.items[i]));
    const step = (direction) => {
      const next = direction > 0 ? enabled.find((i) => i > open.active) : enabled.filter((i) => i < open.active).pop();
      if (next !== undefined) highlight(next);
    };
    const keys = {
      ArrowDown: () => step(1),
      ArrowUp: () => step(-1),
      Home: () => enabled.length && highlight(enabled[0]),
      End: () => enabled.length && highlight(enabled[enabled.length - 1]),
      Enter: () => choose(open.active),
      " ": () => choose(open.active),
      Escape: () => close(false),
      Tab: () => close(false),
    };
    // A space inside a typed prefix is part of it; alone it chooses.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && (e.key !== " " || typed)) {
      e.preventDefault();
      return typeahead(e.key);
    }
    // A key combination would change the box underneath the open list.
    if (!keys[e.key]) return e.key.length === 1 && e.preventDefault();
    if (e.key !== "Tab") e.preventDefault();
    forgetTyped();
    keys[e.key]();
  }, true);

  // The list belongs to where the box was; moving the page or window closes it immediately.
  addEventListener("scroll", (e) => open && !open.menu.contains(e.target) && close(true), true);
  addEventListener("resize", () => close(true));
  addEventListener("blur", () => close(true));
  document.addEventListener("focusout", (e) => open?.select === e.target && close(true));
}
