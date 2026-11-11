// RGBable UI. Talks to the Rust side through Tauri commands.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (sel) => document.querySelector(sel);

const DEFAULT_PRESETS = ["#8000ff", "#ff2040", "#00e5ff", "#ffffff", "#ff8a00", "#00ff6a"];
let PRESETS = DEFAULT_PRESETS.slice();
let editingPresets = false;
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
    document.dispatchEvent(new CustomEvent("page-changed", { detail: b.dataset.page }));
  })
);


/// Closes on a click that both starts and ends on the backdrop itself, so selecting text in a
/// dialog and letting go outside it doesn't close it.
function closeOnBackdrop(backdrop, close) {
  let pressed = false;
  backdrop.addEventListener("mousedown", (e) => (pressed = e.target === backdrop));
  backdrop.addEventListener("click", (e) => {
    if (pressed && e.target === backdrop) close();
    pressed = false;
  });
}

const ICONS = {
  reverse: '<path d="M4 8h14l-3-3M20 16H6l3 3"/>',
  power: '<path d="M12 3v8M6.3 6.3a8 8 0 1 0 11.4 0"/>',
  resync: '<path d="M3 12a9 9 0 0 1 15.5-6.2L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15.5 6.2L3 16"/><path d="M3 21v-5h5"/>',
  clock: '<circle cx="12" cy="12" r="8"/><path d="M12 8v4l3 2"/>',
  across: '<path d="M3 12h18M7 8l-4 4 4 4M17 8l4 4-4 4"/>',
  chain: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
};
function icon(name) {
  const span = el("span", { class: "icon" });
  span.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`;
  return span;
}
