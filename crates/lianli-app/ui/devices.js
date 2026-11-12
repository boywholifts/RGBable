// Devices page: each device's own settings, the things that stay put whatever effect it
// runs: name, group, LED count, color temperature, whether All Devices changes it, the Polymo
// picture and whether it's hidden. Effects and timing are set on the Lighting page.

let devicesKey = "";
let devicesFocus = null;

const KIND_NAMES = { header: "ARGB header", ram: "RAM", gpu: "GPU", keyboard: "Keyboard", mouse: "Mouse", headset: "Headset", motherboard: "Motherboard" };
const kindName = (l) => KIND_NAMES[l.kind] || l.kind.charAt(0).toUpperCase() + l.kind.slice(1);

function openDeviceSettings(id) {
  devicesFocus = id;
  devicesKey = "";
  showPage("devices");
  if (last) renderDevices(last);
}

function renderDevices(o) {
  const key = JSON.stringify([o.lights, o.groups, o.warmth, devicesFocus]);
  if (key === devicesKey || document.querySelector("#device-list select:focus, #device-list .dragging")) return;
  devicesKey = key;
  const root = $("#device-list");
  if (!o.lights.length) return root.replaceChildren(el("div", { class: "empty" }, "Waiting for devices…"));
  // every group is listed, empty ones too, so devices can be dragged into it
  const sections = [
    ...o.groups.map((g) => [g.name, g.id, o.lights.filter((l) => l.group === g.id)]),
    ["Not in a group", null, o.lights.filter((l) => !l.group || !o.groups.some((g) => g.id === l.group))],
  ];
  root.replaceChildren(...sections.map(([name, group, lights]) => {
    const section = el("div", { class: "dev-section" }, [
      el("h2", {}, name),
      el("div", { class: "dev-list" }, lights.length ? lights.map((l) => deviceSettingsRow(l, o)) : [el("div", { class: "dev-empty muted small" }, "Drag devices here")]),
    ]);
    dropZone(section, group);
    return section;
  }));
  const focused = devicesFocus && root.querySelector(`[data-id="${CSS.escape(devicesFocus)}"]`);
  if (focused) {
    focused.scrollIntoView({ block: "center" });
    focused.classList.add("flash");
    devicesFocus = null;
  }
}

function deviceSettingsRow(l, o) {
  const sync = el("input", { type: "checkbox", class: "toggle" });
  sync.checked = l.sync;
  sync.addEventListener("change", () => call("set_sync", { id: l.id, sync: sync.checked }).then(rerender));

  const shown = el("input", { type: "checkbox", class: "toggle" });
  shown.checked = !l.hidden;
  shown.addEventListener("change", () =>
    call("set_hidden", { id: l.id, hidden: !shown.checked }, shown.checked ? `${l.name} is shown on Lighting` : `${l.name} hidden from Lighting`).then(rerender)
  );

  const warmth = l.warmth == null ? "Global" : warmthLabel(l.warmth);
  const controls = [
    el("label", { class: "dev-field", title: "When off, changing All Devices leaves this device alone" }, [el("span", { class: "muted small" }, "In All Devices"), sync]),
    el("label", { class: "dev-field", title: "Hidden devices keep their lighting but aren't listed on the Lighting page" }, [el("span", { class: "muted small" }, "On Lighting page"), shown]),
    el("button", { class: "btn mini", title: "This device's own white balance", onclick: () => warmthDialog(l) }, ["Color temp: ", warmth]),
  ];
  if (l.zones && l.zones.length) {
    controls.push(el("button", { class: "btn mini", onclick: () => ledCountDialog(l) }, `LEDs: ${l.zones.map((z) => z.leds).join(" + ")}`));
  }
  if (l.polymo_header) {
    const polymo = el("select", { title: "This header drives the board's Polymo panel: which picture shows the lighting" }, [
      el("option", { value: "" }, "Plain header"),
      el("option", { value: "logo" }, "Polymo: ROG logo"),
      el("option", { value: "diamonds" }, "Polymo: diamonds"),
      el("option", { value: "both" }, "Polymo: both"),
    ]);
    polymo.value = l.polymo || "";
    polymo.addEventListener("change", () => call("set_polymo", { id: l.id, polymo: polymo.value || null }, "Polymo updated").then(rerender));
    controls.push(polymo);
  }
  controls.push(el("button", { class: "btn mini ghost", title: "Blink it so you can find it", onclick: () => call("identify", { id: l.id }, `Blinking ${l.name}`) }, "Identify"));

  const row = el("div", { class: "card dev-row" + (l.hidden ? " is-hidden" : ""), "data-id": l.id, draggable: "true" }, [
    el("span", { class: "seq-handle", title: "Drag into another group" }, "⋮⋮"),
    el("button", { class: "dev-name", title: "Rename", onclick: () => promptName(`Rename ${l.name}`, l.name, (name) => call("rename", { id: l.id, name }, "Renamed").then(rerender)) }, [
      el("div", { class: "row-name" }, l.name),
      el("div", { class: "row-sub" }, l.detail || kindName(l)),
    ]),
    el("div", { class: "dev-controls" }, controls),
  ]);
  row.addEventListener("dragstart", (e) => {
    if (e.target.closest && e.target !== row && e.target.closest("select, input, button")) return e.preventDefault();
    e.dataTransfer.setData("text/plain", l.id);
    row.classList.add("dragging");
  });
  row.addEventListener("dragend", () => row.classList.remove("dragging"));
  return row;
}

$("#setup-again").addEventListener("click", () => openWizard());

$("#dev-group-new").addEventListener("click", () =>
  promptName("New group", "", async (name) => {
    await call("create_group", { name: name || "New group" }, "Group created: drag devices into it");
    rerender();
  })
);
