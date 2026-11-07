// RGBable UI. Talks to the Rust side through Tauri commands.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (sel) => document.querySelector(sel);

const PRESETS = ["#8000ff", "#ff2040", "#00e5ff", "#ffffff", "#ff8a00", "#00ff6a"];
let last = null;
let structureKey = "";

// ---------------------------------------------------------------- helpers

function hexToRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function rgbToHex(rgb) {
  if (!rgb) return "#000000";
  return "#" + rgb.map((c) => c.toString(16).padStart(2, "0")).join("");
}
function toast(text, bad = false) {
  const t = $("#toast");
  t.textContent = text;
  t.className = "show" + (bad ? " bad" : "");
  clearTimeout(t._timer);
  t._timer = setTimeout(() => (t.className = ""), 3200);
}
async function call(cmd, args, okText) {
  try {
    const result = await invoke(cmd, args);
    if (okText) toast(okText);
    return result;
  } catch (e) {
    toast(String(e), true);
    throw e;
  }
}
function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v);
  }
  for (const c of [].concat(children)) node.append(c);
  return node;
}

// ---------------------------------------------------------------- navigation

document.querySelectorAll(".nav").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav").forEach((n) => n.classList.toggle("active", n === b));
    document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === "page-" + b.dataset.page));
  })
);

// ---------------------------------------------------------------- lighting

function swatch(color, onPick) {
  const input = el("input", { type: "color", value: rgbToHex(color) });
  const label = el("label", { class: "swatch" }, input);
  label.style.setProperty("--glow", rgbToHex(color));
  input.addEventListener("input", () => label.style.setProperty("--glow", input.value));
  input.addEventListener("change", () => onPick(hexToRgb(input.value)));
  return label;
}

function lightCard(l, groups) {
  const name = el("input", { class: "name", value: l.name, spellcheck: "false", title: "Rename" });
  name.addEventListener("change", () => call("rename", { id: l.id, name: name.value }, "Renamed").then(rerender));
  name.addEventListener("keydown", (e) => e.key === "Enter" && name.blur());
  const meta = el("div", { class: "meta", title: l.detail }, [el("span", { class: "tag" }, l.kind), l.detail]);

  const move = el("select", { class: "small", title: "Move to group" }, [
    el("option", { value: "" }, "Ungrouped"),
    ...groups.map((g) => el("option", { value: g.id }, g.name)),
  ]);
  move.value = l.group || "";
  move.addEventListener("change", () => call("move_to_group", { id: l.id, group: move.value || null }).then(rerender));

  const sync = el("input", { type: "checkbox" });
  sync.checked = l.sync;
  sync.addEventListener("change", () =>
    call("set_sync", { id: l.id, sync: sync.checked }, sync.checked ? `${l.name} follows Sync everything` : `Sync everything leaves ${l.name} alone`)
  );

  const card = el("div", { class: "card device", draggable: "true", "data-id": l.id }, [
    swatch(l.color, (rgb) => call("set_color", { id: l.id, color: rgb }, `${name.value} updated`)),
    el("div", { class: "info" }, [
      name,
      meta,
      el("div", { class: "actions" }, [
        el("button", { class: "btn mini", onclick: () => call("identify", { id: l.id }, `Blinking ${l.name}`) }, "Identify"),
        move,
        el("label", { class: "check", title: "Include in Sync everything" }, [sync, "Sync"]),
      ]),
    ]),
  ]);
  card.addEventListener("dragstart", (e) => {
    e.dataTransfer.setData("text/plain", l.id);
    card.classList.add("dragging");
  });
  card.addEventListener("dragend", () => card.classList.remove("dragging"));
  return card;
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

function groupSection(g, members, groups) {
  const title = el("input", { class: "name group-name", value: g.name, spellcheck: "false", title: "Rename group", size: Math.max(6, g.name.length + 1) });
  title.addEventListener("change", () => call("rename_group", { group: g.id, name: title.value }).then(rerender));
  title.addEventListener("keydown", (e) => e.key === "Enter" && title.blur());
  const head = el("div", { class: "group-head" }, [
    swatch(g.color, (rgb) => call("set_group_color", { group: g.id, color: rgb }, `${g.name} updated`).finally(rerender)),
    title,
    el("span", { class: "muted small" }, members.length === 1 ? "1 device" : `${members.length} devices`),
    el("button", {
      class: "btn mini",
      title: "Delete the group; its devices become ungrouped",
      onclick: () => call("delete_group", { group: g.id }, `Deleted ${g.name}`).then(rerender),
    }, "Delete"),
  ]);
  const grid = el("div", { class: "grid" }, members.length
    ? members.map((l) => lightCard(l, groups))
    : el("div", { class: "empty" }, "Drag devices here, or use a device's group menu."));
  const section = el("section", { class: "group" }, [head, grid]);
  dropZone(section, g.id);
  return section;
}

function renderLights(o) {
  const orgb = o.openrgb;
  $("#orgb-status").replaceChildren(
    orgb.connected ? `OpenRGB connected on port ${orgb.port}` : "OpenRGB not connected (RAM, GPU and motherboard lighting) · ",
    ...(orgb.connected ? [] : [el("a", { href: "#", onclick: (e) => { e.preventDefault(); reconnect(); } }, "Connect")])
  );
  $("#orgb-status2").textContent = orgb.status;
  const names = $("#group-names");
  names.replaceChildren(...o.groups.map((g) => el("option", { value: g.name })));

  const key = JSON.stringify([o.lights, o.groups, orgb.connected]);
  const focused = document.activeElement;
  const busy = focused && focused.closest("#lights") && focused.matches("input.name, input[type=color]");
  if (key === structureKey || busy || document.querySelector("#lights .dragging")) return;
  structureKey = key;

  const root = $("#lights");
  root.replaceChildren();
  if (!o.lights.length) {
    root.append(el("div", { class: "empty" }, "Waiting for devices…"));
    return;
  }
  const byId = new Map(o.lights.map((l) => [l.id, l]));
  for (const g of o.groups) {
    const members = g.members.map((id) => byId.get(id)).filter(Boolean);
    root.append(groupSection(g, members, o.groups));
  }
  const loose = o.lights.filter((l) => !l.group || !o.groups.some((g) => g.id === l.group));
  const section = el("section", { class: "group ungrouped" }, [
    el("div", { class: "group-head" }, el("h2", {}, o.groups.length ? "Ungrouped" : "Devices")),
    el("div", { class: "grid" }, loose.length ? loose.map((l) => lightCard(l, o.groups)) : el("div", { class: "empty" }, "Every device is in a group.")),
  ]);
  dropZone(section, null);
  root.append(section);
}

function rerender() {
  structureKey = "";
  refresh();
}

$("#group-new").addEventListener("click", async () => {
  await call("create_group", { name: "New group" }, "Group created: click its name to rename it");
  rerender();
});

// ---------------------------------------------------------------- setup wizard

const wizard = { open: false, index: 0, lights: [], entries: new Map() };

function openWizard() {
  if (!last || !last.lights.length) return toast("No devices found yet", true);
  wizard.open = true;
  wizard.index = -1;
  wizard.lights = last.lights.slice();
  wizard.entries = new Map();
  $("#wizard").hidden = false;
  showWizardStep();
}

function closeWizard() {
  wizard.open = false;
  $("#wizard").hidden = true;
}

function groupName(id) {
  const g = last && last.groups.find((g) => g.id === id);
  return g ? g.name : "";
}

function wizardEntry(l) {
  return wizard.entries.get(l.id) || { id: l.id, name: l.name, group: groupName(l.group), sync: l.sync };
}

function showWizardStep() {
  const total = wizard.lights.length;
  const body = $("#wz-body");
  $("#wz-back").disabled = wizard.index < 0;
  $("#wz-back").style.visibility = wizard.index < 0 ? "hidden" : "";
  $("#wz-skip").style.visibility = wizard.index < 0 || wizard.index >= total ? "hidden" : "";

  if (wizard.index < 0) {
    $("#wz-step").textContent = "Setup";
    $("#wz-title").textContent = "Let's find your lights";
    const orgbNote = last.openrgb.connected
      ? `${total} devices found, including OpenRGB devices like RAM, GPU, monitors and sound cards.`
      : `${total} Lian Li devices found. OpenRGB isn't connected yet, so RAM, GPU, motherboard and monitor lighting won't be listed; you can run setup again from Settings once it is.`;
    body.replaceChildren(
      el("p", {}, "Each device will blink white so you can see which one it is. Give it a name you'll recognise, put it in a group, and choose whether Sync everything should change it."),
      el("p", { class: "muted small" }, orgbNote),
      el("p", { class: "muted small" }, "Nothing is saved until you click Finish.")
    );
    $("#wz-next").textContent = "Start";
    return;
  }

  if (wizard.index >= total) {
    $("#wz-step").textContent = "Done";
    $("#wz-title").textContent = "Ready to save";
    const rows = wizard.lights.map((l) => {
      const e = wizard.entries.get(l.id);
      const text = e ? `${e.name}${e.group ? " · " + e.group : ""}${e.sync ? "" : " · not in Sync everything"}` : `${l.name} · unchanged`;
      return el("li", {}, text);
    });
    body.replaceChildren(el("ul", { class: "wizard-summary" }, rows));
    $("#wz-next").textContent = "Finish";
    return;
  }

  const l = wizard.lights[wizard.index];
  const e = wizardEntry(l);
  $("#wz-step").textContent = `Device ${wizard.index + 1} of ${total}`;
  $("#wz-title").textContent = l.default_name;
  const name = el("input", { class: "text", id: "wz-name", value: e.name, spellcheck: "false" });
  const group = el("input", { class: "text", id: "wz-group", value: e.group, list: "group-names", placeholder: "No group (type a name to create one)" });
  const sync = el("input", { type: "checkbox", class: "toggle", id: "wz-sync" });
  sync.checked = e.sync;
  body.replaceChildren(
    el("div", { class: "meta" }, [el("span", { class: "tag" }, l.kind), l.detail]),
    el("div", { class: "row" }, [
      el("span", { class: "muted small" }, "It should be blinking white now."),
      el("button", { class: "btn mini", onclick: () => call("identify", { id: l.id }) }, "Blink again"),
    ]),
    el("label", { class: "field-col" }, [el("span", {}, "Name"), name]),
    el("label", { class: "field-col" }, [el("span", {}, "Group"), group]),
    el("label", { class: "toggle-row" }, [
      el("div", {}, [el("div", {}, "Include in Sync everything"), el("div", { class: "muted small" }, "Turn off for things like monitors you don't want recoloured.")]),
      sync,
    ])
  );
  name.addEventListener("keydown", (ev) => ev.key === "Enter" && $("#wz-next").click());
  $("#wz-next").textContent = "Next";
  call("identify", { id: l.id }).catch(() => {});
  setTimeout(() => name.focus(), 0);
}

function saveWizardStep() {
  const l = wizard.lights[wizard.index];
  if (!l) return;
  wizard.entries.set(l.id, {
    id: l.id,
    name: $("#wz-name").value.trim() || l.default_name,
    group: $("#wz-group").value.trim(),
    sync: $("#wz-sync").checked,
  });
}

$("#wz-next").addEventListener("click", async () => {
  if (wizard.index >= wizard.lights.length) {
    const entries = [...wizard.entries.values()].map((e) => ({ ...e, group: e.group || null }));
    await call("finish_setup", { entries }, "Setup saved");
    closeWizard();
    rerender();
    return;
  }
  if (wizard.index >= 0) saveWizardStep();
  wizard.index++;
  showWizardStep();
});
$("#wz-skip").addEventListener("click", () => {
  wizard.entries.delete(wizard.lights[wizard.index].id);
  wizard.index++;
  showWizardStep();
});
$("#wz-back").addEventListener("click", () => {
  if (wizard.index < wizard.lights.length) saveWizardStep();
  wizard.index--;
  showWizardStep();
});
$("#wz-close").addEventListener("click", closeWizard);
$("#setup-again").addEventListener("click", openWizard);

const startedAt = Date.now();
let wizardOffered = false;
function maybeStartWizard(o) {
  if (wizardOffered || o.setup_done || o.status !== "Ready" || !o.lights.length) return;
  if (!o.openrgb.connected && Date.now() - startedAt < 30000) return;
  wizardOffered = true;
  openWizard();
}

async function reconnect() {
  toast("Looking for OpenRGB…");
  await call("openrgb_connect", {}, "Connected to OpenRGB").catch(() => {});
  refresh();
}

for (const hex of PRESETS) {
  const p = el("button", { class: "preset", title: hex });
  p.style.background = hex;
  p.addEventListener("click", () => {
    $("#sync-color").value = hex;
  });
  $("#presets").append(p);
}
$("#sync-apply").addEventListener("click", async () => {
  const color = hexToRgb($("#sync-color").value);
  await call("set_all", { color, includeOpenrgb: $("#sync-openrgb").checked }, "Applied to everything");
  structureKey = "";
  refresh();
});

// ---------------------------------------------------------------- OLED

let oledFit = "cover";
$("#oled-fit").addEventListener("click", (e) => {
  const fit = e.target.dataset.fit;
  if (!fit) return;
  oledFit = fit;
  document.querySelectorAll("#oled-fit button").forEach((b) => b.classList.toggle("on", b.dataset.fit === fit));
});
$("#oled-pick").addEventListener("click", async () => {
  const file = await invoke("pick_media");
  if (!file) return;
  await call("set_oled", { file, fit: oledFit, fps: Number($("#oled-fps").value) }, "Sending to the OLED…");
  refresh();
});
$("#oled-apply").addEventListener("click", async () => {
  await call("set_oled", { file: null, fit: oledFit, fps: Number($("#oled-fps").value) }, "Updated");
  refresh();
});
$("#oled-stop").addEventListener("click", async () => {
  await call("stop_oled", {}, "OLED stopped");
  refresh();
});

function renderOled(oled, initial) {
  $("#oled-file").textContent = oled.file || "Nothing selected";
  const state = $("#oled-state");
  state.textContent = oled.state;
  state.classList.toggle("on", oled.state === "Playing");
  if (initial) {
    oledFit = oled.fit;
    document.querySelectorAll("#oled-fit button").forEach((b) => b.classList.toggle("on", b.dataset.fit === oled.fit));
    $("#oled-fps").value = String(oled.fps);
  }
}

// ---------------------------------------------------------------- monitor

function renderMonitor(o) {
  $("#m-pump").textContent = o.telemetry.pump_rpm ?? "–";
  $("#m-coolant").textContent = o.telemetry.coolant_c ?? "–";
  const grid = $("#m-fans");
  grid.replaceChildren();
  for (const d of o.devices.filter((d) => d.kind === "fans")) {
    grid.append(el("div", { class: "card" }, [
      el("div", { class: "card-title" }, d.name),
      el("div", { class: "fanrow" }, d.rpm.map((r) => el("span", {}, `${r} rpm`))),
    ]));
  }
}

// ---------------------------------------------------------------- settings

$("#autostart").addEventListener("change", (e) =>
  call("set_autostart", { enabled: e.target.checked }, e.target.checked ? "RGBable will start with Windows" : "Autostart off")
);
$("#orgb-connect").addEventListener("click", reconnect);

// ---------------------------------------------------------------- updates

let dismissedUpdate = null;

function renderUpdate(u) {
  $("#app-version").textContent = u.current;
  const a = u.available;
  let status = "Up to date";
  if (u.installing) status = "Downloading update…";
  else if (u.checking) status = "Checking…";
  else if (u.error) status = "Couldn't check: " + u.error;
  else if (a) status = "Version " + a.version + " is available";
  $("#update-status").textContent = status;
  $("#update-banner").hidden = !a || a.version === dismissedUpdate;
  if (a) {
    $("#update-version").textContent = a.version;
    $("#update-notes").textContent = a.notes.replace(/^#+\s*/gm, "").trim() || "See the release page for what's new.";
  }
  $("#update-install").disabled = u.installing;
  $("#update-install").textContent = u.installing ? "Downloading…" : "Update now";
}

$("#update-check").addEventListener("click", async () => renderUpdate(await call("check_update")));
$("#update-channel").addEventListener("change", async (e) =>
  renderUpdate(await call("set_update_channel", { channel: e.target.value }))
);
$("#update-later").addEventListener("click", () => {
  dismissedUpdate = last && last.update.available ? last.update.available.version : null;
  $("#update-banner").hidden = true;
});
$("#update-install").addEventListener("click", async () => {
  $("#update-install").disabled = true;
  $("#update-install").textContent = "Downloading…";
  try {
    await call("install_update", {}, "Installing the update, RGBable will reopen");
  } catch (e) {
    $("#update-install").disabled = false;
    $("#update-install").textContent = "Update now";
  }
});

// ---------------------------------------------------------------- refresh loop

async function refresh() {
  let o;
  try {
    o = await invoke("overview");
  } catch (e) {
    return;
  }
  const initial = last === null;
  last = o;

  const status = $("#status");
  let text = o.status;
  if (o.lconnect_running && o.status !== "Ready") text = "Waiting for L-Connect to close";
  status.textContent = text;
  status.className = "status " + (o.status === "Ready" ? "ok" : o.status.startsWith("Error") ? "bad" : "");

  renderLights(o);
  maybeStartWizard(o);
  renderOled(o.oled, initial);
  renderMonitor(o);
  renderUpdate(o.update);
  if (initial) {
    $("#autostart").checked = o.autostart;
    $("#data-dir").textContent = o.data_dir;
    $("#update-channel").value = o.update_channel;
  }
}

refresh();
setInterval(refresh, 1500);
