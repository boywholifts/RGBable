// Effect catalogue shared by the Lighting page, the effect panel and the effect library.

const KINDS = [
  { id: "static", label: "Static", colors: 1 },
  { id: "breathing", label: "Breathing", colors: 4 },
  { id: "color_cycle", label: "Color cycle", colors: 4 },
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
  { id: "starry_night", label: "Starry night", colors: 2, hint: "The first color is the sky, the second the stars.", defaults: ["#2a0aa0", "#ffffff"] },
  { id: "aurora", label: "Aurora", colors: 4, reverse: true, defaults: ["#00ffa0", "#8000ff", "#00a0ff"] },
  { id: "police", label: "Police", colors: 2, defaults: ["#ff0000", "#0040ff"] },
  { id: "drizzle", label: "Drizzle", colors: 6, reverse: true, hint: "Lian Li's Drizzling. Strimers play their built-in version.", defaults: ["#00e5ff", "#8000ff", "#ff2040"] },
];

const CATEGORIES = [
  { id: "basic", label: "Basic", kinds: ["static", "breathing", "color_cycle", "rainbow", "heartbeat", "strobe"] },
  { id: "motion", label: "Motion", kinds: ["wave", "rainbow_wave", "meteor", "drizzle", "scanner", "runway", "fill", "ripple", "police"] },
  { id: "ambient", label: "Ambient", kinds: ["twinkle", "starry_night", "fire", "aurora"] },
  { id: "lianli", label: "Lian Li" },
  { id: "mine", label: "Mine" },
];

const kindInfo = (id) => KINDS.find((k) => k.id === id) || KINDS[0];
let LIANLI = [];
let MINE = [];
const DIRECTIONS = { Clockwise: "Clockwise", CounterClockwise: "Counter-clockwise", Up: "Up", Down: "Down", Spread: "Spread out", Gather: "Gather in" };
const MODE_NAMES = { RainbowMorph: "Rainbow morph", ColorCycle: "Color cycle", ElectricCurrent: "Electric current", ShuttleRun: "Shuttle run", GradientRibbon: "Gradient ribbon", PingPong: "Ping-pong" };
const modeName = (mode) => MODE_NAMES[mode] || mode.replace(/([a-z])([A-Z])/g, "$1 $2");
const lianliInfo = (mode) => LIANLI.find((m) => m.mode === mode);
const isCustom = (effect) => !!effect && (effect.kind === "custom" || effect.kind === "script");

function kindLabelOf(effect) {
  if (!effect) return "Color";
  if (effect.kind === "lianli") return modeName(effect.mode || "");
  if (isCustom(effect)) return effect.name || "Custom";
  return kindInfo(effect.kind).label;
}

function customType(effect) {
  if (effect.kind === "custom") return "Layers";
  return effect.recipe ? "Recipe" : "Script";
}

const RAINBOW = ["#ff0000", "#ffff00", "#00ff00", "#00ffff", "#0000ff", "#ff00ff"];
const FIRE_COLORS = ["#c81400", "#ff6e00", "#ffc828"];
const PULSING = ["breathing", "heartbeat", "strobe", "police", "color_cycle"];

function effectColors(effect) {
  if (effect.kind === "rainbow" || effect.kind === "rainbow_wave") return RAINBOW;
  if (effect.kind === "lianli" && !effect.colors.length) return RAINBOW;
  if (effect.kind === "custom") {
    const colors = (effect.layers || []).flatMap((l) => (l.effect.colors.length ? l.effect.colors.map(rgbToHex) : RAINBOW.slice(0, 3)));
    return colors.length ? colors : RAINBOW;
  }
  if (effect.kind === "fire") return FIRE_COLORS;
  const colors = effect.colors.map(rgbToHex);
  return colors.length ? colors : ["#8000ff"];
}

/// A color box. With an effect it shows the effect's colors moving; picking a color sets a plain color.
function swatch(color, onPick, effect) {
  const input = el("input", { type: "color", value: rgbToHex(color) });
  const animated = effect && effect.kind !== "static";
  const label = el("label", { class: "swatch", title: animated ? `${kindLabelOf(effect)}: pick a color to replace it` : "Pick a color" }, input);
  label.style.setProperty("--glow", rgbToHex(color));
  if (animated) {
    const colors = effectColors(effect);
    input.value = colors[0];
    label.classList.add("fx", PULSING.includes(effect.kind) ? "pulse" : "flow");
    label.style.setProperty("--fx", `linear-gradient(90deg, ${[...colors, colors[0]].join(", ")})`);
    label.style.setProperty("--glow", colors[0]);
    label.style.setProperty("--fx-time", `${[6, 4.2, 3, 2.1, 1.5][(effect.speed || 3) - 1]}s`);
  }
  input.addEventListener("input", () => {
    label.classList.remove("fx", "pulse", "flow");
    label.style.setProperty("--glow", input.value);
  });
  input.addEventListener("change", () => onPick(hexToRgb(input.value)));
  return label;
}

/// Editable list of color boxes; `onChange(structural)` is told whether boxes were added or removed.
function colorRow(colors, max, onChange) {
  const row = el("div", { class: "fx-colors" });
  colors.forEach((hex, i) => {
    const s = swatch(hexToRgb(hex), () => {});
    s.classList.add("small");
    const input = s.querySelector("input");
    input.addEventListener("input", () => {
      colors[i] = input.value;
      onChange(false);
    });
    const wrap = el("div", { class: "fx-color" }, s);
    if (colors.length > 1) {
      wrap.append(el("button", { class: "remove", title: "Remove color", onclick: () => { colors.splice(i, 1); onChange(true); } }, "×"));
    }
    row.append(wrap);
  });
  if (colors.length < max) {
    row.append(el("button", { class: "btn mini", title: "Add a color", onclick: () => { colors.push(PRESETS[colors.length % PRESETS.length]); onChange(true); } }, "+"));
  }
  return row;
}

function segmented(options, value, onChange) {
  const wrap = el("div", { class: "seg" });
  for (const [v, label] of options) {
    wrap.append(el("button", { class: v === value ? "on" : "", onclick: () => onChange(v) }, label));
  }
  return wrap;
}



/// Plays preview frames (13 inner + 13 outer LEDs) on a canvas as a fan.
function previewPlayer(canvas) {
  const player = { frames: [], interval: 40, index: 0, timer: null, seq: 0 };
  const draw = () => {
    const ctx = canvas.getContext("2d");
    const w = canvas.width, h = canvas.height, cx = w / 2, cy = h / 2;
    ctx.clearRect(0, 0, w, h);
    const frame = player.frames[player.index] || [];
    const ring = (offset, radius, size) => {
      for (let i = 0; i < 13; i++) {
        const a = -Math.PI / 2 + (i / 13) * Math.PI * 2;
        const c = frame[offset + i] || [20, 20, 28];
        const x = cx + Math.cos(a) * radius, y = cy + Math.sin(a) * radius;
        const color = `rgb(${c[0]},${c[1]},${c[2]})`;
        ctx.shadowColor = color;
        ctx.shadowBlur = size * 1.6;
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.arc(x, y, size, 0, Math.PI * 2);
        ctx.fill();
      }
    };
    ctx.shadowBlur = 0;
    ctx.fillStyle = "#15111d";
    ctx.beginPath();
    ctx.arc(cx, cy, w * 0.47, 0, Math.PI * 2);
    ctx.fill();
    ring(13, w * 0.4, w * 0.035);
    ring(0, w * 0.25, w * 0.03);
  };
  player.show = (frames, interval) => {
    player.frames = frames;
    player.interval = Math.max(15, interval);
    player.index = 0;
    clearInterval(player.timer);
    draw();
    if (frames.length > 1) {
      player.timer = setInterval(() => {
        player.index = (player.index + 1) % player.frames.length;
        draw();
      }, player.interval);
    }
  };
  player.load = async (effect, onError) => {
    const seq = ++player.seq;
    try {
      const p = await invoke("preview_effect", { effect });
      if (seq === player.seq) {
        player.show(p.frames, p.interval_ms);
        if (onError) onError("");
      }
    } catch (e) {
      if (seq === player.seq && onError) onError(String(e));
    }
  };
  draw();
  return player;
}

async function loadEffectCatalogue() {
  try {
    LIANLI = (await invoke("lianli_effects")) || [];
  } catch (e) {
    LIANLI = [];
  }
  await loadMine();
}

async function loadMine() {
  try {
    MINE = (await invoke("custom_effects")) || [];
  } catch (e) {
    MINE = [];
  }
  document.dispatchEvent(new Event("effects-changed"));
}
