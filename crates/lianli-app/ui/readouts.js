// Readouts over the HydroShift screen's video: placed on the preview, saved per video.

const READOUT_KINDS = {
  clock: { name: "Clock", label: "" },
  date: { name: "Date", label: "" },
  coolant: { name: "Coolant temperature", label: "COOLANT" },
  pump: { name: "Pump speed", label: "PUMP" },
  gpu_temp: { name: "GPU temperature", label: "GPU" },
  gpu_load: { name: "GPU load", label: "GPU LOAD" },
  fans: { name: "Fan speed", label: "FANS" },
  text: { name: "Text", label: "" },
};

const readouts = { file: null, modules: [], values: {}, active: -1, saveTimer: null };

function readoutValue(m) {
  const v = readouts.values;
  const dash = (x, f) => (x == null ? "–" : f(x));
  const now = new Date();
  switch (m.kind) {
    case "clock": {
      const h = now.getHours(), min = String(now.getMinutes()).padStart(2, "0");
      return m.hour12 ? `${h % 12 || 12}:${min}` : `${String(h).padStart(2, "0")}:${min}`;
    }
    case "date": return now.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" }).replace(",", "");
    case "coolant": return dash(v.coolant_c, (c) => `${c}°C`);
    case "pump": return dash(v.pump_rpm, (r) => `${r} RPM`);
    case "gpu_temp": return dash(v.gpu_temp_c, (c) => `${c}°C`);
    case "gpu_load": return dash(v.gpu_load, (p) => `${p}%`);
    case "fans": return dash(v.fans_rpm, (r) => `${r} RPM`);
    default: return m.text || "";
  }
}

/// Called whenever the screen page shows a different video (or none).
function syncReadouts(path) {
  if (path === readouts.file) return drawReadouts();
  readouts.file = path || null;
  readouts.active = -1;
  const saved = (last && last.oled && last.oled.modules) || {};
  readouts.modules = structuredClone(saved[path] || []);
  drawReadouts();
}

function saveReadouts() {
  clearTimeout(readouts.saveTimer);
  const file = readouts.file;
  const modules = structuredClone(readouts.modules);
  readouts.saveTimer = setTimeout(async () => {
    if (!file) return;
    await call("set_oled_modules", { file, modules });
    if (last && last.oled) {
      last.oled.modules = { ...(last.oled.modules || {}), [file]: modules };
      if (!modules.length) delete last.oled.modules[file];
    }
  }, 900);
}

function hexOf(rgb) {
  return "#" + rgb.map((c) => c.toString(16).padStart(2, "0")).join("");
}

function drawReadouts() {
  $("#readout-add").disabled = !readouts.file;
  drawLayer();
  drawList();
}

function drawLayer() {
  const layer = $("#oled-readouts");
  if (!readouts.file) {
    layer.replaceChildren();
    return;
  }
  const height = layer.clientHeight || 1;
  layer.replaceChildren(...readouts.modules.map((m, i) => {
    const size = (m.size / 1000) * height;
    const node = el("div", { class: "readout" + (i === readouts.active ? " active" : ""), title: "Drag to move" }, [
      m.label ? el("span", { class: "readout-caption" }, m.label) : null,
      el("span", { class: "readout-value" }, readoutValue(m)),
      el("span", { class: "readout-size", title: "Drag to resize" }),
    ].filter(Boolean));
    node.style.left = `${m.x / 10}%`;
    node.style.top = `${m.y / 10}%`;
    node.style.color = hexOf(m.color);
    node.style.fontSize = `${size}px`;
    const caption = node.querySelector(".readout-caption");
    if (caption) caption.style.fontSize = `${size * 0.38}px`;
    readoutDrag(node, m, i);
    return node;
  }));
}

function drawList() {
  const list = $("#readout-list");
  if (!readouts.file) {
    list.replaceChildren(el("div", { class: "empty small" }, "Pick a video first."));
    return;
  }
  if (!readouts.modules.length) {
    list.replaceChildren(el("div", { class: "empty small" }, "No readouts on this video."));
    return;
  }
  list.replaceChildren(...readouts.modules.map((m, i) => {
    const label = el("input", { type: "text", class: "text", placeholder: "Caption (optional)", value: m.label || "" });
    label.addEventListener("input", () => { m.label = label.value; drawLayer(); saveReadouts(); });
    const color = el("input", { type: "color", value: hexOf(m.color), title: "Color" });
    color.addEventListener("input", () => {
      const hex = color.value.slice(1);
      m.color = [0, 2, 4].map((k) => parseInt(hex.slice(k, k + 2), 16));
      drawLayer();
      saveReadouts();
    });
    const extra = [];
    if (m.kind === "text") {
      const text = el("input", { type: "text", class: "text", placeholder: "Text to show", value: m.text || "" });
      text.addEventListener("input", () => { m.text = text.value; drawLayer(); saveReadouts(); });
      extra.push(text);
    }
    if (m.kind === "clock") {
      const h12 = el("label", { class: "check" }, [el("input", { type: "checkbox" }), "12-hour"]);
      h12.querySelector("input").checked = !!m.hour12;
      h12.querySelector("input").addEventListener("change", (e) => { m.hour12 = e.target.checked; drawLayer(); saveReadouts(); });
      extra.push(h12);
    }
    const remove = el("button", { class: "icon-btn", title: "Remove" }, "✕");
    remove.addEventListener("click", () => {
      readouts.modules.splice(i, 1);
      readouts.active = -1;
      drawReadouts();
      saveReadouts();
    });
    const row = el("div", { class: "readout-row" + (i === readouts.active ? " active" : "") }, [
      el("span", { class: "kind" }, READOUT_KINDS[m.kind].name),
      ...extra,
      label,
      color,
      remove,
    ]);
    row.addEventListener("pointerdown", () => {
      if (readouts.active === i) return;
      readouts.active = i;
      document.querySelectorAll(".readout-row").forEach((r, k) => r.classList.toggle("active", k === i));
      drawLayer();
    });
    return row;
  }));
}

function readoutDrag(node, m, i) {
  node.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    const layer = $("#oled-readouts");
    const box = layer.getBoundingClientRect();
    const resizing = e.target.classList.contains("readout-size");
    const start = { x: e.clientX, y: e.clientY, size: m.size };
    readouts.active = i;
    document.querySelectorAll(".readout-row").forEach((r, k) => r.classList.toggle("active", k === i));
    node.classList.add("active");
    node.setPointerCapture(e.pointerId);
    const move = (ev) => {
      if (resizing) {
        const grow = ((ev.clientX - start.x) + (ev.clientY - start.y)) / box.height * 1000;
        m.size = Math.round(Math.min(600, Math.max(20, start.size + grow)));
        node.style.fontSize = `${(m.size / 1000) * box.height}px`;
        const caption = node.querySelector(".readout-caption");
        if (caption) caption.style.fontSize = `${(m.size / 1000) * box.height * 0.38}px`;
      } else {
        m.x = Math.round(Math.min(1000, Math.max(0, ((ev.clientX - box.left) / box.width) * 1000)));
        m.y = Math.round(Math.min(1000, Math.max(0, ((ev.clientY - box.top) / box.height) * 1000)));
        node.style.left = `${m.x / 10}%`;
        node.style.top = `${m.y / 10}%`;
      }
    };
    const up = () => {
      node.removeEventListener("pointermove", move);
      node.removeEventListener("pointerup", up);
      node.removeEventListener("pointercancel", up);
      saveReadouts();
    };
    node.addEventListener("pointermove", move);
    node.addEventListener("pointerup", up);
    node.addEventListener("pointercancel", up);
  });
}

$("#readout-add").addEventListener("change", (e) => {
  const kind = e.target.value;
  e.target.value = "";
  if (!kind || !readouts.file) return;
  const n = readouts.modules.length;
  readouts.modules.push({
    kind,
    x: 500,
    y: Math.min(900, 200 + n * 150),
    size: kind === "clock" ? 260 : 160,
    color: [255, 255, 255],
    label: READOUT_KINDS[kind].label,
    text: kind === "text" ? "RGBable" : "",
    hour12: false,
  });
  readouts.active = n;
  drawReadouts();
  saveReadouts();
});

// Live values for the preview, only while the screen page is open and has readouts.
setInterval(async () => {
  if (!$("#page-oled").classList.contains("active") || !readouts.modules.length) return;
  try {
    readouts.values = await invoke("oled_readings");
  } catch {
    return;
  }
  document.querySelectorAll("#oled-readouts .readout-value").forEach((v, i) => {
    const m = readouts.modules[i];
    if (m) v.textContent = readoutValue(m);
  });
}, 2000);

window.addEventListener("resize", () => readouts.file && drawLayer());
