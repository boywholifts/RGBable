// Devices page: every device in the order its effect starts, top first, with its delay,
// direction, whether moving effects run across its fans, and the same ⋯ menu as the
// Lighting page. "Flow through" chains the devices into one strip, so a meteor or wave
// travels from the first device into the next.

const seq = { group: "all", items: [], dirty: false, step: 300, auto: true, chain: false, key: "" };

const isAnimated = (l) => [l.effect, l.outer_effect].some((e) => e && e.kind !== "static");
const multiZone = (l) => /fans$/.test(l.kind) || l.kind === "Strimer" || (l.zones && l.zones.length > 1);

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
      across: !!l.span && l.span.len >= 1000,
      chainStart: l.span && l.span.len < 1000 ? l.span.start : null,
      multi: multiZone(l),
      light: l,
    }))
    .sort((a, b) =>
      b.animated - a.animated ||
      (a.chainStart ?? 0) - (b.chainStart ?? 0) ||
      a.delay - b.delay ||
      a.name.localeCompare(b.name)
    );
}

function renderSequence(o, force) {
  const key = JSON.stringify([o.lights.map((l) => [l.id, l.name, l.delay_ms, l.span, l.effect, l.outer_effect, l.group]), o.groups, seq.group]);
  if (!force && (seq.dirty || key === seq.key || document.querySelector(".seq-row.dragging"))) return;
  seq.key = key;
  seq.items = seqLights(o);
  seq.chain = seq.items.some((i) => i.chainStart !== null);
  $("#seq-chain").checked = seq.chain;
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
  for (const item of seq.items) item.delay = item.animated && !seq.chain ? n++ * seq.step : 0;
}

function changed() {
  seq.dirty = true;
  drawSequence();
}

/// Spans for the chain: the moving devices share one strip in list order.
function chainSpans() {
  const moving = seq.items.filter((i) => i.animated);
  const spans = new Map();
  moving.forEach((item, n) => {
    const start = Math.round((n * 1000) / moving.length);
    const end = Math.round(((n + 1) * 1000) / moving.length);
    spans.set(item.id, { start, len: end - start });
  });
  return spans;
}

function drawSequence() {
  const list = $("#seq-list");
  const range = Math.max(3000, ...seq.items.map((i) => i.delay + 1000));
  const spans = seq.chain ? chainSpans() : new Map();
  $("#seq-save").disabled = !seq.dirty;
  $("#seq-discard").hidden = !seq.dirty;
  if (!seq.items.length) {
    list.replaceChildren(el("div", { class: "empty" }, "No devices here yet."));
    return;
  }
  let dragged = null;
  list.replaceChildren(...seq.items.map((item, index) => {
    const input = el("input", { type: "number", class: "text num", min: 0, max: 10000, step: 50, value: item.delay });
    input.disabled = !item.animated || seq.chain;
    input.addEventListener("change", () => {
      item.delay = Math.max(0, Math.min(10000, Math.round(Number(input.value) || 0)));
      changed();
    });

    const reverse = el("button", { class: "icon-btn toggle" + (item.reversed ? " on" : ""), title: item.reversed ? "Reversed: click for the normal direction" : "Reverse the direction" }, icon("reverse"));
    reverse.disabled = !item.animated;
    reverse.addEventListener("click", async (e) => {
      e.stopPropagation();
      await call("set_reverse", { id: item.id, reverse: !item.reversed }, item.reversed ? `${item.name}: normal direction` : `${item.name}: reversed`);
      item.reversed = !item.reversed;
      drawSequence();
    });

    const across = el("button", {
      class: "icon-btn toggle" + (item.across || seq.chain ? " on" : ""),
      title: seq.chain ? "Flowing through the devices" : item.across ? "Runs across all its fans: click to run on each fan" : "Run moving effects across all its fans instead of on each one",
    }, icon("across"));
    across.disabled = !item.animated || !item.multi || seq.chain;
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
    const span = spans.get(item.id);
    if (span) {
      const segment = el("div", { class: "seq-segment", title: `Its part of the strip: ${span.start / 10}–${(span.start + span.len) / 10}%` });
      segment.style.left = `${span.start / 10}%`;
      segment.style.width = `${span.len / 10}%`;
      segment.style.background = item.color;
      track.append(segment);
    } else {
      const marker = el("div", { class: "seq-marker", title: `${(item.delay / 1000).toFixed(2)} s` });
      marker.style.left = `${(item.delay / range) * 100}%`;
      marker.style.background = item.color;
      marker.style.boxShadow = `0 0 10px ${item.color}`;
      track.append(marker);
    }

    const row = el("div", { class: "seq-row" + (item.animated ? "" : " still"), draggable: "true" }, [
      el("span", { class: "seq-handle", title: "Drag to change the order" }, "⋮⋮"),
      el("span", { class: "seq-order" }, String(index + 1)),
      el("div", { class: "row-text" }, [
        el("div", { class: "row-name" }, item.name),
        el("div", { class: "row-sub" }, item.animated ? item.sub : "Plain color, nothing to move"),
      ]),
      track,
      el("label", { class: "seq-delay", title: "Start delay" }, [input, el("span", { class: "muted small" }, "ms")]),
      across,
      reverse,
      more,
    ]);
    row.addEventListener("dragstart", (e) => {
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

/// Flashes each row when its effect would start (in a chain, as the effect reaches it),
/// to check the order before saving.
function previewSequence() {
  const moving = seq.items.filter((i) => i.animated);
  document.querySelectorAll(".seq-row").forEach((row, i) => {
    const item = seq.items[i];
    if (!item.animated) return;
    const at = seq.chain ? moving.indexOf(item) * 400 : item.delay;
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
$("#seq-chain").addEventListener("change", (e) => {
  seq.chain = e.target.checked;
  spaceEvenly();
  changed();
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
  const spans = seq.chain ? chainSpans() : new Map();
  const entries = seq.items.map((i) => ({
    id: i.id,
    delay_ms: seq.chain ? 0 : i.delay,
    span: spans.get(i.id) || (i.across ? { start: 0, len: 1000 } : null),
  }));
  await call("set_sequence", { entries }, "Saved");
  seq.dirty = false;
  await refresh();
}
$("#seq-save").addEventListener("click", async () => {
  await saveSequence();
  if (last) renderSequence(last, true);
});
