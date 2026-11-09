// Fan screens page: the round LCDs of Lian Li LCD fans. Each plays its own video, GIF or
// picture from the library, turned to match how the fan is mounted.

const fanScreens = { list: [], library: null, timer: null, pending: new Map() };

function fanScreenThumb(file) {
  if (!file) return null;
  const item = fanScreens.library && fanScreens.library.items.find((i) => i.path === file);
  if (item && item.thumb) return assetUrl(item.thumb);
  return /\.(png|jpe?g|gif|webp|bmp)$/i.test(file) ? assetUrl(file) : null;
}

function fanScreenName(file) {
  return file ? file.split(/[\\/]/).pop().replace(/\.[^.]+$/, "") : "Nothing playing";
}

/// Saves a change after the slider or buttons settle, so dragging doesn't re-encode each step.
function changeFanScreen(id, change) {
  const key = id || "*";
  const merged = { ...(fanScreens.pending.get(key) || {}), ...change };
  clearTimeout(merged.timer);
  merged.timer = setTimeout(async () => {
    fanScreens.pending.delete(key);
    const { timer, ...args } = merged;
    await call("set_fan_screen", { id, file: null, fit: null, rotation: null, ...args });
    loadFanScreens();
  }, change.rotation === undefined ? 0 : 600);
  fanScreens.pending.set(key, merged);
}

function fanScreenCard(s) {
  const src = fanScreenThumb(s.file);
  const face = el("div", { class: "fan-face", style: `transform: rotate(${s.rotation}deg)` }, [
    src ? el("img", { src, alt: "", class: s.fit === "contain" ? "contain" : "" }) : el("div", { class: "fan-face-empty" }, "Off"),
  ]);
  const angle = el("input", { type: "range", min: "0", max: "359", step: "1", value: String(s.rotation) });
  const degrees = el("input", { type: "number", class: "text deg", min: "0", max: "359", value: String(s.rotation) });
  const turnTo = (value) => {
    const v = ((Math.round(Number(value) || 0) % 360) + 360) % 360;
    angle.value = degrees.value = String(v);
    face.style.transform = `rotate(${v}deg)`;
    changeFanScreen(s.id, { rotation: v });
  };
  angle.addEventListener("input", () => turnTo(angle.value));
  degrees.addEventListener("change", () => turnTo(degrees.value));
  const quarter = el("button", { class: "btn mini", title: "Turn a quarter clockwise" }, "+90°");
  quarter.addEventListener("click", () => turnTo(Number(angle.value) + 90));

  const fit = el("div", { class: "seg" }, ["cover", "contain"].map((f) => {
    const b = el("button", { class: s.fit === f ? "on" : "" }, f === "cover" ? "Fill" : "Fit");
    b.addEventListener("click", () => changeFanScreen(s.id, { fit: f }));
    return b;
  }));
  const choose = el("button", { class: "btn primary" }, s.file ? "Change…" : "Choose…");
  choose.addEventListener("click", () => pickFanMedia(s.name, (file) => changeFanScreen(s.id, { file })));
  const off = el("button", { class: "btn" }, "Turn off");
  off.disabled = !s.file;
  const powerOn = el("button", { class: "btn mini", title: "Show the first frame of this from power-on, until RGBable starts" }, "Use as power-on picture");
  powerOn.disabled = !s.file;
  powerOn.addEventListener("click", async () => {
    powerOn.disabled = true;
    powerOn.textContent = "Saving…";
    await call("save_fan_screen_power_on", { id: s.id }, `${s.name}: power-on picture saved`);
    loadFanScreens();
  });
  off.addEventListener("click", () => changeFanScreen(s.id, { file: "" }));
  const name = el("button", { class: "fan-screen-name", title: "Rename" }, s.name);
  name.addEventListener("click", () =>
    promptName("Rename fan screen", s.name, async (n) => {
      await call("rename_fan_screen", { id: s.id, name: n });
      loadFanScreens();
    }));

  return el("div", { class: "card fan-screen" }, [
    el("div", { class: "fan-ring" }, face),
    el("div", { class: "fan-screen-info" }, [
      el("div", { class: "row" }, [name, el("span", { class: "grow" }), el("span", { class: "pill" }, s.state)]),
      el("div", { class: "muted small" }, `${s.model} · ${fanScreenName(s.file)}`),
      el("div", { class: "row" }, [choose, off, fit]),
      el("div", { class: "row fan-turn" }, [el("span", { class: "muted small" }, "Turn"), angle, degrees, el("span", { class: "muted small" }, "°"), quarter]),
      el("div", { class: "row" }, [powerOn]),
    ]),
  ]);
}

function renderFanScreens() {
  const grid = $("#fan-screen-grid");
  if (!fanScreens.list.length) {
    grid.replaceChildren(el("div", { class: "empty" }, "No fan screens connected. Quit L-Connect, check the fans' USB cable, and they show up here."));
    return;
  }
  // a turn being dragged or typed keeps its place until it's saved
  if (grid.contains(document.activeElement) && document.activeElement.tagName === "INPUT") return;
  if (fanScreens.pending.size) return;
  grid.replaceChildren(...fanScreens.list.map(fanScreenCard));
}

async function loadFanScreens() {
  try {
    fanScreens.list = await invoke("fan_screens");
  } catch (e) {
    return;
  }
  renderFanScreens();
}

async function loadFanLibrary() {
  try {
    fanScreens.library = await invoke("oled_library");
  } catch (e) {
    fanScreens.library = { items: [], folders: [] };
  }
}

/// Picks a video or picture from the library for a screen.
async function pickFanMedia(target, onPick) {
  await loadFanLibrary();
  const items = fanScreens.library.items;
  const search = el("input", { class: "text", placeholder: "Search" });
  const grid = el("div", { class: "lib-grid fan-pick-grid" });
  const draw = () => {
    const q = search.value.trim().toLowerCase();
    const shown = items.filter((i) => !q || i.name.toLowerCase().includes(q));
    grid.replaceChildren(...(shown.length ? shown.map((item) => {
      const tile = el("div", { class: "oled-tile", title: item.path }, [
        el("div", { class: "oled-thumb fan-thumb" }, item.thumb ? el("img", { src: assetUrl(item.thumb), alt: "", loading: "lazy" }) : el("div", { class: "thumb-wait" }, "…")),
        el("div", { class: "oled-tile-name" }, item.name),
        el("div", { class: "muted small" }, item.folder),
      ]);
      tile.addEventListener("click", () => {
        closeDialog();
        onPick(item.path);
      });
      return tile;
    }) : [el("div", { class: "empty" }, items.length ? "Nothing matches your search." : "The library is empty. Import L-Connect's fan assets or add a folder on the HydroShift screen page.")]));
  };
  search.addEventListener("input", draw);
  draw();
  openDialog(`Play on ${target}`, [search, grid], [el("button", { class: "btn", onclick: closeDialog }, "Cancel")]);
  document.querySelector(".dialog").classList.add("wide");
}

$("#fan-screen-all").addEventListener("click", () =>
  pickFanMedia("every fan screen", (file) => changeFanScreen(null, { file })));

$("#fan-screen-import").addEventListener("click", async () => {
  const r = await call("import_lconnect_fan_assets");
  if (!r) return;
  toast(`Imported ${r.videos} video${r.videos === 1 ? "" : "s"} and ${r.images} picture${r.images === 1 ? "" : "s"} into the library`);
  await loadFanLibrary();
  renderFanScreens();
});

/// The tab shows only while a fan screen is connected.
function renderFanScreensNav(o) {
  const nav = document.querySelector('.nav[data-page="fanscreens"]');
  const has = (o.fan_screens || 0) > 0;
  if (nav.hidden === !has) return;
  nav.hidden = !has;
  if (!has && nav.classList.contains("active")) document.querySelector('.nav[data-page="lighting"]').click();
}

document.addEventListener("page-changed", async (e) => {
  clearInterval(fanScreens.timer);
  if (e.detail !== "fanscreens") return;
  await loadFanLibrary();
  loadFanScreens();
  fanScreens.timer = setInterval(loadFanScreens, 3000);
});
