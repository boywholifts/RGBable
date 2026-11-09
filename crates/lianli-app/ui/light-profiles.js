// Lighting profiles: snapshots of every device's lighting (and, if chosen, the screen
// video), picked from one menu so a long list stays tidy.
let lightProfiles = [];
let lightProfile = null;
let screenVideo = null;

const FILM_STRIP =
  '<svg class="film" viewBox="0 0 24 24" aria-label="with a video"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 4v16M17 4v16M3 8h4M3 12h4M3 16h4M17 8h4M17 12h4M17 16h4"/></svg>';

function filmStrip() {
  const span = el("span", { class: "film-wrap", title: "Includes a screen video" });
  span.innerHTML = FILM_STRIP;
  return span;
}

function renderLightProfiles(o) {
  screenVideo = o.oled && o.oled.file ? o.oled.file : null;
  lightProfiles = o.lighting_profiles || [];
  lightProfile = lightProfiles.some((p) => p.name === o.lighting_profile) ? o.lighting_profile : null;
  const key = JSON.stringify([lightProfiles, lightProfile]);
  if (key === renderLightProfiles.key) return;
  renderLightProfiles.key = key;
  const current = lightProfiles.find((p) => p.name === lightProfile);
  $("#light-profile-current").replaceChildren(
    ...(current ? [current.name, current.screen ? filmStrip() : null].filter(Boolean) : [lightProfiles.length ? "Choose" : "None"]),
  );
}

async function lightProfileAction(cmd, args, text) {
  await call(cmd, args, text);
  renderLightProfiles.key = "";
  refresh();
}

function profileLabel(p) {
  return el("span", { class: "profile-label" }, [el("span", { class: "profile-name" }, p.name), p.screen ? filmStrip() : null].filter(Boolean));
}

$("#light-profile-pick").addEventListener("click", (e) => {
  e.stopPropagation();
  const items = [];
  if (lightProfiles.length) {
    items.push({ header: "Lighting profiles" });
    for (const p of lightProfiles) {
      items.push({
        label: profileLabel(p),
        checked: p.name === lightProfile,
        onClick: () => confirmLightProfile(p),
      });
    }
    items.push("-");
  }
  items.push({ label: "Save current lighting as a profile…", onClick: saveLightProfileDialog });
  if (lightProfile) {
    const name = lightProfile;
    items.push(
      { label: `Update ${name} with current lighting`, onClick: () => lightProfileAction("save_lighting_profile", { name }, `${name} updated`) },
      { label: `Rename ${name}…`, onClick: () => promptName("Rename profile", name, (to) => to && to !== name && lightProfileAction("rename_lighting_profile", { name, to })) },
      { label: `Delete ${name}`, danger: true, onClick: () => deleteLightProfile(name) },
    );
  }
  openMenu($("#light-profile-pick"), items);
});

// Picking a profile only opens this: nothing changes until Apply, so a stray click in the
// menu can't replace the lighting.
function confirmLightProfile(p) {
  const what = p.screen
    ? "Every device gets the lighting saved in it, and its video goes on the HydroShift screen."
    : "Every device gets the lighting saved in it.";
  openDialog(`Apply ${p.name}?`, el("p", { class: "muted small" }, what), [
    el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
    el("button", { class: "btn primary", onclick: () => { closeDialog(); lightProfileAction("apply_lighting_profile", { name: p.name }, `${p.name} on`); } }, "Apply"),
  ]);
}

function deleteLightProfile(name) {
  openDialog(`Delete ${name}?`, el("p", { class: "muted small" }, "The lighting you have now stays as it is."), [
    el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
    el("button", { class: "btn primary", onclick: () => { closeDialog(); lightProfileAction("delete_lighting_profile", { name }, `${name} deleted`); } }, "Delete"),
  ]);
}

function saveLightProfileDialog() {
  const input = el("input", { class: "text", maxlength: "40", placeholder: "Name" });
  const screen = el("label", { class: "check" }, [
    el("input", { type: "checkbox" }),
    screenVideo ? "Include the current video" : "Include the current video (nothing is playing)",
  ]);
  const box = screen.querySelector("input");
  box.disabled = !screenVideo;
  const save = el("button", { class: "btn primary" }, "Save");
  save.addEventListener("click", () => {
    const name = input.value.trim();
    if (!name) return input.focus();
    closeDialog();
    const go = () => lightProfileAction("save_lighting_profile", { name, screen: box.checked }, `Saved ${name}`);
    if (!lightProfiles.some((p) => p.name === name)) return go();
    openDialog(`Replace ${name}?`, el("p", { class: "muted small" }, "It gets the lighting as it is now."), [
      el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
      el("button", { class: "btn primary", onclick: () => { closeDialog(); go(); } }, "Replace"),
    ]);
  });
  input.addEventListener("keydown", (e) => e.key === "Enter" && save.click());
  openDialog("Save the lighting as a profile", el("div", { class: "profile-save" }, [input, screen]), [
    el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
    save,
  ]);
  input.focus();
}
