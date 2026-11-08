// Widgets over the HydroShift screen's video: placed on the preview, applied per video.

const READOUT_KINDS = {
  clock: { name: "Clock", label: "" },
  date: { name: "Date", label: "" },
  coolant: { name: "Coolant temperature", label: "COOLANT" },
  cpu_temp: { name: "CPU temperature", label: "CPU" },
  pump: { name: "Pump speed", label: "PUMP" },
  gpu_temp: { name: "GPU temperature", label: "GPU" },
  gpu_load: { name: "GPU load", label: "GPU LOAD" },
  fans: { name: "Fan speed", label: "FANS" },
  text: { name: "Text", label: "" },
};

/// Snapping reach, in thousandths of the preview.
const SNAP = 14;

const SHARED = "*";
const readouts = { file: null, own: false, savedOwn: false, modules: [], saved: "[]", values: {}, active: -1, open: new Set(), dragging: false, fonts: null, cpu: null };

const withDefaults = (m) => ({ font: "", opacity: 100, shadow: true, caption_scale: 38, label: "", text: "", hour12: false, ...m });

function readoutValue(m) {
  const v = readouts.values;
  const dash = (x, f) => (x == null ? "–" : f(x));
  const now = new Date();
  switch (m.kind) {
    case "clock": {
      const h = now.getHours(), min = String(now.getMinutes()).padStart(2, "0");
      return m.hour12 ? `${h % 12 || 12}:${min} ${h < 12 ? "AM" : "PM"}` : `${String(h).padStart(2, "0")}:${min}`;
    }
    case "date": return now.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" }).replace(",", "");
    case "coolant": return dash(v.coolant_c, (c) => `${c}°C`);
    case "cpu_temp": return dash(v.cpu_temp_c, (c) => `${c}°C`);
    case "pump": return dash(v.pump_rpm, (r) => `${r} RPM`);
    case "gpu_temp": return dash(v.gpu_temp_c, (c) => `${c}°C`);
    case "gpu_load": return dash(v.gpu_load, (p) => `${p}%`);
    case "fans": return dash(v.fans_rpm, (r) => `${r} RPM`);
    default: return m.text || "";
  }
}

const fontFamily = (id) => {
  if (!id) return '"Bahnschrift", "Segoe UI", "DejaVu Sans", sans-serif';
  if (id.startsWith("file:")) return `"w-file-${fontKey(id)}", sans-serif`;
  return `"w-${id}", sans-serif`;
};
const fontKey = (id) => [...id].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7).toString(36);

async function loadWidgetFonts() {
  if (readouts.fonts) return readouts.fonts;
  try {
    readouts.fonts = await invoke("widget_fonts");
  } catch {
    readouts.fonts = [];
  }
  for (const f of readouts.fonts.filter((f) => f.file)) {
    const face = new FontFace(`w-file-${fontKey(f.id)}`, `url("${assetUrl(f.file)}")`);
    face.load().then((loaded) => document.fonts.add(loaded)).catch(() => {});
  }
  drawWidgetList();
  return readouts.fonts;
}

const widgetsDirty = () => JSON.stringify(readouts.modules) !== readouts.saved || readouts.own !== readouts.savedOwn;

function updateWidgetActions() {
  $("#widgets-apply").disabled = !readouts.file || !widgetsDirty();
  $("#widgets-discard").disabled = !readouts.file || !widgetsDirty();
}

/// Called whenever the screen page shows a different video (or none). Unapplied changes are
/// kept while the same video stays up, so a refresh never interrupts editing.
function syncReadouts(path) {
  if (path === readouts.file) return;
  readouts.file = path || null;
  readouts.active = -1;
  readouts.open.clear();
  const saved = (last && last.oled && last.oled.modules) || {};
  readouts.own = !!(path && saved[path]);
  readouts.savedOwn = readouts.own;
  readouts.modules = (saved[path] || saved[SHARED] || []).map(withDefaults);
  readouts.saved = JSON.stringify(readouts.modules);
  $("#widgets-own").checked = readouts.own;
  drawReadouts();
}

function widgetsChanged() {
  updateWidgetActions();
}

async function applyWidgets() {
  const file = readouts.file;
  if (!file) return;
  const modules = structuredClone(readouts.modules);
  const key = readouts.own ? file : SHARED;
  // a video going back to the shared layout drops its own
  if (!readouts.own && readouts.savedOwn) await invoke("set_oled_modules", { file, modules: [] });
  const done = file === oled.current ? "Widgets sent to the screen" : readouts.own ? "Widgets saved for this video" : "Widgets saved for every video";
  await call("set_oled_modules", { file: key, modules }, done);
  readouts.saved = JSON.stringify(modules);
  readouts.savedOwn = readouts.own;
  if (last && last.oled) {
    const all = { ...(last.oled.modules || {}) };
    if (!readouts.own) delete all[file];
    all[key] = modules;
    if (!modules.length) delete all[key];
    last.oled.modules = all;
  }
  updateWidgetActions();
}

function discardWidgets() {
  readouts.own = readouts.savedOwn;
  $("#widgets-own").checked = readouts.own;
  readouts.modules = JSON.parse(readouts.saved);
  readouts.active = -1;
  drawReadouts();
}

const hexOf = (rgb) => "#" + rgb.map((c) => c.toString(16).padStart(2, "0")).join("");

async function drawCpuSetup() {
  const box = $("#cpu-setup");
  const wanted = readouts.modules.some((m) => m.kind === "cpu_temp");
  if (!wanted) return (box.hidden = true);
  if (!readouts.cpu) {
    try {
      readouts.cpu = await invoke("cpu_temp_status");
    } catch {
      return;
    }
  }
  const s = readouts.cpu;
  box.hidden = s.task && s.pawnio;
  if (box.hidden) return;
  if (!s.supported) {
    box.replaceChildren(el("span", { class: "muted small" }, "CPU temperature is only supported on Intel CPUs for now."));
    return;
  }
  const button = el("button", { class: "btn primary" }, "Set up CPU temperature");
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "Waiting for the administrator prompt…";
    try {
      await call("setup_cpu_temp", {}, "CPU temperature is set up");
      readouts.cpu = null;
    } finally {
      button.disabled = false;
      button.textContent = "Set up CPU temperature";
      drawCpuSetup();
    }
  });
  box.replaceChildren(
    el("span", { class: "muted small" }, "Windows only lets administrators read the CPU temperature. One administrator prompt sets up a small helper (and PawnIO, the driver OpenRGB uses, if it's missing); after that it works on its own."),
    button,
  );
}

function drawReadouts() {
  $("#readout-add").disabled = !readouts.file;
  drawCpuSetup();
  drawWidgetLayer();
  drawWidgetList();
  updateWidgetActions();
}

function styleWidget(node, m, height) {
  const size = (m.size / 1000) * height;
  node.style.left = `${m.x / 10}%`;
  node.style.top = `${m.y / 10}%`;
  node.style.color = hexOf(m.color);
  node.style.fontSize = `${size}px`;
  node.style.fontFamily = fontFamily(m.font);
  node.style.opacity = String(m.opacity / 100);
  node.style.textShadow = m.shadow ? "0.05em 0.05em 0 rgba(0, 0, 0, .6)" : "none";
  const caption = node.querySelector(".readout-caption");
  if (caption) caption.style.fontSize = `${(size * m.caption_scale) / 100}px`;
}

function drawWidgetLayer() {
  const layer = $("#oled-readouts");
  if (readouts.dragging) return;
  if (!readouts.file) {
    layer.replaceChildren();
    return;
  }
  const height = layer.clientHeight || 1;
  layer.replaceChildren(...readouts.modules.map((m, i) => {
    const node = el("div", { class: "readout" + (i === readouts.active ? " active" : ""), title: "Drag to move" }, [
      m.label ? el("span", { class: "readout-caption" }, m.label) : null,
      el("span", { class: "readout-value" }, readoutValue(m)),
      el("span", { class: "readout-size", title: "Drag to resize" }),
    ].filter(Boolean));
    styleWidget(node, m, height);
    readoutDrag(node, m, i);
    return node;
  }), el("div", { class: "snap-line v", hidden: "" }), el("div", { class: "snap-line h", hidden: "" }));
}

function widgetSlider(label, min, max, value, unit, onInput) {
  const range = el("input", { type: "range", min: String(min), max: String(max), value: String(value) });
  const number = el("input", { type: "number", class: "text num", min: String(min), max: String(max), value: String(value) });
  const set = (v) => {
    v = Math.max(min, Math.min(max, Math.round(Number(v) || 0)));
    range.value = number.value = String(v);
    onInput(v);
  };
  range.addEventListener("input", () => set(range.value));
  number.addEventListener("change", () => set(number.value));
  return el("label", { class: "adv-field" }, [el("span", { class: "muted small" }, label), range, number, el("span", { class: "muted small" }, unit)]);
}

function drawWidgetList() {
  const list = $("#readout-list");
  if (!readouts.file) {
    list.replaceChildren(el("div", { class: "empty small" }, "Pick a video first."));
    return;
  }
  if (!readouts.modules.length) {
    list.replaceChildren(el("div", { class: "empty small" }, "No widgets on this video."));
    return;
  }
  const update = () => { drawWidgetLayer(); widgetsChanged(); };
  list.replaceChildren(...readouts.modules.map((m, i) => {
    const label = el("input", { type: "text", class: "text", placeholder: "Caption (optional)", value: m.label || "" });
    label.addEventListener("input", () => { m.label = label.value; update(); });
    const color = el("input", { type: "color", value: hexOf(m.color), title: "Color" });
    color.addEventListener("input", () => {
      const hex = color.value.slice(1);
      m.color = [0, 2, 4].map((k) => parseInt(hex.slice(k, k + 2), 16));
      update();
    });
    const font = el("select", { class: "widget-font", title: "Font" }, [
      el("option", { value: "" }, "Default font"),
      ...(readouts.fonts || []).map((f) => el("option", { value: f.id }, f.name)),
    ]);
    if (m.font && !(readouts.fonts || []).some((f) => f.id === m.font)) font.append(el("option", { value: m.font }, "Saved font"));
    font.value = m.font || "";
    font.addEventListener("change", () => { m.font = font.value; update(); });
    const extra = [];
    if (m.kind === "text") {
      const text = el("input", { type: "text", class: "text", placeholder: "Text to show", value: m.text || "" });
      text.addEventListener("input", () => { m.text = text.value; update(); });
      extra.push(text);
    }
    if (m.kind === "clock") {
      const h12 = el("label", { class: "check" }, [el("input", { type: "checkbox" }), "12-hour (AM/PM)"]);
      h12.querySelector("input").checked = !!m.hour12;
      h12.querySelector("input").addEventListener("change", (e) => { m.hour12 = e.target.checked; update(); });
      extra.push(h12);
    }
    const open = readouts.open.has(i);
    const advanced = el("button", { class: "icon-btn switchable" + (open ? " on" : ""), title: "Advanced: position, size, opacity" }, "⚙");
    advanced.addEventListener("click", () => {
      if (open) readouts.open.delete(i);
      else readouts.open.add(i);
      drawWidgetList();
    });
    const remove = el("button", { class: "icon-btn", title: "Remove" }, "✕");
    remove.addEventListener("click", () => {
      readouts.modules.splice(i, 1);
      readouts.open.clear();
      readouts.active = -1;
      drawReadouts();
    });
    const main = el("div", { class: "readout-row-main" }, [
      el("span", { class: "kind" }, READOUT_KINDS[m.kind].name),
      ...extra,
      label,
      font,
      color,
      advanced,
      remove,
    ]);
    const parts = [main];
    if (open) {
      const shadow = el("label", { class: "check" }, [el("input", { type: "checkbox" }), "Shadow"]);
      shadow.querySelector("input").checked = m.shadow;
      shadow.querySelector("input").addEventListener("change", (e) => { m.shadow = e.target.checked; update(); });
      parts.push(el("div", { class: "readout-adv" }, [
        widgetSlider("Left ↔ right", 0, 1000, m.x, "", (v) => { m.x = v; update(); }),
        widgetSlider("Up ↕ down", 0, 1000, m.y, "", (v) => { m.y = v; update(); }),
        widgetSlider("Size", 20, 600, m.size, "", (v) => { m.size = v; update(); }),
        widgetSlider("Caption size", 15, 100, m.caption_scale, "%", (v) => { m.caption_scale = v; update(); }),
        widgetSlider("Opacity", 10, 100, m.opacity, "%", (v) => { m.opacity = v; update(); }),
        shadow,
      ]));
    }
    const row = el("div", { class: "readout-row" + (i === readouts.active ? " active" : "") }, parts);
    row.addEventListener("pointerdown", () => {
      if (readouts.active === i) return;
      readouts.active = i;
      document.querySelectorAll(".readout-row").forEach((r, k) => r.classList.toggle("active", k === i));
      document.querySelectorAll("#oled-readouts .readout").forEach((r, k) => r.classList.toggle("active", k === i));
    });
    return row;
  }));
}

/// Where a dragged widget's center snaps: the preview's middle and the other widgets' centers.
function snapped(value, axis, index, free) {
  if (free) return { value, line: null };
  const targets = [500, ...readouts.modules.filter((_, k) => k !== index).map((m) => m[axis])];
  let best = null;
  for (const t of targets) {
    if (Math.abs(t - value) <= SNAP && (best === null || Math.abs(t - value) < Math.abs(best - value))) best = t;
  }
  return best === null ? { value, line: null } : { value: best, line: best };
}

function readoutDrag(node, m, i) {
  node.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const layer = $("#oled-readouts");
    const box = layer.getBoundingClientRect();
    const resizing = e.target.classList.contains("readout-size");
    const start = { x: e.clientX, y: e.clientY, size: m.size, mx: m.x, my: m.y };
    const vLine = layer.querySelector(".snap-line.v");
    const hLine = layer.querySelector(".snap-line.h");
    readouts.active = i;
    readouts.dragging = true;
    document.querySelectorAll(".readout-row").forEach((r, k) => r.classList.toggle("active", k === i));
    document.querySelectorAll("#oled-readouts .readout").forEach((r, k) => r.classList.toggle("active", k === i));
    node.setPointerCapture(e.pointerId);
    const move = (ev) => {
      if (resizing) {
        const grow = ((ev.clientX - start.x) + (ev.clientY - start.y)) / box.height * 1000;
        m.size = Math.round(Math.min(600, Math.max(20, start.size + grow)));
      } else {
        const x = start.mx + ((ev.clientX - start.x) / box.width) * 1000;
        const y = start.my + ((ev.clientY - start.y) / box.height) * 1000;
        const sx = snapped(Math.round(Math.min(1000, Math.max(0, x))), "x", i, ev.altKey);
        const sy = snapped(Math.round(Math.min(1000, Math.max(0, y))), "y", i, ev.altKey);
        m.x = sx.value;
        m.y = sy.value;
        vLine.hidden = sx.line === null;
        hLine.hidden = sy.line === null;
        if (sx.line !== null) vLine.style.left = `${sx.line / 10}%`;
        if (sy.line !== null) hLine.style.top = `${sy.line / 10}%`;
      }
      styleWidget(node, m, box.height);
    };
    const end = () => {
      node.removeEventListener("pointermove", move);
      node.removeEventListener("pointerup", end);
      node.removeEventListener("pointercancel", end);
      node.removeEventListener("lostpointercapture", end);
      vLine.hidden = hLine.hidden = true;
      readouts.dragging = false;
      widgetsChanged();
      if (readouts.open.has(i)) drawWidgetList();
    };
    node.addEventListener("pointermove", move);
    node.addEventListener("pointerup", end);
    node.addEventListener("pointercancel", end);
    node.addEventListener("lostpointercapture", end);
  });
}

$("#readout-add").addEventListener("change", (e) => {
  const kind = e.target.value;
  e.target.value = "";
  if (!kind || !readouts.file) return;
  const n = readouts.modules.length;
  readouts.modules.push(withDefaults({
    kind,
    x: 500,
    y: Math.min(900, 200 + n * 150),
    size: kind === "clock" ? 260 : 160,
    color: [255, 255, 255],
    label: READOUT_KINDS[kind].label,
    text: kind === "text" ? "RGBable" : "",
  }));
  readouts.active = n;
  drawReadouts();
});
$("#widgets-apply").addEventListener("click", applyWidgets);
$("#widgets-own").addEventListener("change", (e) => {
  readouts.own = e.target.checked;
  updateWidgetActions();
});
$("#widgets-discard").addEventListener("click", discardWidgets);

// Live values for the preview, only while the screen page is open and has widgets.
setInterval(async () => {
  if (!$("#page-oled").classList.contains("active")) return;
  loadWidgetFonts();
  if (!readouts.modules.length) return;
  try {
    readouts.values = await invoke("oled_readings", { cpu: readouts.modules.some((m) => m.kind === "cpu_temp") });
  } catch {
    return;
  }
  document.querySelectorAll("#oled-readouts .readout-value").forEach((v, i) => {
    const m = readouts.modules[i];
    if (m) v.textContent = readoutValue(m);
  });
}, 2000);

// Sizes follow the preview's height, which is 0 until the page is shown.
new ResizeObserver(() => readouts.file && drawWidgetLayer()).observe($("#oled-readouts"));
