// Devices page: each device's own settings, the things that stay put whatever effect it
// runs. The list shows only what differs from the defaults; clicking a device opens its
// settings in a side panel, Ctrl/Shift-click selects several for changing them together, and
// dragging moves a device into another group. Effects and timing are set on the Lighting page.

let devicesKey = "";
let devicesFocus = null;
const devSel = new Set();
let devAnchor = null;
let devPanel = null;

const KIND_NAMES = { header: "ARGB header", ram: "RAM", gpu: "GPU", keyboard: "Keyboard", mouse: "Mouse", headset: "Headset", motherboard: "Motherboard" };
const kindName = (l) => KIND_NAMES[l.kind] || l.kind.charAt(0).toUpperCase() + l.kind.slice(1);
const POLYMO_NAMES = { logo: "ROG logo", diamonds: "diamonds", both: "logo and diamonds" };
const devLight = (id) => (last ? last.lights.find((l) => l.id === id) : null);

function openDeviceSettings(id) {
  devicesFocus = id;
  devicesKey = "";
  showPage("devices");
  if (last) renderDevices(last);
  openDevPanel(id);
}

/// Tags for the settings where a device isn't on the default.
function deviceTags(l) {
  const tags = [];
  if (!l.sync) tags.push(["Not in All Devices", "off"]);
  if (l.hidden) tags.push(["Hidden from Lighting", ""]);
  if (l.warmth != null) tags.push([warmthLabel(l.warmth), ""]);
  if (l.polymo) tags.push([`Polymo: ${POLYMO_NAMES[l.polymo]}`, ""]);
  if (l.zones && l.zones.length) tags.push([`${l.zones.map((z) => z.leds).join(" + ")} LEDs`, ""]);
  return tags.map(([text, kind]) => el("span", { class: "tag" + (kind ? " " + kind : "") }, text));
}

function renderDevices(o) {
  for (const id of [...devSel]) if (!o.lights.some((l) => l.id === id)) devSel.delete(id);
  const key = JSON.stringify([o.lights, o.groups, o.warmth, devicesFocus, [...devSel]]);
  if (key !== devicesKey && !document.querySelector("#device-list .dragging")) {
    devicesKey = key;
    drawDeviceList(o);
  }
  renderSelectionBar();
  if (devPanel && !document.querySelector("#dev-panel input:focus, #dev-panel select:focus")) drawDevPanel();
}

function drawDeviceList(o) {
  const root = $("#device-list");
  if (!o.lights.length) return root.replaceChildren(el("div", { class: "empty" }, "Waiting for devices…"));
  // every group is listed, empty ones too, so devices can be dragged into it
  const sections = [
    ...o.groups.map((g) => [g, o.lights.filter((l) => l.group === g.id)]),
    [null, o.lights.filter((l) => !l.group || !o.groups.some((g) => g.id === l.group))],
  ];
  root.replaceChildren(...sections.map(([g, lights]) => {
    const more = el("button", { class: "icon-btn more-btn", title: "Change the whole group" }, "⋯");
    more.addEventListener("click", (e) => devGroupMenu(g, lights, more));
    const section = el("div", { class: "dev-section" }, [
      el("div", { class: "dev-section-head" }, [el("h2", {}, g ? g.name : "Not in a group"), lights.length ? more : null].filter(Boolean)),
      el("div", { class: "dev-list" }, lights.length ? lights.map(deviceSettingsRow) : [el("div", { class: "dev-empty muted small" }, "Drag devices here")]),
    ]);
    dropZone(section, g ? g.id : null);
    return section;
  }));
  const focused = devicesFocus && root.querySelector(`[data-id="${CSS.escape(devicesFocus)}"]`);
  if (focused) {
    focused.scrollIntoView({ block: "center" });
    focused.classList.add("flash");
    devicesFocus = null;
  }
}

function deviceSettingsRow(l) {
  const row = el("div", { class: "card dev-row" + (l.hidden ? " is-hidden" : "") + (devSel.has(l.id) ? " selected" : "") + (devPanel === l.id ? " open" : ""), "data-id": l.id, draggable: "true", title: "Click for its settings · Ctrl or Shift-click to select several · drag into another group" }, [
    el("span", { class: "seq-handle" }, "⋮⋮"),
    el("div", { class: "dev-name" }, [el("div", { class: "row-name" }, l.name), el("div", { class: "row-sub" }, l.detail || kindName(l))]),
    el("div", { class: "dev-tags" }, deviceTags(l)),
  ]);
  row.addEventListener("click", (e) => {
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
      return openDevPanel(l.id);
    }
    devicesKey = "";
    renderDevices(last);
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

function renderSelectionBar() {
  const bar = $("#dev-selbar");
  const ids = [...devSel];
  bar.hidden = !ids.length;
  if (!ids.length) return;
  const lights = ids.map(devLight).filter(Boolean);
  const allIn = lights.every((l) => l.sync);
  const allShown = lights.every((l) => !l.hidden);
  bar.replaceChildren(
    el("span", { class: "dev-selcount" }, `${ids.length} selected`),
    el("button", { class: "btn mini", onclick: () => devCall(ids, "set_sync", { sync: !allIn }, allIn ? "Taken out of All Devices" : "Back in All Devices") }, allIn ? "Take out of All Devices" : "Put in All Devices"),
    el("button", { class: "btn mini", onclick: () => devCall(ids, "set_hidden", { hidden: allShown }, allShown ? "Hidden from Lighting" : "Shown on Lighting") }, allShown ? "Hide from Lighting" : "Show on Lighting"),
    el("button", { class: "btn mini", onclick: () => bulkWarmthDialog(ids, `${ids.length} devices`) }, "Color temperature…"),
    el("button", { class: "btn mini", onclick: () => ids.forEach((id) => call("identify", { id }).catch(() => {})) }, "Identify"),
    el("span", { class: "grow" }),
    el("button", { class: "btn mini ghost", onclick: () => { devSel.clear(); devicesKey = ""; renderDevices(last); } }, "Clear"),
  );
}

function devGroupMenu(g, lights, anchor) {
  const ids = lights.map((l) => l.id);
  const name = g ? g.name : "these devices";
  openMenu(anchor, [
    { label: "Select all", onClick: () => { ids.forEach((id) => devSel.add(id)); devicesKey = ""; renderDevices(last); } },
    { label: "Take all out of All Devices", onClick: () => devCall(ids, "set_sync", { sync: false }, `${name}: out of All Devices`) },
    { label: "Put all in All Devices", onClick: () => devCall(ids, "set_sync", { sync: true }, `${name}: in All Devices`) },
    { label: "Color temperature for all…", onClick: () => bulkWarmthDialog(ids, name) },
  ]);
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

function openDevPanel(id) {
  devPanel = id;
  const panel = $("#dev-panel");
  panel.hidden = false;
  $("#dev-panel-bg").hidden = false;
  requestAnimationFrame(() => panel.classList.add("open"));
  drawDevPanel();
  devicesKey = "";
  if (last) renderDevices(last);
}

function closeDevPanel() {
  if (!devPanel) return;
  devPanel = null;
  $("#dev-panel").classList.remove("open");
  $("#dev-panel-bg").hidden = true;
  setTimeout(() => {
    if (!devPanel) $("#dev-panel").hidden = true;
  }, 200);
  devicesKey = "";
  if (last) renderDevices(last);
}

function settingRow(title, note, control) {
  return el("div", { class: "dev-setting" }, [
    el("div", {}, [el("div", {}, title), note ? el("div", { class: "muted small" }, note) : null].filter(Boolean)),
    control,
  ]);
}

function drawDevPanel() {
  const l = devLight(devPanel);
  if (!l) return closeDevPanel();
  $("#dp-title").textContent = l.name;
  $("#dp-sub").textContent = l.detail || kindName(l);

  const name = el("input", { class: "text", value: l.name, maxlength: "40" });
  const saveName = () => {
    const value = name.value.trim();
    if (value && value !== l.name) call("rename", { id: l.id, name: value }, "Renamed").then(rerender);
  };
  name.addEventListener("change", saveName);
  name.addEventListener("keydown", (e) => e.key === "Enter" && name.blur());

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
    settingRow("Name", null, name),
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
    const polymo = segmented([["", "Plain"], ["logo", "ROG logo"], ["diamonds", "Diamonds"], ["both", "Both"]], l.polymo || "", (v) =>
      call("set_polymo", { id: l.id, polymo: v || null }, "Polymo updated").then(rerender)
    );
    body.push(el("div", { class: "dev-setting column" }, [
      el("div", {}, [el("div", {}, "Polymo panel"), el("div", { class: "muted small" }, "This header drives the board's Polymo panel: the picture that shows the lighting")]),
      polymo,
    ]));
  }
  $("#dp-body").replaceChildren(...body);
  $("#dp-identify").onclick = () => call("identify", { id: l.id }, `Blinking ${l.name}`);
  $("#dp-effect").onclick = () => {
    closeDevPanel();
    showPage("lighting");
    openDrawer({ type: "dev", id: l.id });
  };
}

$("#dp-close").addEventListener("click", closeDevPanel);
closeOnBackdrop($("#dev-panel-bg"), closeDevPanel);
document.addEventListener("keydown", (e) => e.key === "Escape" && devPanel && closeDevPanel());
document.addEventListener("page-changed", (e) => e.detail !== "devices" && closeDevPanel());

$("#setup-again").addEventListener("click", () => openWizard());

$("#dev-group-new").addEventListener("click", () =>
  promptName("New group", "", async (name) => {
    await call("create_group", { name: name || "New group" }, "Group created: drag devices into it");
    rerender();
  })
);
