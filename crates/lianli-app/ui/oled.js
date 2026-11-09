// OLED page: a preview of the screen and a library of videos shared with OLEDable.

const assetUrl = (path) => window.__TAURI__.core.convertFileSrc(path);
const isImageFile = (path) => /\.(png|jpe?g|bmp|webp)$/i.test(path || "");
const fileName = (path) => (path || "").split(/[\\/]/).pop().replace(/\.[^.]+$/, "");

const oled = { current: null, selected: null, fit: "cover", fps: 30, library: null, query: "", shownKey: "" };

function showOnScreen(path) {
  const screen = $("#oled-screen");
  const key = path + "|" + oled.fit;
  if (key === oled.shownKey) return;
  oled.shownKey = key;
  if (!path) {
    screen.replaceChildren(el("div", { class: "oled-empty" }, "Nothing on the screen"));
    return;
  }
  const media = isImageFile(path) || /\.gif$/i.test(path)
    ? el("img", { alt: "" })
    : el("video", { autoplay: "", loop: "", muted: "", playsinline: "" });
  // the file only becomes readable once the app has allowed it; right after a start the
  // library may not have done that yet
  invoke("allow_oled_preview", { file: path })
    .catch(() => {})
    .finally(() => {
      if (oled.shownKey === key) media.src = assetUrl(path);
    });
  if (media.tagName === "VIDEO") media.muted = true;
  media.style.objectFit = oled.fit;
  // L-Connect keeps some videos the way the panel sits (1080 × 2288); the screen plays them upright
  const turnUpright = () => {
    const [w, h] = media.tagName === "VIDEO" ? [media.videoWidth, media.videoHeight] : [media.naturalWidth, media.naturalHeight];
    media.classList.toggle("panel-native", w === 1080 && h === 2288);
  };
  media.addEventListener(media.tagName === "VIDEO" ? "loadedmetadata" : "load", turnUpright);
  media.addEventListener("error", () => {
    // MediaError codes: 2 the file couldn't be read, 3 decoding failed, 4 the format isn't supported
    const reason = { 2: "couldn't read the file", 3: "couldn't decode it", 4: "its format isn't supported here" }[media.error && media.error.code];
    const detail = reason ? ` (${reason})` : "";
    console.warn("OLED preview failed", path, media.error && media.error.code, media.error && media.error.message);
    screen.replaceChildren(el("div", { class: "oled-empty" }, `Can't preview this file here${detail}, but the OLED can still play it`));
  });
  screen.replaceChildren(media);
  syncPreviewPlayback();
}

function renderOledStage() {
  const previewing = oled.selected && oled.selected !== oled.current;
  const path = previewing ? oled.selected : oled.current;
  showOnScreen(path);
  $("#oled-caption").textContent = previewing ? "Preview: not on the HydroShift yet" : oled.current ? "Now on the HydroShift" : "Nothing playing";
  $("#oled-name").textContent = path ? fileName(path) : "Pick something from the library below";
  $("#oled-show").hidden = !previewing;
  $("#oled-frame").classList.toggle("previewing", !!previewing);
  document.querySelectorAll("#oled-fit button").forEach((b) => b.classList.toggle("on", b.dataset.fit === oled.fit));
  document.querySelectorAll(".oled-tile").forEach((t) => {
    t.classList.toggle("selected", t.dataset.path === oled.selected);
    t.classList.toggle("playing", t.dataset.path === oled.current);
  });
  if (typeof syncReadouts === "function") syncReadouts(path);
}

function prepare() {
  const file = oled.selected || oled.current;
  if (file) invoke("prepare_oled", { file, fit: oled.fit, fps: oled.fps }).catch(() => {});
}

async function playOnOled(file) {
  await call("set_oled", { file, fit: oled.fit, fps: oled.fps }, `Sending ${fileName(file)} to the OLED…`);
  oled.current = file;
  oled.selected = null;
  renderOledStage();
  refresh();
}

/// Fill/Fit and the frame rate take effect straight away on what's playing; while another
/// video is being previewed they only change the preview (and its encode).
async function fitOrRateChanged() {
  prepare();
  renderOledStage();
  const previewing = oled.selected && oled.selected !== oled.current;
  if (previewing || !oled.current) return;
  await call("set_oled", { file: null, fit: oled.fit, fps: oled.fps }, `${oled.fit === "cover" ? "Fill" : "Fit"}, ${oled.fps} fps`);
  refresh();
}
$("#oled-fit").addEventListener("click", (e) => {
  const fit = e.target.dataset.fit;
  if (!fit || fit === oled.fit) return;
  oled.fit = fit;
  oled.shownKey = "";
  fitOrRateChanged();
});
$("#oled-fps").addEventListener("change", () => {
  oled.fps = Number($("#oled-fps").value);
  fitOrRateChanged();
});
$("#oled-pick").addEventListener("click", async () => {
  const file = await invoke("pick_media");
  if (file) playOnOled(file);
});
$("#oled-show").addEventListener("click", () => oled.selected && playOnOled(oled.selected));
async function setOledMatch() {
  const count = $("#oled-match").checked ? Number($("#oled-match-count").value) : null;
  await call("set_oled_match", { count }, count ? "Lighting now follows the OLED" : "Lighting no longer follows the OLED");
  refresh();
}
$("#oled-match").addEventListener("change", setOledMatch);
$("#oled-match-count").addEventListener("change", () => $("#oled-match").checked && setOledMatch());

$("#oled-stop").addEventListener("click", async () => {
  await call("stop_oled", {}, "OLED turned off");
  refresh();
});

function renderOled(state, initial) {
  const pill = $("#oled-state");
  pill.textContent = state.state;
  pill.classList.toggle("on", state.state === "Playing");
  if (initial) {
    oled.fit = state.fit;
    oled.fps = state.fps;
    $("#oled-fps").value = String(state.fps);
  }
  const match = last && last.oled_match;
  showMatchStatus((last && last.oled_match_status) || "");
  $("#oled-match").checked = !!match;
  if (match) $("#oled-match-count").value = String(match);
  if (state.file !== oled.current) {
    // changed from elsewhere (a lighting profile, say): show what's playing now, not an
    // older preview
    if (!initial && oled.current !== undefined) {
      oled.selected = null;
      oled.fit = state.fit;
      oled.fps = state.fps;
      $("#oled-fps").value = String(state.fps);
    }
    oled.current = state.file;
    if (oled.selected === oled.current) oled.selected = null;
  }
  renderOledStage();
}

// ---------------------------------------------------------------- library

function formatSize(bytes) {
  if (bytes > 1e9) return (bytes / 1e9).toFixed(1) + " GB";
  if (bytes > 1e6) return (bytes / 1e6).toFixed(0) + " MB";
  return Math.max(1, Math.round(bytes / 1e3)) + " KB";
}

function renderLibraryGrid() {
  const grid = $("#oled-grid");
  const lib = oled.library;
  if (!lib) return;
  const q = oled.query.trim().toLowerCase();
  const items = lib.items.filter((i) => !q || i.name.toLowerCase().includes(q));
  if (!lib.items.length) {
    const linked = lib.folders.some((f) => f.from_oledable && f.exists);
    grid.replaceChildren(el("div", { class: "empty" }, [
      el("div", {}, "No videos yet."),
      el("div", { class: "muted small" }, linked
        ? "Videos you make in OLEDable show up here."
        : "Add a folder of videos with Folders…, or install OLEDable to record wallpapers for the screen."),
    ]));
    return;
  }
  if (!items.length) {
    grid.replaceChildren(el("div", { class: "empty" }, "Nothing matches your search."));
    return;
  }
  grid.replaceChildren(...items.map((item) => {
    const thumb = item.thumb
      ? el("img", { src: assetUrl(item.thumb), alt: "", loading: "lazy" })
      : el("div", { class: "thumb-wait" }, "…");
    const tile = el("div", { class: "oled-tile", "data-path": item.path, title: item.path }, [
      el("div", { class: "oled-thumb" }, [thumb, el("span", { class: "now-badge" }, "On the HydroShift")]),
      el("div", { class: "oled-tile-name" }, item.name),
      el("div", { class: "muted small" }, `${item.folder} · ${formatSize(item.size)}`),
    ]);
    tile.addEventListener("click", () => {
      oled.selected = item.path;
      prepare();
      renderOledStage();
      $("#page-oled").querySelector(".oled-stage").scrollIntoView({ behavior: "smooth", block: "nearest" });
    });
    tile.addEventListener("dblclick", () => playOnOled(item.path));
    return tile;
  }));
  renderOledStage();
}

let libraryRetries = 0;
async function loadOledLibrary() {
  try {
    oled.library = await invoke("oled_library");
  } catch (e) {
    return;
  }
  renderLibraryGrid();
  if (oled.library.making_thumbs && libraryRetries < 20 && $("#page-oled").classList.contains("active")) {
    libraryRetries++;
    setTimeout(loadOledLibrary, 3000);
  }
}

function foldersDialog() {
  const lib = oled.library || { folders: [], linked: true };
  const link = el("input", { type: "checkbox" });
  link.checked = lib.linked;
  link.addEventListener("change", async () => {
    await call("set_oled_link", { linked: link.checked });
    await loadOledLibrary();
    foldersDialog();
  });
  const rows = lib.folders.map((f) =>
    el("div", { class: "folder-row" }, [
      el("div", { class: "row-text" }, [
        el("div", { class: "row-name" }, f.label),
        el("div", { class: "row-sub" }, f.exists ? f.path : f.path + " (not found)"),
      ]),
      ...(f.from_oledable ? [] : [el("button", { class: "btn mini", onclick: async () => {
        await call("remove_oled_folder", { path: f.path });
        await loadOledLibrary();
        foldersDialog();
      } }, "Remove")]),
    ])
  );
  openDialog("Library folders", [
    el("label", { class: "check" }, [link, "Include OLEDable's videos"]),
    ...rows,
  ], [
    el("button", { class: "btn", onclick: async () => {
      if (await call("add_oled_folder")) {
        await loadOledLibrary();
        foldersDialog();
      }
    } }, "Add folder…"),
    el("button", { class: "btn primary", onclick: closeDialog }, "Done"),
  ]);
}

$("#oled-folders-btn").addEventListener("click", foldersDialog);
$("#lib-search").addEventListener("input", (e) => {
  oled.query = e.target.value;
  renderLibraryGrid();
});
document.querySelector('.nav[data-page="oled"]').addEventListener("click", () => {
  libraryRetries = 0;
  loadOledLibrary();
});

// The preview can be a 4K video; decoding it off-screen slows the whole app down.
function syncPreviewPlayback() {
  const video = $("#oled-screen video");
  if (!video) return;
  const visible = $("#page-oled").classList.contains("active") && !document.hidden;
  if (visible) video.play().catch(() => {});
  else video.pause();
}
document.addEventListener("page-changed", syncPreviewPlayback);
document.addEventListener("visibilitychange", syncPreviewPlayback);

$("#oled-match-now").addEventListener("click", async () => {
  const button = $("#oled-match-now");
  button.disabled = true;
  button.textContent = "Matching…";
  try {
    await call("match_oled_now", { count: Number($("#oled-match-count").value) });
  } finally {
    button.disabled = false;
    button.textContent = "Match now";
    refresh();
  }
});

/// The match status with each picked color shown as a chip instead of its hex code.
let shownMatchStatus = null;
function showMatchStatus(text) {
  if (text === shownMatchStatus) return;
  shownMatchStatus = text;
  const parts = text.split(/(#[0-9a-f]{6})/i).filter(Boolean);
  $("#oled-match-status").replaceChildren(...parts.map((part) => {
    if (!/^#[0-9a-f]{6}$/i.test(part)) return part.replace(/\s+$/, " ");
    const chip = el("span", { class: "color-chip", title: part });
    chip.style.background = part;
    chip.style.boxShadow = `0 0 8px ${part}`;
    return chip;
  }));
}
