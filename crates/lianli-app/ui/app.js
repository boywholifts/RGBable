// RGBable app: setup wizard, OLED, monitor, settings, updates and the refresh loop.
// Loaded after core.js, effects.js, lighting.js and library.js.

// ---------------------------------------------------------------- setup wizard

const wizard = { open: false, index: 0, lights: [], entries: new Map() };

function openWizard() {
  $("#group-names").replaceChildren(...(last ? last.groups : []).map((g) => el("option", { value: g.name })));
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

loadEffectCatalogue();
