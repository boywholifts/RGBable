// Lighting profiles: snapshots of every device's lighting (and, if chosen, the screen
// video), applied in one click.
let lightProfiles = [];
let lightProfile = null;
let screenVideo = null;

function renderLightProfiles(o) {
  screenVideo = o.oled && o.oled.file ? o.oled.file.split(/[\\/]/).pop().replace(/\.[^.]+$/, "") : null;
  const key = JSON.stringify([o.lighting_profiles, o.lighting_profile]);
  if (key === renderLightProfiles.key) return;
  renderLightProfiles.key = key;
  lightProfiles = o.lighting_profiles || [];
  lightProfile = lightProfiles.some((p) => p.name === o.lighting_profile) ? o.lighting_profile : null;
  $("#light-profiles").replaceChildren(
    ...(lightProfiles.length
      ? lightProfiles.map((p) =>
          el(
            "button",
            {
              class: p.name === lightProfile ? "on" : "",
              title: p.screen ? `Lighting from ${p.name}, and ${p.screen} on the screen` : `Lighting from ${p.name}`,
              onclick: () => lightProfileAction("apply_lighting_profile", { name: p.name }, `${p.name} on`),
            },
            [p.name, p.screen ? el("span", { class: "profile-screen", "aria-label": "with a screen video" }, " ▶") : null].filter(Boolean),
          ),
        )
      : [el("span", { class: "muted small" }, "None yet: save the lighting you have to come back to it")]),
  );
  for (const id of ["#light-profile-update", "#light-profile-rename", "#light-profile-delete"]) $(id).disabled = !lightProfile;
}

async function lightProfileAction(cmd, args, text) {
  await call(cmd, args, text);
  renderLightProfiles.key = "";
  refresh();
}

function saveLightProfileDialog() {
  const input = el("input", { class: "text", maxlength: "40", placeholder: "Name" });
  const screen = el("label", { class: "check" }, [
    el("input", { type: "checkbox" }),
    screenVideo ? `Also put ${screenVideo} on the HydroShift screen` : "Also save the screen video (nothing is playing now)",
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

$("#light-profile-save").addEventListener("click", saveLightProfileDialog);
$("#light-profile-update").addEventListener("click", () =>
  lightProfile && lightProfileAction("save_lighting_profile", { name: lightProfile }, `${lightProfile} updated`),
);
$("#light-profile-rename").addEventListener("click", () =>
  lightProfile && promptName("Rename profile", lightProfile, (to) => to && to !== lightProfile && lightProfileAction("rename_lighting_profile", { name: lightProfile, to })),
);
$("#light-profile-delete").addEventListener("click", () => {
  const name = lightProfile;
  if (!name) return;
  openDialog(`Delete ${name}?`, el("p", { class: "muted small" }, "The lighting you have now stays as it is."), [
    el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
    el("button", { class: "btn primary", onclick: () => { closeDialog(); lightProfileAction("delete_lighting_profile", { name }, `${name} deleted`); } }, "Delete"),
  ]);
});
