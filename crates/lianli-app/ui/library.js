// Effects page (saved custom effects) and the custom effect builder: Recipe, Layers, Script.

// ---------------------------------------------------------------- recipe → script

const RECIPE_DEFAULT = {
  color: "gradient",
  shape: "bar",
  bands: 3,
  width: 25,
  motion: "spin",
  tail: 60,
  softness: 50,
  min: 0,
  max: 100,
  offset: 0,
  alternate: false,
};

const RECIPE_CHOICES = {
  color: [["solid", "Solid"], ["gradient", "Gradient"], ["rainbow", "Rainbow"], ["random", "Random per LED"]],
  shape: [["all", "Everywhere"], ["bands", "Bands"], ["dot", "Dot"], ["bar", "Bar"], ["mirror", "Mirrored"], ["segments", "Alternating"]],
  motion: [["still", "Still"], ["spin", "Spin ↻"], ["spin_back", "Spin ↺"], ["bounce", "Bounce"], ["pulse", "Pulse"], ["flicker", "Flicker"], ["flash", "Flash"]],
};

const num = (v) => (Number.isInteger(v) ? v + ".0" : String(v));

/// Writes a script for a recipe. Every value is a number the controls produced, so the
/// script is always valid Rhai.
function recipeScript(r) {
  const lines = ["// Made with the recipe builder. Edit by hand to change it freely."];
  lines.push("let p = pos;");
  if (r.alternate) lines.push("if zone % 2 == 1 { p = 1.0 - p; }");
  lines.push(`let ph = fract(t + zone * ${num(r.offset / 100)});`);

  // Where the pattern sits now: moving shapes slide along p.
  let x = "p";
  if (r.motion === "spin") x = "fract(p - ph)";
  else if (r.motion === "spin_back") x = "fract(p + ph)";
  else if (r.motion === "bounce") x = "fract(p - (1.0 - abs(2.0 * ph - 1.0)) + 0.5)";
  lines.push(`let x = ${x};`);

  const soft = num(Math.max(0.02, r.softness / 100));
  const width = num(Math.max(0.03, r.width / 100));
  const tail = num(r.tail / 100);
  switch (r.shape) {
    case "all":
      lines.push("let level = 1.0;");
      break;
    case "bands":
      lines.push(`let wave = cos(x * ${num(r.bands)} * TAU) * 0.5 + 0.5;`);
      lines.push(`let level = smooth((wave - 0.5) / ${soft} + 0.5);`);
      break;
    case "dot":
    case "bar": {
      const w = r.shape === "dot" ? "0.06" : width;
      lines.push("let ahead = fract(x);");
      lines.push("let behind = fract(1.0 - x);");
      lines.push(`let body = clamp01(1.0 - min(ahead, behind) / ${w});`);
      if (r.motion === "spin" || r.motion === "spin_back") {
        lines.push(`let trail = if ${tail} > 0.0 { clamp01(1.0 - behind / (${tail} + 0.001)) * clamp01(1.0 - behind / (${tail} + 0.001)) } else { 0.0 };`);
        lines.push(`let level = max(smooth(body / ${soft} * 0.5 + body * 0.5), trail * 0.8);`);
      } else {
        lines.push(`let level = smooth(body / ${soft} * 0.5 + body * 0.5);`);
      }
      break;
    }
    case "mirror":
      lines.push("let d = abs(x - 0.5) * 2.0;");
      lines.push(`let level = smooth(clamp01((1.0 - d) / ${width}));`);
      break;
    case "segments":
      lines.push(`let level = if (x * ${num(r.bands * 2)}).floor() % 2.0 == 0.0 { 1.0 } else { 0.0 };`);
      break;
  }

  if (r.motion === "pulse") lines.push("let level = level * sin(ph * PI) ** 2.0;");
  else if (r.motion === "flicker") lines.push("let level = level * (0.55 + 0.45 * noise(led * 31 + (ph * 24.0).floor().to_int()));");
  else if (r.motion === "flash") lines.push("let level = level * if fract(ph * 4.0) < 0.3 { 1.0 } else { 0.0 };");

  lines.push(`let level = ${num(r.min / 100)} + ${num((r.max - r.min) / 100)} * clamp01(level);`);
  switch (r.color) {
    case "solid":
      lines.push("let c = colors[0];");
      break;
    case "gradient":
      lines.push(`let c = palette(colors, ${r.motion === "pulse" || r.motion === "flicker" || r.motion === "flash" ? "fract(p + ph)" : "x"});`);
      break;
    case "rainbow":
      lines.push(`let c = hsv(${r.motion === "pulse" || r.motion === "flicker" || r.motion === "flash" ? "fract(p + ph)" : "x"}, 1.0, 1.0);`);
      break;
    case "random":
      lines.push("let c = colors[(noise(led) * colors.len().to_float()).floor().to_int() % colors.len()];");
      break;
  }
  lines.push("dim(c, level)");
  return lines.join("\n") + "\n";
}

// ---------------------------------------------------------------- builder

const BLENDS = { over: "Cover", add: "Add", multiply: "Multiply", brightest: "Brightest" };
const builder = { mode: "recipe", recipe: { ...RECIPE_DEFAULT }, recipeColors: ["#ff0080", "#00c8ff"], layers: [], scriptColors: ["#ff0080", "#00c8ff"], original: null };
const builderPreview = previewPlayer($("#b-preview"));

function defaultLayer(kind = "rainbow_wave") {
  const info = kindInfo(kind);
  return {
    effect: { kind, colors: (info.defaults || ["#8000ff", "#00e5ff"]).slice(0, Math.max(1, info.colors)).map(hexToRgb), speed: 3, brightness: 100, reverse: false },
    blend: "over",
    opacity: 100,
  };
}

function builderEffect() {
  const base = { name: $("#b-name").value.trim(), speed: Number($("#b-speed").value), brightness: 100, reverse: false, colors: [] };
  if (builder.mode === "recipe") {
    return { ...base, kind: "script", script: recipeScript(builder.recipe), recipe: JSON.stringify(builder.recipe), colors: builder.recipeColors.map(hexToRgb) };
  }
  if (builder.mode === "script") {
    return { ...base, kind: "script", script: $("#b-script").value, colors: builder.scriptColors.map(hexToRgb) };
  }
  return { ...base, kind: "custom", layers: builder.layers };
}

let builderTimer = null;
function builderChanged() {
  if (builder.mode === "recipe") $("#b-recipe-code").textContent = recipeScript(builder.recipe);
  clearTimeout(builderTimer);
  builderTimer = setTimeout(() => builderPreview.load(builderEffect(), (msg) => ($("#b-error").textContent = msg)), 180);
}

function recipeRow(label, control, help) {
  return el("div", { class: "recipe-row" }, [
    el("div", { class: "recipe-label" }, [label, ...(help ? [el("div", { class: "muted small" }, help)] : [])]),
    control,
  ]);
}

function recipeSlider(key, min, max, step, suffix) {
  const value = el("span", { class: "muted small slider-value" }, builder.recipe[key] + suffix);
  const input = el("input", { type: "range", min, max, step, value: builder.recipe[key] });
  input.addEventListener("input", () => {
    builder.recipe[key] = Number(input.value);
    value.textContent = input.value + suffix;
    builderChanged();
  });
  return el("div", { class: "slider" }, [input, value]);
}

function renderRecipe() {
  const r = builder.recipe;
  const pick = (key) => segmented(RECIPE_CHOICES[key], r[key], (v) => {
    r[key] = v;
    renderRecipe();
    builderChanged();
  });
  const pane = $("#b-recipe");
  const rows = [
    el("div", { class: "recipe-step" }, "1 · Color"),
    recipeRow("Colors", pick("color")),
  ];
  if (r.color !== "rainbow") {
    rows.push(recipeRow("", colorRow(builder.recipeColors, 4, (structural) => {
      if (structural) renderRecipe();
      builderChanged();
    })));
  }
  rows.push(el("div", { class: "recipe-step" }, "2 · Shape"), recipeRow("Where it's lit", pick("shape")));
  if (r.shape === "bands" || r.shape === "segments") rows.push(recipeRow("How many", recipeSlider("bands", 1, 6, 1, "")));
  if (r.shape === "bar" || r.shape === "mirror") rows.push(recipeRow("Width", recipeSlider("width", 5, 90, 5, "%")));
  rows.push(el("div", { class: "recipe-step" }, "3 · Motion"), recipeRow("How it moves", pick("motion")));
  rows.push(el("div", { class: "recipe-step" }, "4 · Finish"));
  if ((r.shape === "dot" || r.shape === "bar") && (r.motion === "spin" || r.motion === "spin_back")) {
    rows.push(recipeRow("Tail", recipeSlider("tail", 0, 90, 5, "%"), "A fading trail behind it"));
  }
  if (r.shape !== "all" && r.shape !== "segments") rows.push(recipeRow("Softness", recipeSlider("softness", 5, 100, 5, "%"), "Hard edges to soft glow"));
  rows.push(recipeRow("Dimmest", recipeSlider("min", 0, 90, 5, "%"), "How dark the unlit parts get"));
  rows.push(recipeRow("Brightest", recipeSlider("max", 10, 100, 5, "%")));
  rows.push(recipeRow("Fans out of step", recipeSlider("offset", 0, 50, 5, "%"), "Each fan starts a little later, so it flows across them"));
  const alt = el("input", { type: "checkbox" });
  alt.checked = r.alternate;
  alt.addEventListener("change", () => {
    r.alternate = alt.checked;
    builderChanged();
  });
  rows.push(recipeRow("Every other fan reversed", el("label", { class: "check" }, [alt, "Mirror alternate fans"])));
  const code = el("details", { class: "recipe-code" }, [
    el("summary", {}, "Generated script"),
    el("pre", { id: "b-recipe-code" }, recipeScript(r)),
    el("button", { class: "btn mini", onclick: editRecipeByHand }, "Edit by hand"),
  ]);
  rows.push(code);
  pane.replaceChildren(...rows);
}

function editRecipeByHand() {
  $("#b-script").value = recipeScript(builder.recipe);
  builder.scriptColors = builder.recipeColors.slice();
  renderScriptColors();
  setBuilderMode("script");
  toast("The script is yours to edit now; the recipe controls no longer apply");
}

function renderLayers() {
  const list = $("#b-layers");
  list.replaceChildren();
  builder.layers.forEach((layer, index) => {
    const info = kindInfo(layer.effect.kind);
    const kind = el("select", {}, KINDS.map((k) => el("option", { value: k.id }, k.label)));
    kind.value = layer.effect.kind;
    kind.addEventListener("change", () => {
      layer.effect.kind = kind.value;
      layer.effect.colors = defaultLayer(kind.value).effect.colors;
      renderLayers();
      builderChanged();
    });
    const blend = el("select", {}, Object.entries(BLENDS).map(([v, label]) => el("option", { value: v }, label)));
    blend.value = layer.blend;
    blend.addEventListener("change", () => { layer.blend = blend.value; builderChanged(); });
    const speed = el("input", { type: "range", min: 1, max: 5, value: layer.effect.speed });
    speed.addEventListener("input", () => { layer.effect.speed = Number(speed.value); builderChanged(); });
    const opacity = el("input", { type: "range", min: 5, max: 100, step: 5, value: layer.opacity });
    opacity.addEventListener("input", () => { layer.opacity = Number(opacity.value); builderChanged(); });
    const reverse = el("input", { type: "checkbox" });
    reverse.checked = layer.effect.reverse;
    reverse.addEventListener("change", () => { layer.effect.reverse = reverse.checked; builderChanged(); });
    const hexes = layer.effect.colors.map(rgbToHex);
    const colors = info.colors
      ? colorRow(hexes, info.colors, (structural) => {
          layer.effect.colors = hexes.map(hexToRgb);
          if (structural) renderLayers();
          builderChanged();
        })
      : el("span", { class: "muted small" }, "Uses its own colors");
    const move = (delta) => {
      const target = index + delta;
      if (target < 0 || target >= builder.layers.length) return;
      [builder.layers[index], builder.layers[target]] = [builder.layers[target], builder.layers[index]];
      renderLayers();
      builderChanged();
    };
    list.append(el("div", { class: "layer" }, [
      el("div", { class: "layer-head" }, [
        el("span", { class: "layer-n" }, String(index + 1)),
        kind,
        el("label", { class: "field" }, ["Blend", blend]),
        el("span", { class: "grow" }),
        el("button", { class: "btn mini", title: "Move up", onclick: () => move(-1) }, "↑"),
        el("button", { class: "btn mini", title: "Move down", onclick: () => move(1) }, "↓"),
        el("button", { class: "btn mini", title: "Remove layer", onclick: () => { builder.layers.splice(index, 1); renderLayers(); builderChanged(); } }, "×"),
      ]),
      el("div", { class: "layer-body" }, [
        colors,
        el("label", { class: "field" }, ["Speed", speed]),
        el("label", { class: "field" }, ["Opacity", opacity]),
        ...(info.reverse ? [el("label", { class: "check" }, [reverse, "Reverse"])] : []),
      ]),
    ]));
  });
  $("#b-add-layer").hidden = builder.layers.length >= 6;
}

function renderScriptColors() {
  $("#b-script-colors").replaceChildren(colorRow(builder.scriptColors, 4, (structural) => {
    if (structural) renderScriptColors();
    builderChanged();
  }));
}

function setBuilderMode(mode) {
  builder.mode = mode;
  document.querySelectorAll("#b-mode button").forEach((b) => b.classList.toggle("on", b.dataset.mode === mode));
  $("#b-recipe").hidden = mode !== "recipe";
  $("#b-layers-pane").hidden = mode !== "custom";
  $("#b-script-pane").hidden = mode !== "script";
  $("#b-speed-row").hidden = mode === "custom";
  $("#b-error").textContent = "";
  builderChanged();
}

async function openBuilder(effect) {
  builder.original = effect ? effect.name : null;
  $("#b-name").value = effect ? effect.name : "";
  let recipe = null;
  if (effect && effect.recipe) {
    try {
      recipe = { ...RECIPE_DEFAULT, ...JSON.parse(effect.recipe) };
    } catch (e) {}
  }
  builder.recipe = recipe || { ...RECIPE_DEFAULT };
  builder.recipeColors = recipe && effect.colors.length ? effect.colors.map(rgbToHex) : ["#ff0080", "#00c8ff"];
  builder.layers = effect && effect.kind === "custom" ? JSON.parse(JSON.stringify(effect.layers)) : [defaultLayer("rainbow_wave"), { ...defaultLayer("twinkle"), blend: "brightest" }];
  builder.scriptColors = effect && effect.kind === "script" && !recipe && effect.colors.length ? effect.colors.map(rgbToHex) : ["#ff0080", "#00c8ff"];
  $("#b-script").value = effect && effect.kind === "script" && !recipe ? effect.script : await invoke("script_template");
  $("#b-speed").value = effect ? effect.speed || 3 : 3;
  $("#b-delete").hidden = !effect;
  renderRecipe();
  renderLayers();
  renderScriptColors();
  $("#builder").hidden = false;
  setBuilderMode(effect ? (effect.kind === "custom" ? "custom" : recipe ? "recipe" : "script") : "recipe");
}

function closeBuilder() {
  $("#builder").hidden = true;
  builderPreview.show([], 40);
}

document.querySelectorAll("#b-mode button").forEach((b) => b.addEventListener("click", () => setBuilderMode(b.dataset.mode)));
$("#b-add-layer").addEventListener("click", () => {
  builder.layers.push({ ...defaultLayer("breathing"), blend: "add", opacity: 60 });
  renderLayers();
  builderChanged();
});
$("#b-script").addEventListener("input", builderChanged);
$("#b-script").addEventListener("keydown", (e) => {
  if (e.key === "Tab") {
    e.preventDefault();
    const t = e.target, s = t.selectionStart;
    t.value = t.value.slice(0, s) + "    " + t.value.slice(t.selectionEnd);
    t.selectionStart = t.selectionEnd = s + 4;
  }
});
$("#b-speed").addEventListener("input", builderChanged);
$("#b-close").addEventListener("click", closeBuilder);
$("#b-save").addEventListener("click", async () => {
  const effect = builderEffect();
  if (!effect.name) return toast("Give the effect a name first", true);
  if (builder.original && builder.original !== effect.name) {
    await call("delete_custom_effect", { name: builder.original }).catch(() => {});
  }
  await call("save_custom_effect", { effect }, `Saved ${effect.name}`);
  closeBuilder();
  await loadMine();
});
$("#b-delete").addEventListener("click", async () => {
  if (!builder.original) return;
  await call("delete_custom_effect", { name: builder.original }, `Deleted ${builder.original}`);
  closeBuilder();
  loadMine();
});

// ---------------------------------------------------------------- effects page

const libraryPlayers = [];

function renderLibrary() {
  libraryPlayers.splice(0).forEach((p) => p.show([], 40));
  const grid = $("#library");
  if (!MINE.length) {
    grid.replaceChildren(el("div", { class: "empty" }, [
      el("div", {}, "No custom effects yet."),
      el("div", { class: "muted small" }, "Make one from a recipe, from layers of effects, or with a script."),
    ]));
    return;
  }
  const visible = $("#page-effects").classList.contains("active");
  grid.replaceChildren(...MINE.map((e) => {
    const canvas = el("canvas", { class: "lib-preview", width: "120", height: "120" });
    if (visible) {
      const player = previewPlayer(canvas);
      player.load(e);
      libraryPlayers.push(player);
    }
    const apply = el("button", { class: "btn mini primary" }, "Use");
    apply.addEventListener("click", () => {
      document.querySelector('.nav[data-page="lighting"]').click();
      openDrawer({ type: "all" });
      drawer.category = "mine";
      drawer.kind = e.kind;
      drawer.custom = e;
      drawer.speed = e.speed || 3;
      renderDrawer();
    });
    return el("div", { class: "card lib-card" }, [
      canvas,
      el("div", { class: "lib-name" }, e.name),
      el("div", { class: "muted small" }, customType(e)),
      el("div", { class: "row" }, [
        apply,
        el("button", { class: "btn mini", onclick: () => openBuilder(e) }, "Edit"),
      ]),
    ]);
  }));
}

$("#lib-new").addEventListener("click", () => openBuilder(null));
$("#lib-import").addEventListener("click", async () => {
  const n = await call("import_custom_effects");
  if (n) toast(`Imported ${n} effect${n === 1 ? "" : "s"}`);
  loadMine();
});
$("#lib-export").addEventListener("click", async () => {
  const path = await call("export_custom_effects");
  if (path) toast("Saved to " + path);
});
document.addEventListener("effects-changed", renderLibrary);
document.querySelector('.nav[data-page="effects"]').addEventListener("click", () => setTimeout(renderLibrary, 0));
document.querySelectorAll('.nav:not([data-page="effects"])').forEach((n) =>
  n.addEventListener("click", () => libraryPlayers.splice(0).forEach((p) => p.show([], 40)))
);
