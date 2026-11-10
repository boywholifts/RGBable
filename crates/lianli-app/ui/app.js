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
      el("p", {}, "Each device will blink white so you can see which one it is. Give it a name you'll recognise, put it in a group, and choose whether All Devices should change it."),
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
      const text = e ? `${e.name}${e.group ? " · " + e.group : ""}${e.sync ? "" : " · not in All Devices"}` : `${l.name} · unchanged`;
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
      el("div", {}, [el("div", {}, "Include in All Devices"), el("div", { class: "muted small" }, "Turn off for things like monitors you don't want recolored.")]),
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
$("#restart-app").addEventListener("click", async () => {
  const button = $("#restart-app");
  button.disabled = true;
  button.textContent = "Restarting…";
  try {
    await invoke("restart_app");
  } catch (e) {
    toast(String(e), true);
    button.disabled = false;
    button.textContent = "Restart RGBable";
  }
});

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
  if (!editingPresets) PRESETS = o.color_presets.length ? o.color_presets : DEFAULT_PRESETS.slice();

  const status = $("#status");
  let text = o.status;
  if (o.lconnect_running && o.status !== "Ready") text = "Waiting for L-Connect to close";
  status.textContent = text;
  status.className = "status " + (o.status === "Ready" ? "ok" : o.status.startsWith("Error") ? "bad" : "");

  renderAllOff(o);
  renderFanScreensNav(o);
  renderLights(o);
  renderLightProfiles(o);
  maybeStartWizard(o);
  renderOled(o.oled, initial);
  renderSequence(o);
  renderMonitor(o);
  renderUpdate(o.update);
  if (initial) {
    $("#autostart").checked = o.autostart;
    $("#autostart-blocked").hidden = !(o.autostart && o.autostart_blocked);
    $("#warmth").value = String(o.warmth || 0);
    $("#clean-transitions").checked = o.clean_transitions;
    $("#warmth-value").textContent = warmthLabel(o.warmth || 0);
    $("#data-dir").textContent = o.data_dir;
    $("#update-channel").value = o.update_channel;
  }
  renderStartupProfile(o);
}

refresh();
let refreshing = false;
setInterval(async () => {
  if (refreshing || document.hidden) return;
  refreshing = true;
  try {
    await refresh();
  } finally {
    refreshing = false;
  }
}, 1500);
$("#ram-hint-link").addEventListener("click", (e) => {
  e.preventDefault();
  document.querySelector('.nav[data-page="settings"]').click();
});

loadEffectCatalogue();

// ---------------------------------------------------------------- white balance

const warmthLabel = (w) => (w === 0 ? "Neutral" : w > 0 ? `Warmer ${w}` : `Cooler ${-w}`);
let warmthTimer = null;
function setWarmth(value) {
  $("#warmth").value = String(value);
  $("#warmth-value").textContent = warmthLabel(value);
  clearTimeout(warmthTimer);
  warmthTimer = setTimeout(() => invoke("set_warmth", { warmth: value }).catch((e) => toast(String(e), true)), 250);
}
$("#warmth").addEventListener("input", (e) => setWarmth(Number(e.target.value)));
$("#warmth-reset").addEventListener("click", () => setWarmth(0));
$("#clean-transitions").addEventListener("change", (e) => invoke("set_clean_transitions", { enabled: e.target.checked }).catch((err) => toast(String(err), true)));

// ------------------------------------------------------------- startup and power-on

function renderStartupProfile(o) {
  const select = $("#startup-profile");
  const names = (o.lighting_profiles || []).map((p) => p.name);
  const key = JSON.stringify([names, o.startup_profile, o.startup_dark, o.startup_profile_when_locked]);
  if (select.dataset.key === key) return;
  select.dataset.key = key;
  select.replaceChildren(
    el("option", { value: "" }, "None: keep my lighting"),
    el("option", { value: DARK_AT_STARTUP }, "Dark until I sign in"),
    ...names.map((n) => el("option", { value: n }, `Profile: ${n}`)),
  );
  select.value = o.startup_dark ? DARK_AT_STARTUP : o.startup_profile || "";
  $("#startup-locked").checked = !!o.startup_profile_when_locked;
  $(".startup-locked").hidden = !select.value;
}

const DARK_AT_STARTUP = "__dark__";

function saveStartupProfile(note) {
  const value = $("#startup-profile").value;
  const dark = value === DARK_AT_STARTUP;
  $(".startup-locked").hidden = !value;
  return call("set_startup_profile", { name: dark ? null : value || null, dark, whenLocked: $("#startup-locked").checked }, note);
}

$("#startup-profile").addEventListener("change", (e) =>
  saveStartupProfile(e.target.value === DARK_AT_STARTUP ? "Dark until you sign in" : e.target.value ? `${e.target.value} at startup` : "No startup lighting"));
$("#startup-locked").addEventListener("change", (e) =>
  saveStartupProfile(e.target.checked ? "Shown while locked too" : "Only shown while the PC is off"));

let powerOnMode = "plain";
document.querySelectorAll("#power-on-mode button").forEach((b) =>
  b.addEventListener("click", () => {
    powerOnMode = b.dataset.mode;
    document.querySelectorAll("#power-on-mode button").forEach((x) => x.classList.toggle("on", x === b));
  }));

$("#power-on-save").addEventListener("click", async () => {
  const button = $("#power-on-save");
  button.disabled = true;
  $("#power-on-result").textContent = "Saving…";
  try {
    const r = await call("save_power_on_lighting", { style: powerOnMode });
    if (!r) return;
    const parts = [];
    if (r.saved.length) parts.push(`Saved on ${r.saved.join(", ")}.`);
    if (r.unsupported.length) parts.push(`${r.unsupported.join(", ")} can't keep lighting in memory.`);
    if (r.failed.length) parts.push(`Couldn't save on ${r.failed.join(", ")}; see the log.`);
    $("#power-on-result").textContent = parts.join(" ") || "No OpenRGB devices are connected.";
  } finally {
    button.disabled = false;
  }
});
