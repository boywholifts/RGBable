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
  const meta = el("div", { class: "meta", title: l.detail }, [
    el("span", { class: "tag" }, l.kind),
    ...(l.effect ? [el("span", { class: "tag fx" }, kindLabelOf(l.effect))] : []),
    l.detail,
  ]);

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
        el("button", { class: "btn mini", onclick: () => editTarget("dev:" + l.id) }, "Effect"),
        el("button", { class: "btn mini", onclick: () => call("identify", { id: l.id }, `Blinking ${l.name}`) }, "Identify"),
        move,
        el("label", { class: "check", title: "Include in Sync everything" }, [sync, "Sync"]),
      ]),
    ]),
  ]);
  if (l.zones.length) {
    const panel = el("div", { class: "zones", hidden: "" }, [
      el("div", { class: "muted small" }, "How many LEDs are on each header. Raise it until the whole strip lights up."),
      ...l.zones.map((z) => {
        const input = el("input", { type: "number", class: "text num", min: z.min, max: z.max, value: z.leds });
        const save = el("button", { class: "btn mini" }, "Set");
        save.addEventListener("click", () =>
          call("resize_zone", { id: l.id, zone: z.index, size: Number(input.value) }, `${z.name}: ${input.value} LEDs`).then(rerender)
        );
        input.addEventListener("keydown", (e) => e.key === "Enter" && save.click());
        return el("div", { class: "zone-row" }, [el("span", { class: "zone-name" }, z.name), input, save]);
      }),
    ]);
    const toggle = el("button", { class: "btn mini" }, "LED count");
    toggle.addEventListener("click", () => (panel.hidden = !panel.hidden));
    card.querySelector(".actions").append(toggle);
    card.querySelector(".info").append(panel);
  }
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
    ...(g.effect ? [el("span", { class: "tag fx" }, kindLabelOf(g.effect))] : []),
    el("button", { class: "btn mini push", onclick: () => editTarget("group:" + g.id) }, "Effect"),
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
  const noRam = orgb.connected && !o.lights.some((l) => l.source === "openrgb" && l.kind === "ram");
  $("#ram-hint").hidden = !noRam || o.manage_openrgb;
  renderTargets(o);
  const names = $("#group-names");
  names.replaceChildren(...o.groups.map((g) => el("option", { value: g.name })));

  const key = JSON.stringify([o.lights, o.groups, orgb.connected]);
  const focused = document.activeElement;
  const busy = focused && focused.closest("#lights") && focused.matches("input.name, input[type=color], input[type=number]");
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

// ---------------------------------------------------------------- effect editor

const KINDS = [
  { id: "static", label: "Static", colors: 1 },
  { id: "breathing", label: "Breathing", colors: 4 },
  { id: "color_cycle", label: "Colour cycle", colors: 4 },
  { id: "rainbow", label: "Rainbow", colors: 0 },
  { id: "rainbow_wave", label: "Rainbow wave", colors: 0, reverse: true },
  { id: "wave", label: "Wave", colors: 4, reverse: true },
  { id: "meteor", label: "Meteor", colors: 4, reverse: true },
  { id: "twinkle", label: "Twinkle", colors: 4 },
  { id: "strobe", label: "Strobe", colors: 4 },
  { id: "heartbeat", label: "Heartbeat", colors: 4 },
  { id: "fire", label: "Fire", colors: 0 },
  { id: "scanner", label: "Scanner", colors: 4 },
  { id: "runway", label: "Runway", colors: 4, reverse: true },
  { id: "fill", label: "Fill", colors: 4, reverse: true },
  { id: "ripple", label: "Ripple", colors: 4 },
  { id: "starry_night", label: "Starry night", colors: 2, hint: "The first colour is the sky, the second the stars.", defaults: ["#2a0aa0", "#ffffff"] },
  { id: "aurora", label: "Aurora", colors: 4, reverse: true, defaults: ["#00ffa0", "#8000ff", "#00a0ff"] },
  { id: "police", label: "Police", colors: 2, defaults: ["#ff0000", "#0040ff"] },
];
const kindInfo = (id) => KINDS.find((k) => k.id === id) || KINDS[0];
const kindLabelOf = (effect) => (effect ? kindInfo(effect.kind).label : "Static");

const fx = { kind: "static", colors: ["#8000ff"], active: 0, targetKey: "" };

function renderKinds() {
  $("#fx-kinds").replaceChildren(...KINDS.map((k) =>
    el("button", { class: "chip" + (k.id === fx.kind ? " on" : ""), onclick: () => {
      fx.kind = k.id;
      if (k.defaults) fx.colors = k.defaults.slice();
      renderEditor();
    } }, k.label)
  ));
}

function renderEditor() {
  renderKinds();
  const info = kindInfo(fx.kind);
  fx.colors = fx.colors.slice(0, Math.max(1, info.colors));
  fx.active = Math.min(fx.active, fx.colors.length - 1);
  $("#fx-colors-row").hidden = info.colors === 0;
  $("#fx-add").hidden = fx.colors.length >= info.colors;
  $("#fx-speed-row").hidden = fx.kind === "static";
  $("#fx-reverse-row").hidden = !info.reverse;
  $("#fx-bright-val").textContent = $("#fx-brightness").value + "%";
  $("#fx-colors").replaceChildren(...fx.colors.map((hex, i) => {
    const s = swatch(hexToRgb(hex), () => {});
    const input = s.querySelector("input");
    input.addEventListener("input", () => (fx.colors[i] = input.value));
    s.classList.add("small");
    s.classList.toggle("active", i === fx.active);
    s.addEventListener("click", () => {
      fx.active = i;
      document.querySelectorAll("#fx-colors .swatch").forEach((n, j) => n.classList.toggle("active", j === i));
    });
    const wrap = el("div", { class: "fx-color" }, s);
    if (fx.colors.length > 1) {
      wrap.append(el("button", { class: "remove", title: "Remove colour", onclick: () => { fx.colors.splice(i, 1); renderEditor(); } }, "×"));
    }
    return wrap;
  }));
  const target = $("#fx-target").value;
  $("#fx-openrgb-row").hidden = target !== "all";
  const hint = info.hint ? info.hint + " " : "";
  $("#fx-note").textContent = hint + (target === "all"
    ? "Devices with their Sync switch off are left alone."
    : fx.kind === "static" ? "" : "Lian Li fans and Strimers play effects by themselves; RGBable keeps the OLED ring and OpenRGB devices animated while it runs.");
}

function currentEffect() {
  return {
    kind: fx.kind,
    colors: fx.colors.map(hexToRgb),
    speed: Number($("#fx-speed").value),
    brightness: Number($("#fx-brightness").value),
    reverse: $("#fx-reverse").checked,
  };
}

function loadEffect(effect, color) {
  if (effect) {
    fx.kind = effect.kind;
    fx.colors = effect.colors.length ? effect.colors.map(rgbToHex) : fx.colors;
    $("#fx-speed").value = effect.speed;
    $("#fx-brightness").value = effect.brightness;
    $("#fx-reverse").checked = effect.reverse;
  } else {
    fx.kind = "static";
    if (color) fx.colors = [rgbToHex(color)];
    $("#fx-brightness").value = 100;
  }
  fx.active = 0;
  renderEditor();
}

function renderTargets(o) {
  const select = $("#fx-target");
  const key = JSON.stringify([o.groups.map((g) => [g.id, g.name]), o.lights.map((l) => [l.id, l.name])]);
  if (key === fx.targetKey) return;
  fx.targetKey = key;
  const value = select.value || "all";
  const options = [el("option", { value: "all" }, "Everything (Sync everything)")];
  if (o.groups.length) options.push(el("optgroup", { label: "Groups" }, o.groups.map((g) => el("option", { value: "group:" + g.id }, g.name))));
  if (o.lights.length) options.push(el("optgroup", { label: "Devices" }, o.lights.map((l) => el("option", { value: "dev:" + l.id }, l.name))));
  select.replaceChildren(...options);
  select.value = [...select.options].some((op) => op.value === value) ? value : "all";
}

function editTarget(value) {
  const select = $("#fx-target");
  select.value = value;
  targetChanged();
  $("#editor").scrollIntoView({ behavior: "smooth", block: "start" });
}

function targetChanged() {
  const value = $("#fx-target").value;
  if (value.startsWith("dev:")) {
    const l = last.lights.find((x) => x.id === value.slice(4));
    if (l) loadEffect(l.effect, l.color);
  } else if (value.startsWith("group:")) {
    const g = last.groups.find((x) => x.id === value.slice(6));
    if (g) loadEffect(g.effect, g.color);
  } else {
    renderEditor();
  }
}

for (const hex of PRESETS) {
  const p = el("button", { class: "preset", title: hex });
  p.style.background = hex;
  p.addEventListener("click", () => {
    fx.colors[fx.active] = hex;
    renderEditor();
  });
  $("#presets").append(p);
}
$("#fx-target").addEventListener("change", targetChanged);
$("#fx-add").addEventListener("click", () => {
  fx.colors.push(PRESETS[fx.colors.length % PRESETS.length]);
  fx.active = fx.colors.length - 1;
  renderEditor();
});
$("#fx-brightness").addEventListener("input", () => ($("#fx-bright-val").textContent = $("#fx-brightness").value + "%"));
$("#fx-apply").addEventListener("click", async () => {
  const effect = currentEffect();
  const value = $("#fx-target").value;
  const label = kindInfo(effect.kind).label;
  $("#fx-apply").disabled = true;
  try {
    if (value.startsWith("dev:")) await call("set_effect", { id: value.slice(4), effect }, `${label} applied`);
    else if (value.startsWith("group:")) await call("set_group_effect", { group: value.slice(6), effect }, `${label} applied to the group`);
    else await call("set_all", { effect, includeOpenrgb: $("#sync-openrgb").checked }, `${label} applied to everything`);
  } finally {
    $("#fx-apply").disabled = false;
    rerender();
  }
});
renderEditor();

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

// ---------------------------------------------------------------- OpenRGB setup

function dot(id, state) {
  $(id).className = "dot " + state;
}

async function loadOpenRgbSetup() {
  let s;
  try {
    s = await invoke("openrgb_setup_status");
  } catch (e) {
    return;
  }
  if (!s) return;
  dot("#os-exe-dot", s.exe ? "ok" : "bad");
  $("#os-exe").textContent = s.exe || "Not found. Install it from openrgb.org, or use Locate if it's somewhere else.";
  dot("#os-pawnio-dot", s.pawnio ? "ok" : "bad");
  $("#os-pawnio").textContent = s.pawnio ? "Installed" : "Not installed. Needed for RAM and some motherboard lighting.";
  $("#os-pawnio-install").hidden = s.pawnio;
  $("#os-managed").checked = s.managed;
  dot("#os-managed-dot", s.managed && s.task ? (s.task_running ? "ok" : "warn") : "");
  $("#os-managed-note").textContent = s.managed
    ? s.task
      ? s.task_running ? "OpenRGB is running as administrator." : "Set up. OpenRGB starts with RGBable."
      : "The background task is missing; turn this off and on again."
    : "Starts OpenRGB hidden, as administrator, whenever RGBable starts. Asks for administrator once to set up.";
}

async function busyButton(button, text, work) {
  const old = button.textContent;
  button.disabled = true;
  button.textContent = text;
  try {
    await work();
  } finally {
    button.disabled = false;
    button.textContent = old;
    loadOpenRgbSetup();
  }
}

$("#os-locate").addEventListener("click", async () => {
  const path = await call("locate_openrgb");
  if (path) toast("Using " + path);
  loadOpenRgbSetup();
});
$("#os-pawnio-install").addEventListener("click", (e) =>
  busyButton(e.target, "Installing…", () => call("install_pawnio", {}, "PawnIO installed"))
);
$("#os-managed").addEventListener("change", async (e) => {
  const enabled = e.target.checked;
  e.target.disabled = true;
  $("#os-managed-note").textContent = enabled ? "Setting up; approve the administrator prompt…" : "Removing the background task…";
  try {
    await call("set_openrgb_managed", { enabled }, enabled ? "OpenRGB now runs in the background" : "RGBable no longer runs OpenRGB");
  } catch (err) {
    e.target.checked = !enabled;
  } finally {
    e.target.disabled = false;
    loadOpenRgbSetup();
    setTimeout(rerender, 8000);
  }
});
document.querySelector('.nav[data-page="settings"]').addEventListener("click", loadOpenRgbSetup);
loadOpenRgbSetup();

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
  $("#update-install2").hidden = !a;
  $("#update-install2").disabled = u.installing;
  $("#update-install2").textContent = u.installing ? "Downloading…" : "Update to " + (a ? a.version : "");
}

$("#update-check").addEventListener("click", async () => renderUpdate(await call("check_update")));
$("#update-channel").addEventListener("change", async (e) =>
  renderUpdate(await call("set_update_channel", { channel: e.target.value }))
);
$("#update-later").addEventListener("click", () => {
  dismissedUpdate = last && last.update.available ? last.update.available.version : null;
  $("#update-banner").hidden = true;
});
async function installUpdate() {
  for (const b of [$("#update-install"), $("#update-install2")]) {
    b.disabled = true;
    b.textContent = "Downloading…";
  }
  try {
    await call("install_update", {}, "Installing the update, RGBable will reopen");
  } catch (e) {
    refresh();
  }
}
$("#update-install").addEventListener("click", installUpdate);
$("#update-install2").addEventListener("click", installUpdate);

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
$("#ram-hint-link").addEventListener("click", (e) => {
  e.preventDefault();
  document.querySelector('.nav[data-page="settings"]').click();
});
