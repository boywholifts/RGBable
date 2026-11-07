// RGBable UI. Talks to the Rust side through Tauri commands.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (sel) => document.querySelector(sel);

const PRESETS = ["#8000ff", "#ff2040", "#00e5ff", "#ffffff", "#ff8a00", "#00ff6a"];
let last = null;
let structureKey = "";
let orgbKey = "";

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

function kindLabel(d) {
  if (d.kind === "ring") return "OLED ring";
  if (d.kind === "strimer") return "Strimer";
  return d.fans + (d.fans === 1 ? " fan" : " fans");
}

function renderLianLi(devices) {
  const grid = $("#lianli-devices");
  grid.replaceChildren();
  if (!devices.length) {
    grid.append(el("div", { class: "empty" }, "Waiting for devices…"));
    return;
  }
  for (const d of devices) {
    const name = el("input", { class: "name", value: d.name, spellcheck: "false" });
    name.addEventListener("change", () => call("rename", { id: d.id, name: name.value }, "Renamed"));
    name.addEventListener("keydown", (e) => e.key === "Enter" && name.blur());
    const meta = el("div", { class: "meta" }, [el("span", { class: "tag" }, kindLabel(d)), d.model]);
    const card = el("div", { class: "card device", "data-id": d.id }, [
      swatch(d.color, (rgb) => call("set_color", { id: d.id, color: rgb }, `${name.value} updated`)),
      el("div", { class: "info" }, [name, meta]),
    ]);
    grid.append(card);
  }
}

function renderOpenRgb(orgb) {
  $("#orgb-status").textContent = orgb.connected ? `· via OpenRGB (port ${orgb.port})` : "· OpenRGB not connected";
  $("#orgb-status2").textContent = orgb.status;
  const key = JSON.stringify(orgb.devices.map((d) => d.index + d.name));
  if (key === orgbKey) return;
  orgbKey = key;
  const grid = $("#orgb-devices");
  grid.replaceChildren();
  if (!orgb.connected) {
    grid.append(el("div", { class: "empty" }, [
      "Start OpenRGB's SDK server to control RAM, GPU and motherboard lighting here. ",
      el("a", { href: "#", onclick: (e) => { e.preventDefault(); reconnect(); } }, "Connect"),
    ]));
    return;
  }
  for (const d of orgb.devices) {
    const card = el("div", { class: "card device" }, [
      swatch(null, (rgb) => call("openrgb_set_color", { index: d.index, color: rgb }, `${d.name} updated`)),
      el("div", { class: "info" }, [
        el("div", { class: "name" }, d.name),
        el("div", { class: "meta" }, [el("span", { class: "tag" }, d.kind), `${d.leds} LEDs`]),
      ]),
    ]);
    grid.append(card);
  }
}

async function reconnect() {
  toast("Looking for OpenRGB…");
  orgbKey = "";
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
    $("#update-notes").textContent = a.notes.trim() || "See the release page for what's new.";
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

  const key = JSON.stringify(o.devices.map((d) => [d.id, d.name, d.color]));
  const editing = document.activeElement && document.activeElement.classList.contains("name");
  if (key !== structureKey && !editing) {
    structureKey = key;
    renderLianLi(o.devices);
  }
  renderOpenRgb(o.openrgb);
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
