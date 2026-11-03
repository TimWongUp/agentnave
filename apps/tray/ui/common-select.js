"use strict";


// Every <select> opens its list right below itself, whatever is chosen, instead of the macOS
// menu that lays the chosen item over the box. The <select> still holds the value and fires
// "change", so pages keep using it as before.
{
  let open = null; // { select, menu, items, active }

  function close() {
    if (!open) return;
    open.menu.remove();
    open.select.removeAttribute("aria-expanded");
    open = null;
  }

  function choose(index) {
    const { select } = open;
    close();
    if (index < 0 || select.options[index].disabled || index === select.selectedIndex) return;
    select.selectedIndex = index;
    select.dispatchEvent(new Event("input", { bubbles: true }));
    select.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function highlight(index) {
    open.active = index;
    open.items.forEach((item, i) => item.classList.toggle("active", i === index));
    open.items[index]?.scrollIntoView({ block: "nearest" });
  }

  function show(select) {
    close();
    const menu = document.createElement("div");
    menu.className = "select-menu";
    menu.setAttribute("role", "listbox");
    const items = [...select.options].map((option, index) => {
      const item = document.createElement("div");
      item.className = "select-option";
      item.setAttribute("role", "option");
      item.textContent = option.textContent;
      if (index === select.selectedIndex) item.setAttribute("aria-selected", "true");
      if (option.disabled) item.setAttribute("aria-disabled", "true");
      item.addEventListener("mousedown", (e) => e.preventDefault());
      item.addEventListener("click", () => choose(index));
      item.addEventListener("mousemove", () => open && open.active !== index && highlight(index));
      menu.append(item);
      return item;
    });
    document.body.append(menu);
    const box = select.getBoundingClientRect();
    const below = innerHeight - box.bottom - 8;
    const above = box.top - 8;
    // Below the box unless it fits only above; never over the box itself.
    const up = menu.offsetHeight > below && above > below;
    menu.style.minWidth = `${box.width}px`;
    menu.style.maxHeight = `${Math.max(120, up ? above : below) - 4}px`;
    menu.style.left = `${Math.max(8, Math.min(box.left, innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = up ? `${box.top - 4 - menu.offsetHeight}px` : `${box.bottom + 4}px`;
    select.setAttribute("aria-expanded", "true");
    open = { select, menu, items, active: select.selectedIndex };
    highlight(select.selectedIndex);
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
    const enabled = open.items.map((_, i) => i).filter((i) => !open.select.options[i].disabled);
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
      Escape: close,
      Tab: close,
    };
    // A typed letter would change the box underneath the open list.
    if (!keys[e.key]) return e.key.length === 1 && e.preventDefault();
    if (e.key !== "Tab") e.preventDefault();
    keys[e.key]();
  }, true);

  // The list belongs to where the box was; moving the page or window closes it.
  addEventListener("scroll", (e) => open && !open.menu.contains(e.target) && close(), true);
  addEventListener("resize", close);
  addEventListener("blur", close);
  document.addEventListener("focusout", (e) => open?.select === e.target && close());
}
