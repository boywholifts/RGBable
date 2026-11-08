// Cooling page: live readings and the speed setting of each wireless fan device.

const FAN_FLOOR = 20;
const CURVE_T = [20, 60];
const DEFAULT_CURVE = [[28, 30], [32, 45], [36, 70], [40, 100]];
const fanCards = new Map();
let fanIds = "";

function renderMonitor(o) {
  $("#m-pump").textContent = o.telemetry.pump_rpm ?? "–";
  $("#m-coolant").textContent = o.telemetry.coolant_c ?? "–";
  const fans = o.devices.filter((d) => d.kind === "fans");
  const ids = fans.map((d) => d.id).join(",");
  if (ids !== fanIds) {
    fanIds = ids;
    fanCards.clear();
    const grid = $("#m-fans");
    grid.replaceChildren(...fans.map((d) => fanCard(d)));
    if (!fans.length) grid.append(el("div", { class: "empty" }, "No wireless fans connected."));
  }
  for (const d of fans) fanCards.get(d.id)?.update(d, o.telemetry.coolant_c);
}

function sendFanMode(id, mode) {
  call("set_fan_mode", { id, mode });
}

function fanCard(d) {
  let mode = structuredClone(d.fan_mode);
  const live = el("div", { class: "fanrow" });
  const name = el("div", { class: "card-title" }, d.name);
  const seg = el("div", { class: "seg tabs" });
  const body = el("div", { class: "fan-body" });
  const choices = [
    ["motherboard", "Motherboard"],
    ["fixed", "Fixed"],
    ["curve", "Coolant curve"],
  ];
  for (const [value, label] of choices) {
    seg.append(el("button", { "data-mode": value, onclick: () => choose(value) }, label));
  }
  const all = el("button", { class: "btn mini ghost", title: "Give every wireless fan this setting", onclick: copyToAll }, "Use for all fans");

  let coolant = null;
  let curve = null;

  function choose(value) {
    if (value === mode.mode) return;
    if (value === "fixed") mode = { mode: "fixed", percent: 60 };
    else if (value === "curve") mode = { mode: "curve", points: structuredClone(DEFAULT_CURVE) };
    else mode = { mode: "motherboard" };
    draw();
    sendFanMode(d.id, mode);
  }

  async function copyToAll() {
    for (const id of fanIds.split(",")) {
      await invoke("set_fan_mode", { id, mode }).catch((e) => toast(String(e), true));
      fanCards.get(id)?.set(structuredClone(mode));
    }
    toast("All fans updated");
  }

  function draw() {
    seg.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b.dataset.mode === mode.mode));
    curve = null;
    if (mode.mode === "motherboard") {
      body.replaceChildren(el("p", { class: "muted small" }, "Follows the fan header the wireless dongle is plugged into, like L-Connect. Never drops below 20%."));
    } else if (mode.mode === "fixed") {
      const value = el("span", { class: "fan-value" }, `${mode.percent}%`);
      const range = el("input", { type: "range", min: String(FAN_FLOOR), max: "100", step: "5", value: String(Math.max(FAN_FLOOR, mode.percent)) });
      range.addEventListener("input", () => (value.textContent = `${range.value}%`));
      range.addEventListener("change", () => {
        mode = { mode: "fixed", percent: Number(range.value) };
        sendFanMode(d.id, mode);
      });
      body.replaceChildren(el("div", { class: "fan-fixed" }, [range, value]));
    } else {
      curve = curveEditor(mode.points, (points) => {
        mode = { mode: "curve", points };
        sendFanMode(d.id, mode);
      });
      curve.marker(coolant);
      body.replaceChildren(
        curve.node,
        el("p", { class: "muted small" }, "Drag the points. Without a coolant reading for 5 seconds the fans run at full speed."),
      );
    }
  }

  const card = el("div", { class: "card fan-card" }, [
    el("div", { class: "fan-head" }, [name, all]),
    live,
    seg,
    body,
  ]);
  fanCards.set(d.id, {
    update(device, celsius) {
      name.textContent = device.name;
      coolant = celsius ?? null;
      live.replaceChildren(
        ...device.rpm.map((rpm, i) => el("span", {}, `${rpm} rpm · ${device.speed[i] ?? "–"}%`)),
      );
      curve?.marker(coolant);
    },
    set(next) {
      mode = next;
      draw();
    },
  });
  draw();
  return card;
}

/// An SVG graph of fan % against coolant °C with draggable points.
function curveEditor(initial, onChange) {
  const W = 300, H = 150, PAD = 26;
  const ns = "http://www.w3.org/2000/svg";
  const svgEl = (tag, attrs) => {
    const n = document.createElementNS(ns, tag);
    for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
    return n;
  };
  let points = initial.map((p) => [...p]).sort((a, b) => a[0] - b[0]);
  const x = (t) => PAD + ((t - CURVE_T[0]) / (CURVE_T[1] - CURVE_T[0])) * (W - PAD - 8);
  const y = (p) => H - PAD - (p / 100) * (H - PAD - 8);
  const fromX = (px) => Math.round(CURVE_T[0] + ((px - PAD) / (W - PAD - 8)) * (CURVE_T[1] - CURVE_T[0]));
  const fromY = (py) => Math.round(((H - PAD - py) / (H - PAD - 8)) * 100);

  const svg = svgEl("svg", { viewBox: `0 0 ${W} ${H}`, class: "fan-curve" });
  for (let t = CURVE_T[0]; t <= CURVE_T[1]; t += 10) {
    svg.append(svgEl("line", { x1: x(t), x2: x(t), y1: y(0), y2: y(100), class: "grid-line" }));
    const label = svgEl("text", { x: x(t), y: H - 8, class: "axis", "text-anchor": "middle" });
    label.textContent = `${t}°`;
    svg.append(label);
  }
  for (const p of [0, 50, 100]) {
    svg.append(svgEl("line", { x1: x(CURVE_T[0]), x2: x(CURVE_T[1]), y1: y(p), y2: y(p), class: "grid-line" }));
    const label = svgEl("text", { x: PAD - 4, y: y(p) + 4, class: "axis", "text-anchor": "end" });
    label.textContent = `${p}`;
    svg.append(label);
  }
  svg.append(svgEl("rect", { x: x(CURVE_T[0]), y: y(FAN_FLOOR), width: x(CURVE_T[1]) - x(CURVE_T[0]), height: y(0) - y(FAN_FLOOR), class: "floor" }));
  const now = svgEl("line", { y1: y(0), y2: y(100), class: "now" });
  const path = svgEl("polyline", { class: "curve" });
  svg.append(now, path);
  const handles = points.map(() => svgEl("circle", { r: 6, class: "handle" }));
  handles.forEach((h) => svg.append(h));

  const clampT = (t) => Math.min(CURVE_T[1], Math.max(CURVE_T[0], t));
  function layout() {
    const line = [[CURVE_T[0], points[0][1]], ...points, [CURVE_T[1], points[points.length - 1][1]]];
    path.setAttribute("points", line.map(([t, p]) => `${x(clampT(t))},${y(Math.max(FAN_FLOOR, p))}`).join(" "));
    points.forEach(([t, p], i) => {
      handles[i].setAttribute("cx", x(clampT(t)));
      handles[i].setAttribute("cy", y(p));
    });
  }

  handles.forEach((h, i) => {
    h.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      const box = svg.getBoundingClientRect();
      const move = (ev) => {
        const sx = ((ev.clientX - box.left) / box.width) * W;
        const sy = ((ev.clientY - box.top) / box.height) * H;
        const lo = i > 0 ? points[i - 1][0] + 1 : CURVE_T[0];
        const hi = i < points.length - 1 ? points[i + 1][0] - 1 : CURVE_T[1];
        points[i] = [Math.min(hi, Math.max(lo, fromX(sx))), Math.min(100, Math.max(FAN_FLOOR, fromY(sy)))];
        title.textContent = `${points[i][0]}°C → ${points[i][1]}%`;
        layout();
      };
      const up = () => {
        h.removeEventListener("pointermove", move);
        h.removeEventListener("pointerup", up);
        h.removeEventListener("pointercancel", up);
        title.textContent = "";
        onChange(points.map((p) => [...p]));
      };
      h.addEventListener("pointermove", move);
      h.addEventListener("pointerup", up);
      h.addEventListener("pointercancel", up);
    });
  });

  const title = el("div", { class: "fan-readout muted small" });
  layout();
  return {
    node: el("div", { class: "fan-curve-wrap" }, [svg, title]),
    marker(celsius) {
      const show = celsius != null;
      now.style.display = show ? "" : "none";
      if (show) {
        now.setAttribute("x1", x(clampT(celsius)));
        now.setAttribute("x2", x(clampT(celsius)));
      }
    },
  };
}
