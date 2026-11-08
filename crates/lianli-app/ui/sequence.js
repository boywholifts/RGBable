// Sequence page: the order devices start their effects in, top first, with each one's delay.

const seq = { group: "all", items: [], dirty: false, step: 300, auto: true, key: "" };

function seqLights(o) {
  const lights = o.lights.filter((l) => seq.group === "all" || l.group === seq.group);
  return lights
    .map((l) => ({
      id: l.id,
      name: l.name,
      sub: `${l.kind === "header" ? "ARGB header" : l.kind} · ${effectSummary(l)}`,
      color: l.color ? rgbToHex(l.color) : "#444",
      effect: l.effect || l.outer_effect,
      animated: [l.effect, l.outer_effect].some((e) => e && e.kind !== "static"),
      delay: l.delay_ms || 0,
    }))
    .sort((a, b) => b.animated - a.animated || a.delay - b.delay || a.name.localeCompare(b.name));
}

function renderSequence(o, force) {
  const key = JSON.stringify([o.lights.map((l) => [l.id, l.name, l.delay_ms, l.effect, l.group]), o.groups, seq.group]);
  if (!force && (seq.dirty || key === seq.key || document.querySelector(".seq-row.dragging"))) return;
  seq.key = key;
  seq.items = seqLights(o);
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
  for (const item of seq.items) item.delay = item.animated ? n++ * seq.step : 0;
}

function changed() {
  seq.dirty = true;
  drawSequence();
}

function drawSequence() {
  const list = $("#seq-list");
  const span = Math.max(3000, ...seq.items.map((i) => i.delay + 1000));
  $("#seq-save").disabled = !seq.dirty;
  $("#seq-discard").hidden = !seq.dirty;
  if (!seq.items.length) {
    list.replaceChildren(el("div", { class: "empty" }, "No devices here yet."));
    return;
  }
  let dragged = null;
  list.replaceChildren(...seq.items.map((item, index) => {
    const input = el("input", { type: "number", class: "text num", min: 0, max: 10000, step: 50, value: item.delay, disabled: !item.animated });
    input.addEventListener("change", () => {
      item.delay = Math.max(0, Math.min(10000, Math.round(Number(input.value) || 0)));
      changed();
    });
    const marker = el("div", { class: "seq-marker" });
    marker.style.left = `${(item.delay / span) * 100}%`;
    marker.style.background = item.color;
    marker.style.boxShadow = `0 0 10px ${item.color}`;
    const row = el("div", { class: "seq-row" + (item.animated ? "" : " still"), draggable: "true", "data-index": index }, [
      el("span", { class: "seq-handle", title: "Drag to change the order" }, "⋮⋮"),
      el("span", { class: "seq-order" }, String(index + 1)),
      el("div", { class: "row-text" }, [
        el("div", { class: "row-name" }, item.name),
        el("div", { class: "row-sub" }, item.animated ? item.sub : "Plain color, nothing to delay"),
      ]),
      el("div", { class: "seq-track", title: `${(item.delay / 1000).toFixed(2)} s` }, marker),
      el("label", { class: "seq-delay" }, [input, el("span", { class: "muted small" }, "ms")]),
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

/// Flashes each row when its effect would start, to check the order before saving.
function previewSequence() {
  document.querySelectorAll(".seq-row").forEach((row, i) => {
    const item = seq.items[i];
    if (!item.animated) return;
    setTimeout(() => {
      row.classList.remove("flash");
      void row.offsetWidth;
      row.classList.add("flash");
    }, item.delay);
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
  openDialog("Unsaved sequence", el("p", { class: "muted small" }, "Save the delays you changed before showing other devices?"), [
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
  await call("set_delays", { delays: seq.items.map((i) => [i.id, i.delay]) }, "Sequence saved");
  seq.dirty = false;
  await refresh();
}
$("#seq-save").addEventListener("click", async () => {
  await saveSequence();
  if (last) renderSequence(last, true);
});
