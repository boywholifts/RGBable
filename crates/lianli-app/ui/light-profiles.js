// Lighting profiles: snapshots of every device's lighting, applied in one click.
let lightProfiles = [];
let lightProfile = null;

function renderLightProfiles(o) {
  const key = JSON.stringify([o.lighting_profiles, o.lighting_profile]);
  if (key === renderLightProfiles.key) return;
  renderLightProfiles.key = key;
  lightProfiles = o.lighting_profiles || [];
  lightProfile = lightProfiles.includes(o.lighting_profile) ? o.lighting_profile : null;
  $("#light-profiles").replaceChildren(
    ...(lightProfiles.length
      ? lightProfiles.map((name) =>
          el("button", { class: name === lightProfile ? "on" : "", title: `Light everything as in ${name}`, onclick: () => applyLightProfile(name) }, name),
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

function applyLightProfile(name) {
  lightProfileAction("apply_lighting_profile", { name }, `${name} lighting on`);
}

$("#light-profile-save").addEventListener("click", () =>
  promptName("Save the lighting as a profile", "", (name) => {
    if (!name) return;
    const save = () => lightProfileAction("save_lighting_profile", { name }, `Saved ${name}`);
    if (!lightProfiles.includes(name)) return save();
    openDialog(`Replace ${name}?`, el("p", { class: "muted small" }, "It gets the lighting as it is now."), [
      el("button", { class: "btn", onclick: closeDialog }, "Cancel"),
      el("button", { class: "btn primary", onclick: () => { closeDialog(); save(); } }, "Replace"),
    ]);
  }),
);
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
