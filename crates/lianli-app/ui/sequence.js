// Devices page: every device in the order its effect starts, top first, with its timing,
// direction, whether moving effects run across its fans, and the same ⋯ menu as the
// Lighting page. Timing is one of:
//   together  every device starts at the same moment
//   delays    each device runs its own loop, started a set time after the others
//   flow      the devices form one long strip, so a meteor or wave travels from one into the next
//   turns     one device at a time: each plays its effect in its own slot and is dark otherwise

const seq = { group: "all", items: [], dirty: false, step: 300, auto: true, mode: "together", key: "" };

const isAnimated = (l) => [l.effect, l.outer_effect].some((e) => e && e.kind !== "static");
const multiZone = (l) => /fans$/.test(l.kind) || l.kind === "Strimer" || (l.zones && l.zones.length > 1);
const partial = (span) => span && span.len < 1000;

const SPEEDS = [[1, "Slowest"], [2, "Slow"], [3, "Normal"], [4, "Fast"], [5, "Fastest"]];

function seqLights(o) {
  const lights = o.lights.filter((l) => seq.group === "all" || l.group === seq.group);
  return lights
    .map((l) => ({
      id: l.id,
      name: l.name,
      sub: `${l.kind === "header" ? "ARGB header" : l.kind} · ${effectSummary(l)}`,
      color: l.color ? rgbToHex(l.color) : "#444",
      animated: isAnimated(l),
      delay: l.delay_ms || 0,
      reversed: [l.effect, l.outer_effect].some((e) => e && e.reverse && e.kind !== "static"),
      speed: [l.effect, l.outer_effect].find((e) => e && e.kind !== "static")?.speed || 3,
      across: !!l.span && l.span.len >= 1000,
      slot: partial(l.span) ? l.span.start : partial(l.turn) ? l.turn.start : null,
      flow: partial(l.span),
      turns: partial(l.turn),
      multi: multiZone(l),
      light: l,
    }))
    .sort((a, b) =>
      b.animated - a.animated ||
      (a.slot ?? 0) - (b.slot ?? 0) ||
      a.delay - b.delay ||
      a.name.localeCompare(b.name)
    );
}

function renderSequence(o, force) {
  const key = JSON.stringify([o.lights.map((l) => [l.id, l.name, l.delay_ms, l.span, l.turn, l.effect, l.outer_effect, l.group]), o.groups, seq.group]);
  if (!force && (seq.dirty || key === seq.key || document.querySelector(".seq-row.dragging, .seq-marker.dragging"))) return;
  seq.key = key;
  seq.items = seqLights(o);
  seq.mode = seq.items.some((i) => i.flow) ? "flow" : seq.items.some((i) => i.turns) ? "turns" : seq.items.some((i) => i.delay) ? "delays" : "together";
  seq.dirty = false;
  const pick = $("#seq-group");
  pick.replaceChildren(
    el("option", { value: "all" }, "All devices"),
    ...o.groups.map((g) => el("option", { value: g.id }, g.name))
  );
  pick.value = o.groups.some((g) => g.id === seq.group) ? seq.group : "all";
  drawSequence();
}

function spaceEvenly() {
  let n = 0;
  for (const item of seq.items) item.delay = item.animated && seq.mode === "delays" ? n++ * seq.step : 0;
}

function changed() {
  seq.dirty = true;
  drawSequence();
}

/// Equal slots in list order for the moving devices: places on the strip for "flow",
/// shares of the loop for "turns".
function slots() {
  const moving = seq.items.filter((i) => i.animated);
  const result = new Map();
  if (seq.mode === "delays" || seq.mode === "together") return result;
  moving.forEach((item, n) => {
    const start = Math.round((n * 1000) / moving.length);
    const end = Math.round(((n + 1) * 1000) / moving.length);
    result.set(item.id, { start, len: end - start });
  });
  return result;
}

const SNAP_MS = 50;

/// Drag a dot along the timeline to set that device's delay.
function draggableMarker(marker, track, item, range, input) {
  marker.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    marker.setPointerCapture(e.pointerId);
    marker.classList.add("dragging");
    const move = (ev) => {
      const box = track.getBoundingClientRect();
      const x = Math.max(0, Math.min(1, (ev.clientX - box.left) / box.width));
      item.delay = Math.round((x * range) / SNAP_MS) * SNAP_MS;
      marker.style.left = `${(item.delay / range) * 100}%`;
      input.value = item.delay;
      seq.dirty = true;
      $("#seq-save").disabled = false;
      $("#seq-discard").hidden = false;
    };
    const up = () => {
      marker.classList.remove("dragging");
      marker.removeEventListener("pointermove", move);
      marker.removeEventListener("pointerup", up);
      changed();
    };
    marker.addEventListener("pointermove", move);
    marker.addEventListener("pointerup", up);
  });
}

function drawSequence() {
  const list = $("#seq-list");
  const range = Math.max(3000, ...seq.items.map((i) => i.delay + 1000));
  const parts = slots();
  $("#page-sequence").dataset.mode = seq.mode;
  document.querySelectorAll("#seq-mode button").forEach((b) => b.classList.toggle("on", b.dataset.mode === seq.mode));
  $("#seq-mode-hint").textContent = {
    together: "Every device starts its effect at the same moment. Pick another timing to stagger, chain or alternate them.",
    delays: "Each device loops on its own, starting this long after the top one. Drag the dots or type a delay; delays longer than an effect's loop wrap around.",
    flow: "The devices act as one long strip in this order: a meteor or wave runs through the first device, then into the next. Effects that light everything at once (breathing, strobe) look the same as before.",
    turns: "One device at a time, in this order: each plays its effect once in its turn and stays dark while the others play.",
  }[seq.mode];
  $("#seq-save").disabled = !seq.dirty;
  $("#seq-discard").hidden = !seq.dirty;
  if (!seq.items.length) {
    list.replaceChildren(el("div", { class: "empty" }, "No devices here yet."));
    return;
  }
  let dragged = null;
  list.replaceChildren(...seq.items.map((item, index) => {
    const input = el("input", { type: "number", class: "text num", min: 0, max: 10000, step: 50, value: item.delay });
    input.disabled = !item.animated || seq.mode !== "delays";
    input.addEventListener("change", () => {
      item.delay = Math.max(0, Math.min(10000, Math.round(Number(input.value) || 0)));
      changed();
    });

    const reverse = el("button", { class: "icon-btn switchable" + (item.reversed ? " on" : ""), title: item.reversed ? "Reversed: click for the normal direction" : "Reverse the direction" }, icon("reverse"));
    reverse.disabled = !item.animated;
    reverse.addEventListener("click", async (e) => {
      e.stopPropagation();
      await call("set_reverse", { id: item.id, reverse: !item.reversed }, item.reversed ? `${item.name}: normal direction` : `${item.name}: reversed`);
      item.reversed = !item.reversed;
      drawSequence();
    });

    const speed = el("select", { class: "seq-speed", title: seq.mode === "flow" || seq.mode === "turns" ? "Effect speed. Devices at different speeds won't line up in a flow or turns." : "Effect speed" },
      SPEEDS.map(([value, label]) => el("option", { value: String(value) }, label)));
    speed.value = String(item.speed);
    speed.disabled = !item.animated;
    speed.addEventListener("click", (e) => e.stopPropagation());
    speed.addEventListener("change", async () => {
      const value = Number(speed.value);
      await call("set_speed", { id: item.id, speed: value }, `${item.name}: ${SPEEDS[value - 1][1].toLowerCase()}`);
      item.speed = value;
    });

    const flowing = seq.mode === "flow";
    const across = el("button", {
      class: "icon-btn switchable" + (item.across || flowing ? " on" : ""),
      title: flowing ? "Flowing through the devices" : item.across ? "Runs across all its fans: click to run on each fan" : "Run moving effects across all its fans instead of on each one",
    }, icon("across"));
    across.disabled = !item.animated || !item.multi || flowing;
    across.addEventListener("click", (e) => {
      e.stopPropagation();
      item.across = !item.across;
      changed();
    });

    const more = el("button", { class: "icon-btn", title: "Rename, LED count, group and more" }, "⋯");
    more.addEventListener("click", (e) => {
      e.stopPropagation();
      deviceMenu(item.light, more);
    });

    const track = el("div", { class: "seq-track" });
    const part = parts.get(item.id);
    if (part) {
      const what = seq.mode === "flow" ? "Its part of the strip" : "Its turn";
      const segment = el("div", { class: "seq-segment", title: `${what}: ${part.start / 10}–${(part.start + part.len) / 10}%` });
      segment.style.left = `${part.start / 10}%`;
      segment.style.width = `${part.len / 10}%`;
      segment.style.background = item.color;
      track.append(segment);
    } else {
      const marker = el("div", { class: "seq-marker" + (item.animated ? " movable" : ""), title: item.animated ? "Drag to set the delay" : "" });
      marker.style.left = `${(item.delay / range) * 100}%`;
      marker.style.background = item.color;
      marker.style.boxShadow = `0 0 10px ${item.color}`;
      if (item.animated) draggableMarker(marker, track, item, range, input);
      track.append(marker);
    }

    const row = el("div", { class: "seq-row" + (item.animated ? "" : " still"), draggable: "true" }, [
      el("span", { class: "seq-handle", title: "Drag to change the order" }, "⋮⋮"),
      el("span", { class: "seq-order" }, String(index + 1)),
      el("div", { class: "row-text" }, [
        el("div", { class: "row-name" }, item.name),
        el("div", { class: "row-sub" }, item.animated ? item.sub : "Plain color, nothing to move"),
      ]),
      seq.mode === "flow" ? el("div", { class: "seq-spacer" }) : track,
      el("label", { class: "seq-delay", title: "Start delay" }, [input, el("span", { class: "muted small" }, "ms")]),
      speed,
      across,
      reverse,
      more,
    ]);
    row.addEventListener("dragstart", (e) => {
      if (e.target.closest(".seq-track")) return e.preventDefault();
      dragged = index;
      row.classList.add("dragging");
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", item.id);
    });
    row.addEventListener("dragend", () => row.classList.remove("dragging"));
    row.addEventListener("dragover", (e) => {
      e.preventDefault();
      row.classList.add("drop");
    });
    row.addEventListener("dragleave", () => row.classList.remove("drop"));
    row.addEventListener("drop", (e) => {
      e.preventDefault();
      if (dragged === null || dragged === index) return drawSequence();
      const [moved] = seq.items.splice(dragged, 1);
      seq.items.splice(index, 0, moved);
      if (seq.auto) spaceEvenly();
      changed();
    });
    return row;
  }));
}

/// Flashes each row when its effect would start (or as the effect reaches it), to check
/// the order before saving.
function previewSequence() {
  const moving = seq.items.filter((i) => i.animated);
  document.querySelectorAll(".seq-row").forEach((row, i) => {
    const item = seq.items[i];
    if (!item.animated) return;
    const at = seq.mode === "delays" ? item.delay : moving.indexOf(item) * 400;
    setTimeout(() => {
      row.classList.remove("flash");
      void row.offsetWidth;
      row.classList.add("flash");
    }, at);
  });
}

$("#seq-group").addEventListener("change", (e) => {
  const next = e.target.value;
  const show = () => {
    seq.group = next;
    if (last) renderSequence(last, true);
  };
  if (!seq.dirty) return show();
  e.target.value = seq.group;
  openDialog("Unsaved changes", el("p", { class: "muted small" }, "Save the changes on this page before showing other devices?"), [
    el("button", { class: "btn", onclick: () => { closeDialog(); show(); } }, "Discard"),
    el("button", { class: "btn primary", onclick: async () => { closeDialog(); await saveSequence(); show(); } }, "Save"),
  ]);
});
$("#seq-mode").addEventListener("click", (e) => {
  const mode = e.target.dataset.mode;
  if (!mode || mode === seq.mode) return;
  seq.mode = mode;
  spaceEvenly();
  changed();
});
$("#seq-step").addEventListener("change", (e) => {
  seq.step = Math.max(0, Math.min(5000, Number(e.target.value) || 0));
  if (seq.auto) {
    spaceEvenly();
    changed();
  }
});
$("#seq-auto").addEventListener("change", (e) => {
  seq.auto = e.target.checked;
  if (seq.auto) {
    spaceEvenly();
    changed();
  }
});
$("#seq-even").addEventListener("click", () => {
  spaceEvenly();
  changed();
});
$("#seq-reverse").addEventListener("click", () => {
  seq.items.reverse();
  if (seq.auto) spaceEvenly();
  changed();
});
$("#seq-reset").addEventListener("click", () => {
  for (const item of seq.items) item.delay = 0;
  changed();
});
$("#seq-preview").addEventListener("click", previewSequence);
$("#seq-discard").addEventListener("click", () => last && renderSequence(last, true));

async function saveSequence() {
  const parts = slots();
  const entries = seq.items.map((i) => {
    const part = parts.get(i.id) || null;
    return {
      id: i.id,
      delay_ms: seq.mode === "delays" ? i.delay : 0,
      span: seq.mode === "flow" ? part : i.across ? { start: 0, len: 1000 } : null,
      turn: seq.mode === "turns" ? part : null,
    };
  });
  await call("set_sequence", { entries }, "Saved");
  seq.dirty = false;
  await refresh();
}
$("#seq-save").addEventListener("click", async () => {
  await saveSequence();
  if (last) renderSequence(last, true);
});
