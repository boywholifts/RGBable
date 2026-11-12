// Devices page: each device's own settings, the things that stay put whatever effect it
// runs. The list shows only what differs from the defaults; clicking a device opens its
// settings in a side panel, Ctrl/Shift-click selects several for changing them together, and
// dragging moves a device into another group. Effects and timing are set on the Lighting page.

let devicesKey = "";
let devicesFocus = null;
const devSel = new Set();
let devAnchor = null;
let devPanel = null;
let devShowHidden = false;
let devGroupPick = undefined; // a group id, or null for "Not in a group", when a group is shown

const KIND_NAMES = { header: "ARGB header", ram: "RAM", gpu: "GPU", keyboard: "Keyboard", mouse: "Mouse", headset: "Headset", motherboard: "Motherboard" };
const kindName = (l) => KIND_NAMES[l.kind] || l.kind.charAt(0).toUpperCase() + l.kind.slice(1);
const POLYMO_NAMES = { logo: "ROG logo", diamonds: "HERO logo", both: "both logos" };
const devLight = (id) => (last ? last.lights.find((l) => l.id === id) : null);

function openDeviceSettings(id) {
  devicesFocus = id;
  devPanel = id;
  devSel.clear();
  showPage("devices");
  devRerender();
}

/// Tags for the settings where a device isn't on the default.
function deviceTags(l) {
  const tags = [];
  if (l.hidden) return tags;
  if (!l.sync) tags.push(["Not in All Devices", "off"]);
  if (l.hidden) tags.push(["Hidden from Lighting", ""]);
  if (l.warmth != null) tags.push([warmthLabel(l.warmth), ""]);
  if (l.polymo) tags.push([`Polymo: ${POLYMO_NAMES[l.polymo]}`, ""]);
  if (l.zones && l.zones.length) tags.push([`${l.zones.map((z) => z.leds).join(" + ")} LEDs`, ""]);
  return tags.map(([text, kind]) => el("span", { class: "tag" + (kind ? " " + kind : "") }, text));
}

function renderDevices(o) {
  for (const id of [...devSel]) if (!o.lights.some((l) => l.id === id)) devSel.delete(id);
  const key = JSON.stringify([o.lights, o.groups, o.warmth, devicesFocus, [...devSel], devShowHidden, devPanel, devGroupPick]);
  if (key !== devicesKey && !document.querySelector("#device-list .dragging")) {
    devicesKey = key;
    drawDeviceList(o);
  }
  if (!document.querySelector("#dev-detail input:focus, #dev-detail select:focus")) drawDetail();
}

function devRerender() {
  devicesKey = "";
  if (last) renderDevices(last);
}

/// The right-hand side: one device's settings, what to do with several, a group, or a summary.
let detailKey = "";

/// Redraws the right-hand side only when what it shows changed, keeping where it's scrolled to.
function drawDetail() {
  const shown = devSel.size > 1 ? [...devSel].map(devLight) : devPanel ? devLight(devPanel) : devGroupPick;
  const key = JSON.stringify([shown, devSel.size, devPanel, devGroupPick, last && last.groups, last && last.warmth, devSel.size > 1 || devPanel ? null : last && last.lights.map((l) => [l.name, l.sync, l.hidden])]);
  if (key === detailKey) return;
  detailKey = key;
  const body = $("#dev-detail .drawer-body");
  const scroll = body ? body.scrollTop : 0;
  const same = body && body.dataset.for === String(devSel.size > 1 ? "sel" : devPanel || devGroupPick);
  drawDetailNow();
  const next = $("#dev-detail .drawer-body");
  if (next) {
    next.dataset.for = String(devSel.size > 1 ? "sel" : devPanel || devGroupPick);
    if (same) next.scrollTop = scroll;
  }
}

function drawDetailNow() {
  if (devSel.size > 1) return drawSelection();
  if (devPanel && devLight(devPanel)) return drawDevPanel();
  devPanel = null;
  if (devGroupPick !== undefined) return drawGroup();
  drawSummary();
}

function detailHead(over, title, sub) {
  return el("div", { class: "dev-detail-head" }, [
    el("div", { class: "muted small" }, over),
    el("div", { class: "drawer-title" }, title),
    sub ? el("div", { class: "muted small" }, sub) : null,
  ].filter(Boolean));
}

function drawSummary() {
  const lights = last ? last.lights : [];
  const out = lights.filter((l) => !l.sync && !l.hidden);
  const hidden = lights.filter((l) => l.hidden);
  const list = (title, items) =>
    items.length
      ? el("div", { class: "dev-setting column" }, [el("div", {}, title), el("div", { class: "muted small" }, items.map((l) => l.name).join(", "))])
      : null;
  $("#dev-detail").replaceChildren(
    detailHead("Devices", `${lights.length} device${lights.length === 1 ? "" : "s"}`, "Click a device for its settings, a group's name for the whole group. Ctrl- or Shift-click to change several at once."),
    el("div", { class: "drawer-body" }, [
      list("Not in All Devices", out),
      list("Hidden from the Lighting page", hidden),
      !out.length && !hidden.length ? el("p", { class: "muted small" }, "Every device is in All Devices and shown on the Lighting page.") : null,
    ].filter(Boolean)),
  );
}

function drawDeviceList(o) {
  const root = $("#device-list");
  if (!o.lights.length) return root.replaceChildren(el("div", { class: "empty" }, "Waiting for devices…"));
  const hidden = o.lights.filter((l) => l.hidden);
  const toggle = $("#dev-show-hidden");
  toggle.hidden = !hidden.length;
  toggle.textContent = devShowHidden ? "Hide hidden devices" : `Show hidden (${hidden.length})`;
  const shown = o.lights.filter((l) => !l.hidden);
  // every group is listed, empty ones too, so devices can be dragged into it
  const sections = [
    ...o.groups.map((g) => [g, shown.filter((l) => l.group === g.id)]),
    [null, shown.filter((l) => !l.group || !o.groups.some((g) => g.id === l.group))],
  ].filter(([g, lights]) => g || lights.length || o.groups.length);
  const hiddenSection = devShowHidden && hidden.length
    ? [el("div", { class: "dev-section hidden-section" }, [
        el("div", { class: "dev-section-head static" }, [el("h2", {}, "Hidden"), el("span", { class: "muted small" }, `${hidden.length}`)]),
        el("div", { class: "dev-list" }, hidden.map(deviceSettingsRow)),
      ])]
    : [];
  root.replaceChildren(...sections.map(([g, lights]) => {
    const id = g ? g.id : null;
    const head = el("button", { class: "dev-section-head" + (devGroupPick === id && !devPanel && devSel.size < 2 ? " on" : ""), title: "Settings for the whole group" }, [
      el("h2", {}, g ? g.name : "Not in a group"),
      el("span", { class: "muted small" }, `${lights.length}`),
    ]);
    head.addEventListener("click", () => {
      devGroupPick = id;
      devPanel = null;
      devSel.clear();
      devRerender();
    });
    const section = el("div", { class: "dev-section" }, [
      head,
      el("div", { class: "dev-list" }, lights.length ? lights.map(deviceSettingsRow) : [el("div", { class: "dev-empty muted small" }, "Drag devices here")]),
    ]);
    dropZone(section, g ? g.id : null);
    return section;
  }), ...hiddenSection);
  const focused = devicesFocus && root.querySelector(`[data-id="${CSS.escape(devicesFocus)}"]`);
  if (focused) {
    focused.scrollIntoView({ block: "center" });
    focused.classList.add("flash");
    devicesFocus = null;
  }
}

function deviceSettingsRow(l) {
  const look = l.off ? "#111" : l.effect && l.effect.kind !== "static" ? "linear-gradient(135deg,#ff0040,#ffb000,#40ff40,#00c8ff,#8040ff)" : rgbToHex(l.color || [40, 40, 40]);
  const on = devSel.size > 1 ? devSel.has(l.id) : devPanel === l.id;
  const row = el("div", { class: "dev-row" + (l.hidden ? " is-hidden" : "") + (on ? " selected" : ""), "data-id": l.id, draggable: "true", title: "Click for its settings · Ctrl or Shift-click to select several · drag into another group" }, [
    el("span", { class: "dev-swatch", style: `background:${look}` }),
    el("div", { class: "dev-name" }, [
      el("div", { class: "row-name" }, l.name),
      el("div", { class: "row-sub" }, l.hidden ? "Hidden from the Lighting page" : l.detail || kindName(l)),
      ...(deviceTags(l).length ? [el("div", { class: "dev-tags" }, deviceTags(l))] : []),
    ]),
  ]);
  row.addEventListener("click", (e) => {
    if (devPanel && !devSel.size) devSel.add(devPanel);
    if (e.ctrlKey || e.metaKey) {
      devSel.has(l.id) ? devSel.delete(l.id) : devSel.add(l.id);
      devAnchor = l.id;
    } else if (e.shiftKey && devAnchor) {
      const ids = [...document.querySelectorAll("#device-list .dev-row")].map((r) => r.dataset.id);
      const [a, b] = [ids.indexOf(devAnchor), ids.indexOf(l.id)].sort((x, y) => x - y);
      for (const id of ids.slice(a, b + 1)) devSel.add(id);
    } else {
      devSel.clear();
      devAnchor = l.id;
      devPanel = l.id;
      return devRerender();
    }
    devPanel = devSel.size === 1 ? [...devSel][0] : null;
    if (devSel.size === 1) devSel.clear();
    devRerender();
  });
  row.addEventListener("dragstart", (e) => {
    e.dataTransfer.setData("text/plain", l.id);
    row.classList.add("dragging");
  });
  row.addEventListener("dragend", () => row.classList.remove("dragging"));
  return row;
}

// ---------------------------------------------------------------- several devices at once

const devCall = (ids, cmd, args, done) =>
  Promise.all(ids.map((id) => call(cmd, { id, ...args }))).then(() => {
    if (done) toast(done);
    rerender();
  });

function bulkActions(ids, name) {
  const lights = ids.map(devLight).filter(Boolean);
  const allIn = lights.every((l) => l.sync);
  const allShown = lights.every((l) => !l.hidden);
  return [
    settingRow("In All Devices", allIn ? "All of them are" : "Not all of them are", el("button", { class: "btn mini", onclick: () => devCall(ids, "set_sync", { sync: !allIn }, allIn ? `${name}: out of All Devices` : `${name}: in All Devices`) }, allIn ? "Take all out" : "Put all in")),
    settingRow("On the Lighting page", allShown ? "All of them are shown" : "Some are hidden", el("button", { class: "btn mini", onclick: () => devCall(ids, "set_hidden", { hidden: allShown }, allShown ? `${name}: hidden from Lighting` : `${name}: shown on Lighting`) }, allShown ? "Hide all" : "Show all")),
    settingRow("Color temperature", "One white balance for all of them", el("button", { class: "btn mini", onclick: () => bulkWarmthDialog(ids, name) }, "Set…")),
  ];
}

function drawSelection() {
  const ids = [...devSel];
  $("#dev-detail").replaceChildren(
    detailHead("Selected", `${ids.length} devices`, ids.map((id) => (devLight(id) || {}).name).filter(Boolean).join(", ")),
    el("div", { class: "drawer-body" }, bulkActions(ids, `${ids.length} devices`)),
    el("div", { class: "drawer-foot" }, [
      el("button", { class: "btn", onclick: () => ids.forEach((id) => call("identify", { id }).catch(() => {})) }, "Identify"),
      el("span", { class: "grow" }),
      el("button", { class: "btn ghost", onclick: () => { devSel.clear(); devRerender(); } }, "Clear selection"),
    ]),
  );
}

function drawGroup() {
  const g = devGroupPick && last ? last.groups.find((x) => x.id === devGroupPick) : null;
  if (devGroupPick && !g) {
    devGroupPick = undefined;
    return drawSummary();
  }
  const lights = (last ? last.lights : []).filter((l) => (g ? l.group === g.id : !l.group || !last.groups.some((x) => x.id === l.group)));
  const ids = lights.map((l) => l.id);
  const name = g ? g.name : "Devices not in a group";
  const body = [];
  if (g) {
    const input = el("input", { class: "text", value: g.name, maxlength: "40" });
    input.addEventListener("change", () => input.value.trim() && call("rename_group", { group: g.id, name: input.value.trim() }, "Group renamed").then(rerender));
    input.addEventListener("keydown", (e) => e.key === "Enter" && input.blur());
    body.push(settingRow("Name", null, input));
  }
  if (ids.length) body.push(...bulkActions(ids, name));
  else body.push(el("p", { class: "muted small" }, "No devices yet: drag some into this group."));
  $("#dev-detail").replaceChildren(
    detailHead(g ? "Group" : "", name, `${ids.length} device${ids.length === 1 ? "" : "s"}`),
    el("div", { class: "drawer-body" }, body),
    el("div", { class: "drawer-foot" }, [
      ids.length ? el("button", { class: "btn", onclick: () => ids.forEach((id) => call("identify", { id }).catch(() => {})) }, "Identify all") : null,
      el("span", { class: "grow" }),
      g ? el("button", { class: "btn danger", onclick: () => call("delete_group", { group: g.id }, `Deleted ${g.name}; its devices are ungrouped`).then(() => { devGroupPick = undefined; rerender(); }) }, "Delete group") : null,
    ].filter(Boolean)),
  );
}

/// One white balance for several devices, or back to the global one.
function bulkWarmthDialog(ids, name) {
  const global = last ? last.warmth || 0 : 0;
  const first = devLight(ids[0]);
  const own = el("input", { type: "checkbox" });
  own.checked = !first || first.warmth == null;
  const slider = el("input", { type: "range", min: "-50", max: "50", step: "5", value: String((first && first.warmth) ?? global), style: "flex: 1" });
  const label = el("span", { class: "muted small warmth-value" });
  const draw = () => {
    slider.disabled = own.checked;
    label.textContent = own.checked ? `${warmthLabel(global)} (the global setting)` : warmthLabel(Number(slider.value));
  };
  own.addEventListener("change", draw);
  slider.addEventListener("input", draw);
  draw();
  const save = el("button", { class: "btn primary" }, "Apply");
  save.addEventListener("click", () => {
    closeDialog();
    devCall(ids, "set_device_warmth", { warmth: own.checked ? null : Number(slider.value) }, `Color temperature set for ${name}`);
  });
  openDialog(`Color temperature: ${name}`, [
    el("label", { class: "check" }, [own, "Use the global setting"]),
    el("div", { class: "row" }, [el("span", { class: "muted small" }, "Cooler"), slider, el("span", { class: "muted small" }, "Warmer")]),
    label,
  ], [el("button", { class: "btn", onclick: closeDialog }, "Cancel"), save]);
}

// ---------------------------------------------------------------- one device's panel

function settingRow(title, note, control) {
  return el("div", { class: "dev-setting" }, [
    el("div", {}, [el("div", {}, title), note ? el("div", { class: "muted small" }, note) : null].filter(Boolean)),
    control,
  ]);
}

function drawDevPanel() {
  const l = devLight(devPanel);

  const name = el("input", { class: "text", value: l.name, maxlength: "40" });
  // an empty name goes back to the device's own
  const saveName = () => {
    const value = name.value.trim();
    if (value !== l.name) call("rename", { id: l.id, name: value }, value ? "Renamed" : `Back to ${l.default_name}`).then(rerender);
  };
  name.addEventListener("change", saveName);
  name.addEventListener("keydown", (e) => e.key === "Enter" && name.blur());
  const renamed = l.default_name && l.name !== l.default_name;
  const nameField = el("div", { class: "row dev-name-field" }, [
    name,
    renamed ? el("button", { class: "btn mini ghost", title: `Back to "${l.default_name}"`, onclick: () => call("rename", { id: l.id, name: "" }, `Back to ${l.default_name}`).then(rerender) }, "Reset") : null,
  ].filter(Boolean));

  const toggle = (checked, onChange) => {
    const t = el("input", { type: "checkbox", class: "toggle" });
    t.checked = checked;
    t.addEventListener("change", () => onChange(t.checked));
    return t;
  };

  const global = last ? last.warmth || 0 : 0;
  const ownWarmth = el("input", { type: "checkbox" });
  ownWarmth.checked = l.warmth == null;
  const slider = el("input", { type: "range", min: "-50", max: "50", step: "5", value: String(l.warmth ?? global), style: "flex: 1" });
  const warmthText = el("span", { class: "muted small" });
  let timer = null;
  const drawWarmth = () => {
    slider.disabled = ownWarmth.checked;
    warmthText.textContent = ownWarmth.checked ? `${warmthLabel(global)} (the global setting)` : warmthLabel(Number(slider.value));
  };
  const saveWarmth = () => {
    drawWarmth();
    clearTimeout(timer);
    timer = setTimeout(() => call("set_device_warmth", { id: l.id, warmth: ownWarmth.checked ? null : Number(slider.value) }).then(rerender), 250);
  };
  ownWarmth.addEventListener("change", saveWarmth);
  slider.addEventListener("input", saveWarmth);
  drawWarmth();

  const body = [
    el("div", { class: "dev-setting column" }, [
      el("div", {}, [el("div", {}, "Name"), renamed ? el("div", { class: "muted small" }, `Its own name: ${l.default_name}`) : null].filter(Boolean)),
      nameField,
    ]),
    settingRow("In All Devices", "When off, changing All Devices leaves it alone", toggle(l.sync, (on) => call("set_sync", { id: l.id, sync: on }).then(rerender))),
    settingRow("On the Lighting page", "Hidden devices keep their lighting but aren't listed there", toggle(!l.hidden, (on) => call("set_hidden", { id: l.id, hidden: !on }).then(rerender))),
    el("div", { class: "dev-setting column" }, [
      el("div", {}, [el("div", {}, "Color temperature"), el("div", { class: "muted small" }, "For a device that looks warmer or cooler than the rest")]),
      el("label", { class: "check" }, [ownWarmth, "Use the global setting"]),
      el("div", { class: "row" }, [el("span", { class: "muted small" }, "Cooler"), slider, el("span", { class: "muted small" }, "Warmer")]),
      warmthText,
    ]),
  ];
  if (l.zones && l.zones.length) {
    body.push(el("div", { class: "dev-setting column" }, [
      el("div", {}, [el("div", {}, "LED count"), el("div", { class: "muted small" }, "Raise it until the whole strip lights up")]),
      ...l.zones.map((z) => {
        const input = el("input", { type: "number", class: "text num", min: z.min, max: z.max, value: z.leds });
        input.addEventListener("change", () => call("resize_zone", { id: l.id, zone: z.index, size: Number(input.value) }, `${z.name}: ${input.value} LEDs`).then(rerender));
        return el("div", { class: "zone-row" }, [el("span", { class: "zone-name" }, z.name), input]);
      }),
    ]));
  }
  if (l.polymo_header) {
    const polymo = segmented([["", "Plain"], ["logo", "ROG logo"], ["diamonds", "HERO logo"], ["both", "Both"]], l.polymo || "", (v) =>
      call("set_polymo", { id: l.id, polymo: v || null }, "Polymo updated").then(rerender)
    );
    body.push(el("div", { class: "dev-setting column" }, [
      el("div", {}, [el("div", {}, "Polymo panel"), el("div", { class: "muted small" }, "This header drives the board's Polymo panel: the picture that shows the lighting")]),
      polymo,
    ]));
  }
  $("#dev-detail").replaceChildren(
    detailHead("Device", l.name, l.detail || kindName(l)),
    el("div", { class: "drawer-body" }, body),
    el("div", { class: "drawer-foot" }, [
      el("button", { class: "btn", onclick: () => call("identify", { id: l.id }, `Blinking ${l.name}`) }, "Identify"),
      el("span", { class: "grow" }),
      el("button", { class: "btn primary", onclick: () => { showPage("lighting"); openDrawer({ type: "dev", id: l.id }); } }, "Change its effect…"),
    ]),
  );
}

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || !document.querySelector("#page-devices.active") || document.querySelector(".dialog, .menu")) return;
  devSel.clear();
  devPanel = null;
  devGroupPick = undefined;
  devRerender();
});

$("#setup-again").addEventListener("click", () => openWizard());

$("#dev-group-new").addEventListener("click", () =>
  promptName("New group", "", async (name) => {
    await call("create_group", { name: name || "New group" }, "Group created: drag devices into it");
    rerender();
  })
);

$("#dev-show-hidden").addEventListener("click", () => {
  devShowHidden = !devShowHidden;
  devRerender();
});
