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
  cpu_load: { name: "CPU load", label: "CPU LOAD" },
  ram_load: { name: "RAM load", label: "RAM" },
  gpu_power: { name: "GPU power", label: "GPU POWER" },
  gpu_clock: { name: "GPU clock", label: "GPU CLOCK" },
  text: { name: "Text", label: "" },
  bar: { name: "Bar", label: "" },
  image: { name: "Picture", label: "" },
};
const BOXED = new Set(["bar", "image"]);
const readoutNumberKinds = new Set(["coolant", "cpu_temp", "pump", "gpu_temp", "gpu_load", "fans", "cpu_load", "ram_load", "gpu_power", "gpu_clock"]);
const ANCHOR_SHIFT = { center: "translate(-50%, -50%)", top_left: "none", top_center: "translate(-50%, 0)", top_right: "translate(-100%, 0)" };
const ANCHOR_ALIGN = { center: "center", top_left: "left", top_center: "center", top_right: "right" };

/// Snapping reach, in thousandths of the preview.
const SNAP = 14;

const SHARED = "*";
const readouts = { file: null, own: false, savedOwn: false, modules: [], saved: "[]", values: {}, active: -1, open: new Set(), dragging: false, fonts: null, cpu: null };

const withDefaults = (m) => ({
  font: "", opacity: 100, shadow: true, caption_scale: 38, label: "", text: "", hour12: false,
  anchor: "center", unit: true, format: "", file: "", w: 0, h: 0, source: null, back: [40, 40, 48], block: 0, gap: 0,
  ...m,
});
const usesCpuTemp = (m) => m.kind === "cpu_temp" || (m.kind === "bar" && m.source === "cpu_temp");

/// A reading as a number and its unit, as the screen shows it.
function readoutNumber(kind) {
  const v = readouts.values;
  const n = (x, unit) => (x == null ? null : [String(x), unit]);
  switch (kind) {
    case "coolant": return n(v.coolant_c, "°C");
    case "cpu_temp": return n(v.cpu_temp_c, "°C");
    case "pump": return n(v.pump_rpm, " RPM");
    case "gpu_temp": return n(v.gpu_temp_c, "°C");
    case "gpu_load": return n(v.gpu_load, "%");
    case "fans": return n(v.fans_rpm, " RPM");
    case "cpu_load": return n(v.cpu_load, "%");
    case "ram_load": return n(v.ram_load, "%");
    case "gpu_power": return n(v.gpu_power_w, " W");
    case "gpu_clock": return v.gpu_clock_mhz == null ? null : [(v.gpu_clock_mhz / 1000).toFixed(2), " GHz"];
    default: return null;
  }
}

const BAR_FULL = { coolant: 60, cpu_temp: 100, gpu_temp: 100, pump: 3000, fans: 2500, gpu_load: 100, cpu_load: 100, ram_load: 100, gpu_power: 600, gpu_clock: 3000 };
function barFraction(m) {
  const kind = m.source || "cpu_load";
  const number = readoutNumber(kind);
  if (!number) return 0;
  const raw = kind === "gpu_clock" ? Number(number[0]) * 1000 : Number(number[0]);
  return Math.max(0, Math.min(1, raw / BAR_FULL[kind]));
}

function readoutValue(m) {
  const now = new Date();
  const two = (x) => String(x).padStart(2, "0");
  switch (m.kind) {
    case "clock": {
      const h = now.getHours(), min = two(now.getMinutes());
      if (m.format === "AP") return h < 12 ? "AM" : "PM";
      const sec = m.format.includes("s") ? `:${two(now.getSeconds())}` : "";
      return m.hour12 ? `${h % 12 || 12}:${min}${sec} ${h < 12 ? "AM" : "PM"}` : `${two(h)}:${min}${sec}`;
    }
    case "date": {
      const [y, mo, d] = [now.getFullYear(), two(now.getMonth() + 1), two(now.getDate())];
      switch (m.format) {
        case "Y-M-D": return `${y}-${mo}-${d}`;
        case "M-D": return `${mo}-${d}`;
        case "D/M/Y": return `${d}/${mo}/${y}`;
        case "M/D/Y": return `${mo}/${d}/${y}`;
        default: return now.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" }).replace(",", "");
      }
    }
    case "text": return m.text || "";
    case "bar":
    case "image": return "";
    default: {
      const number = readoutNumber(m.kind);
      if (!number) return "–";
      return m.unit ? number[0] + number[1] : number[0];
    }
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
  drawLayoutChoices();
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

/// The helper is set up but no reading has come for a while (it starts within a few seconds).
function cpuTempSilent() {
  return readouts.cpuSilentSince != null && Date.now() - readouts.cpuSilentSince > 20000;
}

async function drawCpuSetup() {
  const box = $("#cpu-setup");
  const wanted = readouts.modules.some(usesCpuTemp);
  if (!wanted) return (box.hidden = true);
  if (!readouts.cpu) {
    try {
      readouts.cpu = await invoke("cpu_temp_status");
    } catch {
      return;
    }
  }
  const s = readouts.cpu;
  const silent = s.task && s.pawnio && cpuTempSilent();
  box.hidden = s.task && s.pawnio && !silent;
  if (box.hidden) return;
  if (!s.supported) {
    box.replaceChildren(el("span", { class: "muted small" }, "CPU temperature is only supported on Intel CPUs for now."));
    return;
  }
  const label = silent ? "Set up again" : "Set up CPU temperature";
  const button = el("button", { class: "btn primary" }, label);
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "Waiting for the administrator prompt…";
    try {
      await call("setup_cpu_temp", {}, "CPU temperature is set up");
      readouts.cpu = null;
    } finally {
      button.disabled = false;
      button.textContent = label;
      readouts.cpuSilentSince = null;
      drawCpuSetup();
    }
  });
  if (silent) {
    box.replaceChildren(
      el("span", { class: "muted small" }, "The CPU temperature helper isn't sending readings. Setting it up again (one administrator prompt) points it at this copy of RGBable, which fixes it after a reinstall or a move."),
      button,
    );
    return;
  }
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
  drawLayoutChoices();
}

function styleWidget(node, m, height) {
  const size = (m.size / 1000) * height;
  node.style.left = `${m.x / 10}%`;
  node.style.top = `${m.y / 10}%`;
  node.style.transform = ANCHOR_SHIFT[m.anchor] || ANCHOR_SHIFT.center;
  node.style.textAlign = ANCHOR_ALIGN[m.anchor] || "center";
  node.style.opacity = String(m.opacity / 100);
  if (BOXED.has(m.kind)) {
    node.style.width = `${m.w / 10}%`;
    node.style.height = `${m.h / 10}%`;
    if (m.kind === "bar") drawBar(node.querySelector(".widget-bar"), m, node);
    return;
  }
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
    let body;
    if (m.kind === "image") body = [el("img", { class: "widget-image", src: m.file ? assetUrl(m.file) : "", alt: "", draggable: "false" })];
    else if (m.kind === "bar") body = [el("div", { class: "widget-bar" })];
    else body = [m.label ? el("span", { class: "readout-caption" }, m.label) : null, el("span", { class: "readout-value" }, readoutValue(m))];
    // a picture over the whole screen (an imported theme's frames) mustn't catch drags meant for the widgets on it
    const backdrop = m.kind === "image" && m.w >= 1000 && m.h >= 1000;
    const classes = "readout" + (BOXED.has(m.kind) ? " boxed" : "") + (backdrop ? " backdrop" : "") + (i === readouts.active ? " active" : "");
    const node = el("div", { class: classes, title: "Drag to move", "data-index": String(i) }, [
      ...body,
      el("span", { class: "readout-size", title: "Drag to resize" }),
    ].filter(Boolean));
    styleWidget(node, m, height);
    readoutDrag(node, m, i);
    return node;
  }), el("div", { class: "snap-line v", hidden: "" }), el("div", { class: "snap-line h", hidden: "" }));
}

function drawBar(bar, m, node) {
  if (!bar) return;
  const width = node.clientWidth || 1;
  const filled = barFraction(m);
  const on = hexOf(m.color), off = hexOf(m.back);
  if (m.block > 0) {
    const scale = width / ((m.w / 1000) * 2288 || 1);
    const block = m.block * scale, period = (m.block + m.gap) * scale;
    // like the screen: a segment lights once the fill reaches its middle
    const lit = Math.max(0, Math.ceil((filled * width - block / 2) / period));
    const litTo = Math.min(width, lit * period);
    bar.style.background = `linear-gradient(90deg, ${on} 0 ${litTo}px, ${off} ${litTo}px 100%)`;
    bar.style.webkitMaskImage = bar.style.maskImage = `repeating-linear-gradient(90deg, #000 0 ${block}px, transparent ${block}px ${period}px)`;
  } else {
    bar.style.background = `linear-gradient(90deg, ${on} 0 ${filled * 100}%, ${off} ${filled * 100}% 100%)`;
    bar.style.webkitMaskImage = bar.style.maskImage = "none";
  }
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
    if (m.kind === "bar") {
      const source = el("select", { title: "What fills the bar" },
        Object.entries(READOUT_KINDS).filter(([k]) => readoutNumberKinds.has(k)).map(([k, v]) => el("option", { value: k }, v.name)));
      source.value = m.source || "cpu_load";
      source.addEventListener("change", () => { m.source = source.value; update(); });
      extra.push(source);
    }
    if (m.kind === "image") extra.push(el("span", { class: "muted small grow" }, (m.file || "").split(/[\\/]/).pop()));
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
    const boxed = BOXED.has(m.kind);
    const main = el("div", { class: "readout-row-main" }, [
      el("span", { class: "kind" }, (READOUT_KINDS[m.kind] || { name: m.kind }).name),
      ...extra,
      boxed ? null : label,
      boxed ? null : font,
      m.kind === "image" ? null : color,
      advanced,
      remove,
    ].filter(Boolean));
    const parts = [main];
    if (open) {
      const shadow = el("label", { class: "check" }, [el("input", { type: "checkbox" }), "Shadow"]);
      shadow.querySelector("input").checked = m.shadow;
      shadow.querySelector("input").addEventListener("change", (e) => { m.shadow = e.target.checked; update(); });
      const place = [
        widgetSlider("Left ↔ right", 0, 1000, m.x, "", (v) => { m.x = v; update(); }),
        widgetSlider("Up ↕ down", 0, 1000, m.y, "", (v) => { m.y = v; update(); }),
      ];
      const look = boxed
        ? [
            widgetSlider("Width", 1, 1000, m.w, "", (v) => { m.w = v; update(); }),
            widgetSlider("Height", 1, 1000, m.h, "", (v) => { m.h = v; update(); }),
          ]
        : [
            widgetSlider("Size", 20, 600, m.size, "", (v) => { m.size = v; update(); }),
            widgetSlider("Caption size", 15, 100, m.caption_scale, "%", (v) => { m.caption_scale = v; update(); }),
          ];
      const anchor = el("select", { title: "Which point of the widget its position places" }, [
        el("option", { value: "center" }, "Placed by its middle"),
        el("option", { value: "top_left" }, "Placed by its top left"),
        el("option", { value: "top_center" }, "Placed by its top middle"),
        el("option", { value: "top_right" }, "Placed by its top right"),
      ]);
      anchor.value = m.anchor || "center";
      anchor.addEventListener("change", () => { m.anchor = anchor.value; update(); });
      const extras = [anchor];
      if (!boxed) {
        const unit = el("label", { class: "check" }, [el("input", { type: "checkbox" }), "Show unit"]);
        unit.querySelector("input").checked = m.unit;
        unit.querySelector("input").addEventListener("change", (e) => { m.unit = e.target.checked; update(); });
        if (readoutNumberKinds.has(m.kind)) extras.push(unit);
        extras.push(shadow);
      }
      parts.push(el("div", { class: "readout-adv" }, [
        ...place,
        ...look,
        widgetSlider("Opacity", 10, 100, m.opacity, "%", (v) => { m.opacity = v; update(); }),
        ...extras,
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
    const start = { x: e.clientX, y: e.clientY, size: m.size, mx: m.x, my: m.y, w: m.w, h: m.h };
    const vLine = layer.querySelector(".snap-line.v");
    const hLine = layer.querySelector(".snap-line.h");
    readouts.active = i;
    readouts.dragging = true;
    document.querySelectorAll(".readout-row").forEach((r, k) => r.classList.toggle("active", k === i));
    document.querySelectorAll("#oled-readouts .readout").forEach((r, k) => r.classList.toggle("active", k === i));
    node.setPointerCapture(e.pointerId);
    const move = (ev) => {
      if (resizing && BOXED.has(m.kind)) {
        const flip = m.anchor === "top_right" ? -1 : 1;
        const shift = m.anchor === "center" || m.anchor === "top_center" ? 2 : 1;
        m.w = Math.round(Math.min(1000, Math.max(1, start.w + (flip * shift * (ev.clientX - start.x) / box.width) * 1000)));
        m.h = Math.round(Math.min(1000, Math.max(1, start.h + ((m.anchor === "center" ? 2 : 1) * (ev.clientY - start.y) / box.height) * 1000)));
      } else if (resizing) {
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
    ...(kind === "bar" ? { w: 300, h: 30, source: "cpu_load", block: 0, gap: 0 } : {}),
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
    readouts.values = await invoke("oled_readings", { cpu: readouts.modules.some(usesCpuTemp) });
  } catch {
    return;
  }
  if (readouts.modules.some(usesCpuTemp)) {
    const wasSilent = cpuTempSilent();
    if (readouts.values.cpu_temp_c != null) readouts.cpuSilentSince = null;
    else readouts.cpuSilentSince ??= Date.now();
    if (cpuTempSilent() !== wasSilent) drawCpuSetup();
  }
  document.querySelectorAll("#oled-readouts .readout").forEach((node) => {
    const m = readouts.modules[Number(node.dataset.index)];
    if (!m) return;
    const value = node.querySelector(".readout-value");
    if (value) value.textContent = readoutValue(m);
    if (m.kind === "bar") drawBar(node.querySelector(".widget-bar"), m, node);
  });
}, 2000);

// Sizes follow the preview's height, which is 0 until the page is shown.
new ResizeObserver(() => readouts.file && drawWidgetLayer()).observe($("#oled-readouts"));

// Layouts: widget sets saved by name or imported from L-Connect, loaded into the editor.
const savedLayouts = () => (last && last.oled && last.oled.layouts) || {};

function drawLayoutChoices() {
  const select = $("#widget-layouts");
  const names = Object.keys(savedLayouts());
  const current = select.value;
  select.replaceChildren(
    el("option", { value: "" }, names.length ? "Load a layout…" : "No saved layouts"),
    ...names.map((n) => el("option", { value: n }, n)),
  );
  select.value = names.includes(current) ? current : "";
  select.disabled = !names.length || !readouts.file;
  $("#layout-delete").disabled = !select.value;
  $("#layout-save").disabled = !readouts.file || !readouts.modules.length;
}

$("#widget-layouts").addEventListener("change", (e) => {
  const layout = savedLayouts()[e.target.value];
  $("#layout-delete").disabled = !e.target.value;
  if (!layout || !readouts.file) return;
  readouts.modules = structuredClone(layout).map(withDefaults);
  readouts.active = -1;
  readouts.open.clear();
  drawReadouts();
  toast("Layout loaded — Apply to send it to the screen");
});

$("#layout-save").addEventListener("click", () => {
  promptName("Save widgets as a layout", $("#widget-layouts").value || "", async (name) => {
    if (!name) return;
    const modules = structuredClone(readouts.modules);
    await call("save_widget_layout", { name, modules }, `Saved layout “${name}”`);
    if (last && last.oled) last.oled.layouts = { ...savedLayouts(), [name]: modules };
    drawLayoutChoices();
    $("#widget-layouts").value = name;
    $("#layout-delete").disabled = false;
  });
});

$("#layout-delete").addEventListener("click", async () => {
  const name = $("#widget-layouts").value;
  if (!name) return;
  await call("delete_widget_layout", { name }, `Deleted layout “${name}”`);
  if (last && last.oled) {
    const all = { ...savedLayouts() };
    delete all[name];
    last.oled.layouts = all;
  }
  drawLayoutChoices();
});

$("#layout-import").addEventListener("click", async () => {
  const button = $("#layout-import");
  button.disabled = true;
  button.textContent = "Importing…";
  let report = null;
  try {
    report = await call("import_lconnect_themes");
  } catch {
    // shown by call()
  } finally {
    button.disabled = false;
    button.textContent = "Import L-Connect themes…";
  }
  if (!report) return;
  readouts.fonts = null;
  if (typeof refresh === "function") await refresh();
  if (typeof loadOledLibrary === "function") loadOledLibrary();
  drawLayoutChoices();
  const lines = [
    el("p", {}, `${report.imported.length} theme${report.imported.length === 1 ? "" : "s"} imported as layouts. Pick one from “Load a layout…”, then Apply.`),
    report.videos ? el("p", { class: "muted small" }, `Found the video for ${report.videos} of them: those play with their widgets already, and their folder is in the library.`) : null,
    report.backgrounds ? el("p", { class: "muted small" }, `${report.backgrounds} theme background${report.backgrounds === 1 ? "" : "s"} added to the library as still pictures (a single frame of the theme's video, which is all a theme file holds).`) : null,
    report.videos < report.imported.length ? el("p", { class: "muted small" }, "Themes don't contain their videos (only L-Connect's folder does), so the rest are layouts to put over any video. Picking a folder that also holds L-Connect's theme videos links them up.") : null,
    report.skipped.length ? el("p", { class: "muted small" }, `Not shown yet (RGBable doesn't read these): ${report.skipped.join(", ")}.`) : null,
    ...report.failed.map((f) => el("p", { class: "muted small" }, `Skipped ${f}`)),
  ].filter(Boolean);
  openDialog("L-Connect themes", lines, [el("button", { class: "btn primary", onclick: closeDialog }, "Done")]);
});
