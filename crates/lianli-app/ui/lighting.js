// Lighting page: everything, groups and devices as compact rows. Choosing one opens the
// effect panel on the right, which edits that target and applies it.

const collapsed = new Set(JSON.parse(localStorage.getItem("rgbable.collapsed") || "[]"));

function saveCollapsed() {
  try {
    localStorage.setItem("rgbable.collapsed", JSON.stringify([...collapsed]));
  } catch (e) {}
}

function rerender() {
  structureKey = "";
  refresh();
}

// ---------------------------------------------------------------- popup menu and dialog

function closeMenu() {
  const open = $(".menu");
  if (open) open.remove();
}

/// Where a right-click opened the next menu, so it appears at the pointer.
let menuPoint = null;

/// Right-clicking anything with a ⋯ button opens that button's menu at the pointer. The
/// webview's own menu (reload, inspect) is kept only for text fields.
document.addEventListener("contextmenu", (e) => {
  if (e.target.closest("input, textarea, [contenteditable]")) return;
  e.preventDefault();
  for (let node = e.target; node && node !== document.body; node = node.parentElement) {
    const more = node.matches(".more-btn") ? node : [...node.querySelectorAll(".more-btn")].find((b) => b.parentElement === node || b.parentElement.parentElement === node);
    if (more) {
      if (more.disabled) return;
      menuPoint = { x: e.clientX, y: e.clientY };
      more.click();
      return;
    }
  }
});

/// items: { label, onClick, checked, danger, header } or "-" for a divider.
function openMenu(anchor, items) {
  closeMenu();
  const menu = el("div", { class: "menu" });
  for (const item of items) {
    if (item === "-") {
      menu.append(el("div", { class: "menu-divider" }));
    } else if (item.header) {
      menu.append(el("div", { class: "menu-header" }, item.header));
    } else {
      const button = el("button", { class: "menu-item" + (item.danger ? " danger" : "") }, [
        el("span", { class: "menu-check" }, item.checked ? "✓" : ""),
        item.label,
      ]);
      button.addEventListener("click", (e) => {
        e.stopPropagation();
        closeMenu();
        item.onClick();
      });
      menu.append(button);
    }
  }
  document.body.append(menu);
  const point = menuPoint;
  menuPoint = null;
  const r = point ? { left: point.x, right: point.x + menu.offsetWidth, top: point.y, bottom: point.y } : anchor.getBoundingClientRect();
  const width = menu.offsetWidth, height = menu.offsetHeight;
  menu.style.left = Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8)) + "px";
  menu.style.top = (r.bottom + height + 8 > window.innerHeight ? r.top - height - 4 : r.bottom + 4) + "px";
}

document.addEventListener("click", (e) => {
  if (!e.target.closest(".menu")) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeMenu();
    closeDialog();
    closeDrawer();
  }
});

function closeDialog() {
  const open = $(".dialog-overlay");
  if (open) open.remove();
}

function openDialog(title, body, actions) {
  closeDialog();
  const overlay = el("div", { class: "overlay dialog-overlay" }, [
    el("div", { class: "card dialog" }, [
      el("div", { class: "card-title" }, title),
      ...[].concat(body),
      el("div", { class: "row dialog-actions" }, [el("span", { class: "grow" }), ...actions]),
    ]),
  ]);
  closeOnBackdrop(overlay, closeDialog);
  document.body.append(overlay);
  const first = overlay.querySelector("input, select, textarea");
  if (first) setTimeout(() => first.focus(), 0);
}

function promptName(title, current, onSave) {
  const input = el("input", { class: "text", value: current, maxlength: "40" });
  const save = el("button", { class: "btn primary" }, "Save");
  save.addEventListener("click", () => {
    closeDialog();
    onSave(input.value.trim());
  });
  input.addEventListener("keydown", (e) => e.key === "Enter" && save.click());
  openDialog(title, input, [el("button", { class: "btn", onclick: closeDialog }, "Cancel"), save]);
}

function ledCountDialog(l) {
  const rows = l.zones.map((z) => {
    const input = el("input", { type: "number", class: "text num", min: z.min, max: z.max, value: z.leds });
    const set = el("button", { class: "btn mini" }, "Set");
    set.addEventListener("click", () =>
      call("resize_zone", { id: l.id, zone: z.index, size: Number(input.value) }, `${z.name}: ${input.value} LEDs`).then(rerender)
    );
    input.addEventListener("keydown", (e) => e.key === "Enter" && set.click());
    return el("div", { class: "zone-row" }, [el("span", { class: "zone-name" }, z.name), input, set]);
  });
  openDialog(`LED count · ${l.name}`, [
    el("p", { class: "muted small" }, "How many LEDs are on each addressable header. Raise it until the whole strip lights up."),
    ...rows,
  ], [el("button", { class: "btn", onclick: closeDialog }, "Done")]);
}

function delayDialog(l) {
  const value = el("span", { class: "muted small" });
  const slider = el("input", { type: "range", min: 0, max: 5000, step: 100, value: l.delay_ms || 0 });
  const show = () => (value.textContent = Number(slider.value) ? `${(Number(slider.value) / 1000).toFixed(1)} s behind` : "In step");
  show();
  slider.addEventListener("input", show);
  const save = el("button", { class: "btn primary" }, "Save");
  save.addEventListener("click", async () => {
    closeDialog();
    await call("set_delay", { id: l.id, delayMs: Number(slider.value) }, `${l.name}: ${value.textContent.toLowerCase()}`);
    rerender();
  });
  openDialog(`Start delay · ${l.name}`, [
    el("p", { class: "muted small" }, "Runs this device's effect behind the others, so identical devices (two RAM sticks, a row of fans) don't move in step. Good with Drizzle, Meteor and Wave."),
    el("label", { class: "setting" }, [el("span", { class: "fx-label" }, "Delay"), slider, value]),
  ], [el("button", { class: "btn", onclick: closeDialog }, "Cancel"), save]);
}

// ---------------------------------------------------------------- rows

function effectSummary(l) {
  if (l.outer_effect) return `Inner: ${l.effect ? kindLabelOf(l.effect) : "Color"} · Outer: ${kindLabelOf(l.outer_effect)}`;
  if (l.effect) return kindLabelOf(l.effect) + (l.effect.side && l.effect.side !== "both" ? ` (${l.effect.side} ring)` : "");
  return "Color";
}

function deviceMenu(l, anchor) {
  const groups = last ? last.groups : [];
  const items = [
    { label: "Change effect…", onClick: () => openDrawer({ type: "dev", id: l.id }) },
    { label: "Rename…", onClick: () => promptName(`Rename ${l.name}`, l.name, (name) => call("rename", { id: l.id, name }, "Renamed").then(rerender)) },
    { label: "Identify (blink)", onClick: () => call("identify", { id: l.id }, `Blinking ${l.name}`) },
    { label: "Include in Sync everything", checked: l.sync, onClick: () => call("set_sync", { id: l.id, sync: !l.sync }).then(rerender) },
  ];
  const animated = [l.effect, l.outer_effect].some((e) => e && e.kind !== "static");
  const reversed = [l.effect, l.outer_effect].some((e) => e && e.reverse && e.kind !== "static");
  if (animated) items.push({ label: "Reverse direction", checked: reversed, onClick: () => call("set_reverse", { id: l.id, reverse: !reversed }, reversed ? `${l.name}: normal direction` : `${l.name}: reversed`).then(rerender) });
  if (animated && multiZone(l) && !(l.span && l.span.len < 1000)) {
    const across = !!l.span;
    items.push({ label: "Run across all its fans", checked: across, onClick: () => call("set_across", { id: l.id, across: !across }, across ? `${l.name}: effect on each fan` : `${l.name}: effect across its fans`).then(rerender) });
  }
  items.push({ label: l.delay_ms ? `Start delay (${(l.delay_ms / 1000).toFixed(1)} s)…` : "Start delay…", onClick: () => delayDialog(l) });
  if (l.zones && l.zones.length) items.push({ label: "LED count…", onClick: () => ledCountDialog(l) });
  items.push("-", { header: "Move to group" });
  items.push({ label: "No group", checked: !l.group, onClick: () => call("move_to_group", { id: l.id, group: null }).then(rerender) });
  for (const g of groups) {
    items.push({ label: g.name, checked: l.group === g.id, onClick: () => call("move_to_group", { id: l.id, group: g.id }).then(rerender) });
  }
  items.push({ label: "New group…", onClick: () => promptName("New group", "", async (name) => {
    const id = await call("create_group", { name: name || "New group" });
    await call("move_to_group", { id: l.id, group: id });
    rerender();
  }) });
  openMenu(anchor, items);
}

/// Small marks after a device's name for settings that aren't visible in its color box.
function rowBadges(l) {
  const badges = [];
  const reversed = [l.effect, l.outer_effect].some((e) => e && e.reverse && e.kind !== "static");
  if (reversed) badges.push(el("span", { class: "badge", title: "Effect runs in reverse" }, icon("reverse")));
  if (l.span && l.span.len < 1000) badges.push(el("span", { class: "badge", title: "Part of a chain: moving effects flow through it from the device before" }, icon("chain")));
  else if (l.span) badges.push(el("span", { class: "badge", title: "Moving effects run across all its fans" }, icon("across")));
  if (l.delay_ms) badges.push(el("span", { class: "badge", title: `Starts ${(l.delay_ms / 1000).toFixed(1)} s behind` }, [icon("clock"), `${(l.delay_ms / 1000).toFixed(1)}s`]));
  if (!l.sync) badges.push(el("span", { class: "tag off", title: "Sync everything leaves this device alone" }, "Not synced"));
  return badges;
}

function deviceRow(l) {
  const more = el("button", { class: "icon-btn more-btn", title: "More (or right-click)" }, "⋯");
  more.addEventListener("click", (e) => {
    e.stopPropagation();
    deviceMenu(l, more);
  });
  const box = swatch(l.color, (rgb) => call("set_color", { id: l.id, color: rgb }, `${l.name} updated`).then(rerender), l.effect || l.outer_effect);
  box.addEventListener("click", (e) => e.stopPropagation());
  const row = el("div", { class: "device-row", draggable: "true", "data-id": l.id, title: "Change this device's lighting" }, [
    box,
    el("div", { class: "row-text" }, [
      el("div", { class: "row-name with-badges" }, [el("span", { class: "name-text", title: l.name }, l.name), ...rowBadges(l)]),
      el("div", { class: "row-sub" }, `${l.kind === "header" ? "ARGB header" : l.kind} · ${effectSummary(l)}`),
    ]),
    more,
  ]);
  row.addEventListener("click", () => openDrawer({ type: "dev", id: l.id }));
  row.addEventListener("dragstart", (e) => {
    e.dataTransfer.setData("text/plain", l.id);
    row.classList.add("dragging");
  });
  row.addEventListener("dragend", () => row.classList.remove("dragging"));
  return row;
}

function dropZone(section, group) {
  section.addEventListener("dragover", (e) => {
    e.preventDefault();
    section.classList.add("drop");
  });
  section.addEventListener("dragleave", (e) => {
    if (!section.contains(e.relatedTarget)) section.classList.remove("drop");
  });
  section.addEventListener("drop", (e) => {
    e.preventDefault();
    section.classList.remove("drop");
    const id = e.dataTransfer.getData("text/plain");
    if (id) call("move_to_group", { id, group }).then(rerender);
  });
}

function groupMenu(g, anchor) {
  openMenu(anchor, [
    { label: "Change effect…", onClick: () => openDrawer({ type: "group", id: g.id }) },
    { label: "Rename…", onClick: () => promptName(`Rename ${g.name}`, g.name, (name) => call("rename_group", { group: g.id, name }).then(rerender)) },
    { label: "Identify all (blink)", onClick: () => g.members.forEach((id) => call("identify", { id }).catch(() => {})) },
    "-",
    { label: "Delete group", danger: true, onClick: () => call("delete_group", { group: g.id }, `Deleted ${g.name}; its devices are ungrouped`).then(rerender) },
  ]);
}

function groupSection(key, title, members, g) {
  const open = !collapsed.has(key);
  const toggle = el("button", { class: "chevron" + (open ? " open" : ""), title: open ? "Collapse" : "Expand" }, "›");
  toggle.addEventListener("click", (e) => {
    e.stopPropagation();
    if (open) collapsed.add(key);
    else collapsed.delete(key);
    saveCollapsed();
    rerender();
  });
  const look = groupLook(members, g);
  const head = el("div", { class: "group-head" }, [toggle]);
  if (g) {
    const box = swatch(look.color, (rgb) => call("set_group_color", { group: g.id, color: rgb }, `${g.name} updated`).finally(rerender), look.effect);
    box.addEventListener("click", (e) => e.stopPropagation());
    head.append(box);
  }
  head.append(
    el("div", { class: "row-text" }, [
      el("div", { class: "group-title" }, title),
      el("div", { class: "row-sub" }, `${members.length} device${members.length === 1 ? "" : "s"}${g ? " · " + look.label : ""}`),
    ])
  );
  if (g) {
    const change = el("button", { class: "btn mini" }, "Change effect");
    change.addEventListener("click", (e) => {
      e.stopPropagation();
      openDrawer({ type: "group", id: g.id });
    });
    const more = el("button", { class: "icon-btn more-btn", title: "More (or right-click)" }, "⋯");
    more.addEventListener("click", (e) => {
      e.stopPropagation();
      groupMenu(g, more);
    });
    head.append(change, more);
  }
  head.addEventListener("click", () => toggle.click());
  const section = el("section", { class: "group" + (open ? "" : " closed") }, [head]);
  if (open) {
    section.append(el("div", { class: "rows" }, members.length
      ? members.map(deviceRow)
      : el("div", { class: "empty small" }, "Drag devices here, or use a device's ⋯ menu.")));
  }
  dropZone(section, g ? g.id : null);
  return section;
}

/// The effect shared by every synced device, or null when they differ.
/// What a group's header shows: what its devices are actually running, not the effect the
/// group was last given, since devices can be changed on their own or by Everything.
function groupLook(members, g) {
  if (!g) return {};
  const key = (l) => JSON.stringify([l.effect || null, l.outer_effect || null, l.effect ? null : l.color]);
  const same = members.length && members.every((l) => key(l) === key(members[0]));
  if (!members.length) return { color: g.color, effect: g.effect, label: g.effect ? kindLabelOf(g.effect) : "Color" };
  if (!same) return { color: g.color, effect: { kind: "rainbow", colors: [], speed: 2 }, label: "Mixed" };
  const l = members[0];
  return { color: l.color, effect: l.effect || l.outer_effect, label: effectSummary(l) };
}

function commonEffect(lights) {
  const synced = lights.filter((l) => l.sync);
  if (!synced.length) return null;
  const key = (l) => JSON.stringify([l.effect || null, l.outer_effect || null, l.effect ? null : l.color]);
  const first = key(synced[0]);
  return synced.every((l) => key(l) === first) ? synced[0] : null;
}

function renderEverything(o) {
  const shared = commonEffect(o.lights);
  const synced = o.lights.filter((l) => l.sync).length;
  const card = $("#everything");
  const box = swatch(shared ? shared.color : [128, 0, 255], (rgb) =>
    call("set_all", { effect: { kind: "static", colors: [rgb], speed: 3, brightness: 100, reverse: false }, includeOpenrgb: true }, "Everything updated").then(rerender),
    shared ? shared.effect || shared.outer_effect : { kind: "rainbow", colors: [], speed: 2 });
  box.classList.add("big");
  box.addEventListener("click", (e) => e.stopPropagation());
  card.replaceChildren(
    box,
    el("div", { class: "row-text" }, [
      el("div", { class: "everything-title" }, "Everything"),
      el("div", { class: "row-sub" }, `${synced} of ${o.lights.length} devices in sync · ${shared ? effectSummary(shared) : "Mixed lighting"}`),
    ]),
    el("button", { class: "btn primary", onclick: (e) => { e.stopPropagation(); openDrawer({ type: "all" }); } }, "Change effect")
  );
  card.onclick = () => openDrawer({ type: "all" });
}

function renderLights(o) {
  const orgb = o.openrgb;
  $("#orgb-status").replaceChildren(
    orgb.connected ? `OpenRGB connected` : "OpenRGB not connected · ",
    ...(orgb.connected ? [] : [el("a", { href: "#", onclick: (e) => { e.preventDefault(); reconnect(); } }, "Connect")])
  );
  $("#orgb-status2").textContent = orgb.status;
  const noRam = orgb.connected && !o.lights.some((l) => l.source === "openrgb" && l.kind === "ram");
  $("#ram-hint").hidden = !noRam || o.manage_openrgb;

  const key = JSON.stringify([o.lights, o.groups, orgb.connected, [...collapsed]]);
  if (key === structureKey || document.querySelector("#lights .dragging") || document.querySelector(".menu")) return;
  structureKey = key;
  renderEverything(o);

  const root = $("#lights");
  root.replaceChildren();
  if (!o.lights.length) {
    root.append(el("div", { class: "empty" }, "Waiting for devices…"));
    return;
  }
  const byId = new Map(o.lights.map((l) => [l.id, l]));
  for (const g of o.groups) {
    root.append(groupSection("group:" + g.id, g.name, g.members.map((id) => byId.get(id)).filter(Boolean), g));
  }
  const loose = o.lights.filter((l) => !l.group || !o.groups.some((g) => g.id === l.group));
  if (loose.length || !o.groups.length) {
    root.append(groupSection("ungrouped", o.groups.length ? "Not in a group" : "Devices", loose, null));
  }
  if (drawer.open) renderDrawerTitle();
}

$("#group-new").addEventListener("click", () =>
  promptName("New group", "", async (name) => {
    await call("create_group", { name: name || "New group" }, "Group created: drag devices into it");
    rerender();
  })
);

// ---------------------------------------------------------------- effect panel

const drawer = { open: false, target: null, category: "basic", kind: "static", mode: null, custom: null, side: "both", colors: ["#8000ff"], speed: 3, brightness: 100, reverse: false, direction: null };
const drawerPreview = previewPlayer($("#d-preview"));

function targetLights() {
  if (!last || !drawer.target) return [];
  const t = drawer.target;
  if (t.type === "dev") return last.lights.filter((l) => l.id === t.id);
  if (t.type === "group") {
    const g = last.groups.find((x) => x.id === t.id);
    return g ? last.lights.filter((l) => g.members.includes(l.id)) : [];
  }
  return last.lights.filter((l) => l.sync);
}

function targetName() {
  const t = drawer.target;
  if (!t || !last) return "";
  if (t.type === "dev") return (last.lights.find((l) => l.id === t.id) || {}).name || "Device";
  if (t.type === "group") return (last.groups.find((g) => g.id === t.id) || {}).name || "Group";
  return "Everything";
}

function renderDrawerTitle() {
  $("#d-target").textContent = targetName();
}

function categoryOf(effect) {
  if (!effect) return "basic";
  if (effect.kind === "lianli") return "lianli";
  if (isCustom(effect)) return "mine";
  return (CATEGORIES.find((c) => c.kinds && c.kinds.includes(effect.kind)) || CATEGORIES[0]).id;
}

function loadIntoDrawer(effect, color) {
  drawer.kind = effect ? effect.kind : "static";
  drawer.mode = effect && effect.mode ? effect.mode : null;
  drawer.custom = isCustom(effect) ? effect : null;
  drawer.side = (effect && effect.side) || "both";
  drawer.colors = effect && effect.colors && effect.colors.length ? effect.colors.map(rgbToHex) : [rgbToHex(color || [128, 0, 255])];
  drawer.speed = effect ? effect.speed || 3 : 3;
  drawer.brightness = effect ? effect.brightness ?? 100 : 100;
  drawer.reverse = !!(effect && effect.reverse);
  drawer.direction = effect && effect.direction ? effect.direction : null;
  drawer.category = categoryOf(effect);
}

function openDrawer(target) {
  closeMenu();
  drawer.target = target;
  drawer.open = true;
  drawer.activeColor = 0;
  const lights = targetLights();
  if (target.type === "group") {
    const g = last.groups.find((x) => x.id === target.id);
    loadIntoDrawer(g && g.effect, g && g.color);
  } else if (target.type === "dev" && lights[0]) {
    loadIntoDrawer(lights[0].effect || lights[0].outer_effect, lights[0].color);
  } else {
    const shared = commonEffect(last ? last.lights : []);
    loadIntoDrawer(shared && (shared.effect || shared.outer_effect), shared && shared.color);
  }
  renderDrawerTitle();
  $("#d-openrgb-row").hidden = target.type !== "all";
  $("#drawer").hidden = false;
  $("#drawer-bg").hidden = false;
  requestAnimationFrame(() => $("#drawer").classList.add("open"));
  renderDrawer();
}

function closeDrawer() {
  if (!drawer.open) return;
  drawer.open = false;
  $("#drawer").classList.remove("open");
  $("#drawer-bg").hidden = true;
  drawerPreview.show([], 40);
  setTimeout(() => {
    if (!drawer.open) $("#drawer").hidden = true;
  }, 200);
}

/// What the color, speed and direction controls offer for the chosen effect.
function drawerControls() {
  if (drawer.kind === "custom") return { colors: 0, speed: false, directions: [], reverse: false, hint: "" };
  if (drawer.kind === "script") return { colors: 0, speed: true, directions: [], reverse: false, hint: "" };
  if (drawer.kind === "lianli") {
    const m = lianliInfo(drawer.mode) || { max_colors: 4, directions: [], speed: true };
    return { colors: m.per_fan_colors ? 4 : m.max_colors, speed: m.speed, directions: m.directions, reverse: false, hint: m.per_fan_colors ? "Each fan takes the next color." : "" };
  }
  const info = kindInfo(drawer.kind);
  return { colors: info.colors, speed: drawer.kind !== "static", directions: [], reverse: !!info.reverse, hint: info.hint || "" };
}

function drawerEffect() {
  if (isCustom(drawer.custom) && (drawer.kind === "custom" || drawer.kind === "script")) {
    return { ...drawer.custom, speed: drawer.kind === "script" ? drawer.speed : drawer.custom.speed, brightness: drawer.brightness, side: drawer.side };
  }
  const c = drawerControls();
  const effect = {
    kind: drawer.kind,
    colors: c.colors === 0 ? [] : drawer.colors.map(hexToRgb),
    speed: drawer.speed,
    brightness: drawer.brightness,
    reverse: drawer.reverse,
    side: drawer.side,
  };
  if (drawer.kind === "lianli") {
    effect.mode = drawer.mode;
    if (c.directions.length) effect.direction = drawer.direction && c.directions.includes(drawer.direction) ? drawer.direction : c.directions[0];
  }
  return effect;
}

function tile(label, on, onClick, disabled, extra) {
  const b = el("button", { class: "tile" + (on ? " on" : "") }, [label, ...(extra ? [el("span", { class: "tile-sub" }, extra)] : [])]);
  b.addEventListener("click", onClick);
  if (disabled) {
    b.disabled = true;
    b.title = disabled;
  }
  return b;
}

function renderTiles() {
  const tiles = $("#d-tiles");
  const category = drawer.category;
  let items = [];
  if (category === "lianli") {
    items = LIANLI.map((m) =>
      tile(modeName(m.mode), drawer.kind === "lianli" && drawer.mode === m.mode, () => {
        drawer.kind = "lianli";
        drawer.mode = m.mode;
        drawer.custom = null;
        renderDrawer();
      }, drawer.side !== "both" && !m.side ? "Only runs on both rings together" : null)
    );
    if (!items.length) items = [el("div", { class: "muted small" }, "Lian Li effects aren't available.")];
  } else if (category === "mine") {
    items = MINE.map((e) =>
      tile(e.name, isCustom(drawer.custom) && drawer.custom.name === e.name && drawer.kind === e.kind, () => {
        drawer.kind = e.kind;
        drawer.custom = e;
        drawer.mode = null;
        drawer.speed = e.speed || 3;
        renderDrawer();
      }, null, customType(e))
    );
    items.push(tile("+ New effect", false, () => {
      closeDrawer();
      openBuilder(null);
    }));
  } else {
    const cat = CATEGORIES.find((c) => c.id === category);
    items = cat.kinds.map((id) =>
      tile(kindInfo(id).label, drawer.kind === id, () => {
        drawer.kind = id;
        drawer.mode = null;
        drawer.custom = null;
        const info = kindInfo(id);
        if (info.defaults) drawer.colors = info.defaults.slice();
        renderDrawer();
      })
    );
  }
  tiles.replaceChildren(...items);
}

function hasSides() {
  return targetLights().some((l) => l.sides);
}

let drawerTimer = null;
function savePresets(list) {
  PRESETS = list;
  invoke("set_color_presets", { presets: list }).catch((e) => toast(String(e), true));
}

function activeColor() {
  return Math.min(drawer.activeColor || 0, drawer.colors.length - 1);
}

function useColor(hex, index) {
  drawer.colors[index] = hex;
  drawer.activeColor = index;
  renderDrawer();
}

/// First click on a color box selects it (presets then replace it); a click on the selected box opens the picker.
function selectableColors(row) {
  row.querySelectorAll(".fx-color").forEach((wrap, i) => {
    const box = wrap.querySelector(".swatch");
    wrap.classList.toggle("active", i === activeColor());
    box.addEventListener("click", (e) => {
      if (i === activeColor()) return;
      e.preventDefault();
      drawer.activeColor = i;
      row.querySelectorAll(".fx-color").forEach((w, j) => w.classList.toggle("active", j === i));
    });
    wrap.addEventListener("dragover", (e) => {
      e.preventDefault();
      wrap.classList.add("drop");
    });
    wrap.addEventListener("dragleave", () => wrap.classList.remove("drop"));
    wrap.addEventListener("drop", (e) => {
      e.preventDefault();
      const hex = e.dataTransfer.getData("text/plain");
      if (/^#[0-9a-f]{6}$/i.test(hex)) useColor(hex, i);
    });
  });
  return row;
}

function presetRow() {
  const row = el("div", { class: "presets" });
  if (!editingPresets) {
    for (const hex of PRESETS) {
      const p = el("button", { class: "preset", title: "Click to use for the selected color, or drag onto a color", draggable: "true" });
      p.style.background = hex;
      p.addEventListener("click", () => useColor(hex, activeColor()));
      p.addEventListener("dragstart", (e) => {
        e.dataTransfer.setData("text/plain", hex);
        e.dataTransfer.effectAllowed = "copy";
      });
      row.append(p);
    }
    row.append(el("button", { class: "btn mini ghost", title: "Change these quick colors", onclick: () => { editingPresets = true; renderDrawer(); } }, "Edit"));
    return row;
  }
  const list = PRESETS.slice();
  list.forEach((hex, i) => {
    const input = el("input", { type: "color", value: hex });
    input.addEventListener("change", () => { list[i] = input.value; savePresets(list.slice()); });
    const box = el("label", { class: "preset editable", title: "Pick a color" }, input);
    box.style.background = hex;
    input.addEventListener("input", () => (box.style.background = input.value));
    const wrap = el("div", { class: "fx-color" }, box);
    if (list.length > 1) {
      wrap.append(el("button", { class: "remove", title: "Remove", onclick: () => { list.splice(i, 1); savePresets(list); renderDrawer(); } }, "×"));
    }
    row.append(wrap);
  });
  if (list.length < 12) {
    row.append(el("button", { class: "btn mini", title: "Add the last color above", onclick: () => { list.push(drawer.colors[drawer.colors.length - 1] || "#ffffff"); savePresets(list); renderDrawer(); } }, "+"));
  }
  row.append(
    el("button", { class: "btn mini ghost", title: "Use the colors above as the quick colors", onclick: () => { savePresets(drawer.colors.slice()); renderDrawer(); } }, "Save current"),
    el("button", { class: "btn mini ghost", title: "Back to the built-in colors", onclick: () => { savePresets([]); PRESETS = DEFAULT_PRESETS.slice(); renderDrawer(); } }, "Reset"),
    el("button", { class: "btn mini", onclick: () => { editingPresets = false; renderDrawer(); } }, "Done"),
  );
  return row;
}

function oledColorsRow(max) {
  const saved = Number(localStorage.getItem("rgbable.oledColors")) || 4;
  const count = el("select", { title: "How many colors to take" },
    Array.from({ length: max }, (_, i) => el("option", { value: i + 1 }, `${i + 1} color${i ? "s" : ""}`)));
  count.value = String(Math.min(saved, max));
  count.addEventListener("change", () => localStorage.setItem("rgbable.oledColors", count.value));
  const button = el("button", { class: "btn mini", title: "Take the main colors from what's on the OLED" }, "Colors from OLED");
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "Reading the video…";
    try {
      drawer.colors = await invoke("oled_palette", { file: null, count: Number(count.value) });
      renderDrawer();
    } catch (e) {
      toast(String(e), true);
      button.disabled = false;
      button.textContent = "Colors from OLED";
    }
  });
  return el("div", { class: "oled-colors" }, [button, count]);
}

function renderDrawer() {
  if (!hasSides()) drawer.side = "both";
  $("#d-tabs").replaceChildren(
    ...CATEGORIES.map((c) =>
      el("button", { class: drawer.category === c.id ? "on" : "", onclick: () => { drawer.category = c.id; renderDrawer(); } }, c.label)
    )
  );
  renderTiles();
  const ring = $("#d-ring");
  ring.hidden = !hasSides();
  ring.replaceChildren(
    el("span", { class: "fx-label" }, "SL Flex rings"),
    segmented([["both", "Both"], ["inner", "Inner"], ["outer", "Outer"]], drawer.side, (v) => {
      drawer.side = v;
      if (drawer.kind === "lianli" && v !== "both" && !(lianliInfo(drawer.mode) || {}).side) {
        drawer.kind = "static";
        drawer.mode = null;
        drawer.category = "basic";
      }
      renderDrawer();
    })
  );

  const c = drawerControls();
  drawer.colors = drawer.colors.slice(0, Math.max(1, c.colors));
  const settings = $("#d-settings");
  settings.replaceChildren();
  if (c.colors > 0) {
    settings.append(el("div", { class: "setting" }, [
      el("span", { class: "fx-label" }, "Colors"),
      selectableColors(colorRow(drawer.colors, c.colors, (structural) => (structural ? renderDrawer() : schedulePreview()))),
    ]));
    settings.append(el("div", { class: "setting" }, [el("span", { class: "fx-label" }, ""), presetRow()]));
    settings.append(el("div", { class: "setting" }, [el("span", { class: "fx-label" }, ""), oledColorsRow(c.colors)]));
  }
  if (c.speed) {
    const speed = el("input", { type: "range", min: 1, max: 5, value: drawer.speed });
    speed.addEventListener("input", () => { drawer.speed = Number(speed.value); schedulePreview(); });
    settings.append(el("label", { class: "setting" }, [el("span", { class: "fx-label" }, "Speed"), speed]));
  }
  const bright = el("input", { type: "range", min: 5, max: 100, step: 5, value: drawer.brightness });
  const brightVal = el("span", { class: "muted small" }, drawer.brightness + "%");
  bright.addEventListener("input", () => {
    drawer.brightness = Number(bright.value);
    brightVal.textContent = bright.value + "%";
    schedulePreview();
  });
  settings.append(el("label", { class: "setting" }, [el("span", { class: "fx-label" }, "Brightness"), bright, brightVal]));
  if (c.directions.length) {
    const dir = el("select", {}, c.directions.map((d) => el("option", { value: d }, DIRECTIONS[d] || d)));
    dir.value = c.directions.includes(drawer.direction) ? drawer.direction : c.directions[0];
    dir.addEventListener("change", () => { drawer.direction = dir.value; schedulePreview(); });
    settings.append(el("label", { class: "setting" }, [el("span", { class: "fx-label" }, "Direction"), dir]));
  }
  if (c.reverse) {
    const rev = el("input", { type: "checkbox" });
    rev.checked = drawer.reverse;
    rev.addEventListener("change", () => { drawer.reverse = rev.checked; schedulePreview(); });
    settings.append(el("label", { class: "setting check" }, [el("span", { class: "fx-label" }, ""), rev, "Reverse direction"]));
  }

  const notes = [];
  if (c.hint) notes.push(c.hint);
  if (drawer.side !== "both") notes.push(`Only the ${drawer.side} ring of SL Flex fans changes; other devices get the whole effect.`);
  if (drawer.target && drawer.target.type === "all") notes.push("Devices with Sync off are left alone.");
  $("#d-note").textContent = notes.join(" ");
  $("#d-effect-name").textContent = drawer.kind === "lianli" ? modeName(drawer.mode || "") : isCustom(drawer.custom) && (drawer.kind === "custom" || drawer.kind === "script") ? drawer.custom.name : kindInfo(drawer.kind).label;
  schedulePreview();
}

function schedulePreview() {
  clearTimeout(drawerTimer);
  drawerTimer = setTimeout(() => drawerPreview.load(drawerEffect()), 120);
}

$("#d-close").addEventListener("click", closeDrawer);
$("#d-cancel").addEventListener("click", closeDrawer);
closeOnBackdrop($("#drawer-bg"), closeDrawer);
$("#d-apply").addEventListener("click", async () => {
  const effect = drawerEffect();
  const t = drawer.target;
  const label = $("#d-effect-name").textContent;
  const button = $("#d-apply");
  button.disabled = true;
  try {
    if (t.type === "dev") await call("set_effect", { id: t.id, effect }, `${label} applied to ${targetName()}`);
    else if (t.type === "group") await call("set_group_effect", { group: t.id, effect }, `${label} applied to ${targetName()}`);
    else await call("set_all", { effect, includeOpenrgb: $("#d-openrgb").checked }, `${label} applied to everything`);
    closeDrawer();
  } catch (e) {
  } finally {
    button.disabled = false;
    rerender();
  }
});

document.addEventListener("effects-changed", () => {
  if (drawer.open) renderDrawer();
});
