"use strict";

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/** Web covers load through the app's local cache (cover:// protocol); anything else loads as is. */
const coverSrc = (url) => (/^https?:\/\//i.test(url) ? convertFileSrc(url, "cover") : url);

// ---------- reference data ----------

const STATUSES = [
  { value: "unplayed", label: "Unplayed", color: "var(--s-unplayed)" },
  { value: "unfinished", label: "Unfinished", color: "var(--s-unfinished)" },
  { value: "beaten", label: "Beaten", color: "var(--s-beaten)" },
  { value: "completed", label: "Completed", color: "var(--s-completed)" },
  { value: "endless", label: "Endless", color: "var(--s-endless)" },
  { value: "none", label: "None", color: "var(--s-none)" },
];
const PRIORITIES = [
  ["now_playing", "Now Playing"], ["ongoing", "Ongoing"], ["paused", "Paused"], ["high", "High"],
  ["normal", "Normal"], ["low", "Low"], ["replay", "Replay"], ["shelved", "Shelved"],
];
const OWNERSHIP = [
  ["owned", "Owned"], ["wishlist", "Wishlist"], ["household", "Household"], ["subscription", "Subscription"],
  ["played_it", "Played It"], ["formerly_owned", "Formerly Owned"], ["other", "Other"], ["unknown", "Unknown"],
];
const PRIORITY_RANK = Object.fromEntries(PRIORITIES.map(([v], i) => [v, i]));
const statusOf = (v) => STATUSES.find((s) => s.value === v) ?? STATUSES.at(-1);

const isPlaying = (g) => g.priority === "now_playing" || g.priority === "ongoing";
const isBacklog = (g) =>
  (g.status === "unplayed" || g.status === "unfinished") && g.priority !== "shelved" && g.ownership !== "wishlist";

const SHELVES = {
  all: () => true,
  playing: isPlaying,
  backlog: isBacklog,
  finished: (g) => g.status === "beaten" || g.status === "completed",
  wishlist: (g) => g.ownership === "wishlist",
};

const NO_STORE = "__none";

/// Stores a game came from: its import links plus a Backloggery-style
/// sub-platform such as "PC (Steam)".
function storesOf(g) {
  const stores = new Set(g.stores);
  const sub = /\(([^)]+)\)\s*$/.exec(g.platform);
  if (sub) stores.add(sub[1]);
  return [...stores].sort((a, b) => a.localeCompare(b));
}

const SORTS = {
  title: (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: "base", numeric: true }),
  recent: (a, b) => b.updatedAt.localeCompare(a.updatedAt),
  priority: (a, b) => PRIORITY_RANK[a.priority] - PRIORITY_RANK[b.priority] || SORTS.title(a, b),
  platform: (a, b) => a.platform.localeCompare(b.platform) || SORTS.title(a, b),
  // Games without a store sort last.
  store: (a, b) => (storesOf(a)[0] ?? "\uffff").localeCompare(storesOf(b)[0] ?? "\uffff") || SORTS.title(a, b),
  progress: (a, b) => b.progress - a.progress || SORTS.title(a, b),
  played: (a, b) => (b.lastPlayed ?? "").localeCompare(a.lastPlayed ?? "") || SORTS.title(a, b),
  hours: (a, b) => b.hoursPlayed - a.hoursPlayed || SORTS.title(a, b),
};

const isPhysical = (g) => /^physical/i.test(g.format ?? "");
const FORMATS = {
  "": () => true,
  physical: isPhysical,
  digital: (g) => !isPhysical(g),
};

// ---------- state ----------

const state = {
  games: [],
  filtered: [],
  index: 0,
  shelf: "all",
  search: "",
  platform: "",
  format: "",
  store: "",
  sort: "title",
};

const $ = (sel) => document.querySelector(sel);
const el = (tag, props = {}, ...children) => {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children.filter((c) => c != null));
  return node;
};
const current = () => state.filtered[state.index];

function toast(msg, kind = "info") {
  const t = el("div", { className: `toast ${kind}`, textContent: msg });
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "error" ? 7000 : 4000);
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (err) {
    toast(String(err), "error");
    throw err;
  }
}

// ---------- filtering ----------

function applyFilters({ keepId } = {}) {
  const q = state.search.trim().toLowerCase();
  state.filtered = state.games
    .filter(SHELVES[state.shelf])
    .filter((g) => !state.platform || g.platform === state.platform)
    .filter(FORMATS[state.format])
    .filter((g) => !state.store || (state.store === NO_STORE ? !storesOf(g).length : storesOf(g).includes(state.store)))
    .filter((g) => !q || g.title.toLowerCase().includes(q) || g.notes.toLowerCase().includes(q))
    .sort(SORTS[state.sort]);

  const keep = keepId != null ? state.filtered.findIndex((g) => g.id === keepId) : -1;
  state.index = keep >= 0 ? keep : 0;
  resetCarousel();
  render();
}

function refreshPlatforms() {
  const platforms = [...new Set(state.games.map((g) => g.platform).filter(Boolean))].sort();
  const select = $("#platform-filter");
  const chosen = select.value;
  select.replaceChildren(el("option", { value: "", textContent: "All platforms" }),
    ...platforms.map((p) => el("option", { value: p, textContent: p })));
  select.value = platforms.includes(chosen) ? chosen : "";
  state.platform = select.value;
  $("#platform-list").replaceChildren(...platforms.map((p) => el("option", { value: p })));

  const counts = new Map();
  let none = 0;
  for (const g of state.games) {
    const stores = storesOf(g);
    if (!stores.length) none++;
    for (const s of stores) counts.set(s, (counts.get(s) ?? 0) + 1);
  }
  const storeSelect = $("#store-filter");
  const stores = [...counts.keys()].sort((a, b) => a.localeCompare(b));
  storeSelect.replaceChildren(
    el("option", { value: "", textContent: "All stores" }),
    ...stores.map((s) => el("option", { value: s, textContent: `${s} (${counts.get(s)})` })),
    none ? el("option", { value: NO_STORE, textContent: `No store / manual (${none})` }) : null,
  );
  storeSelect.value = stores.includes(state.store) || (state.store === NO_STORE && none) ? state.store : "";
  state.store = storeSelect.value;
}

function renderShelfCounts() {
  for (const btn of document.querySelectorAll("#shelves button")) {
    const n = state.games.filter(SHELVES[btn.dataset.shelf]).length;
    let badge = btn.querySelector(".count");
    if (!badge) btn.append((badge = el("span", { className: "count" })));
    badge.textContent = n;
  }
}

function renderStats() {
  const total = state.games.length;
  const stats = $("#stats");
  if (!total) return stats.replaceChildren();
  const counts = STATUSES.map((s) => ({ ...s, n: state.games.filter((g) => g.status === s.value).length }));
  const backlog = state.games.filter(isBacklog).length;
  stats.replaceChildren(
    el("span", {}, el("strong", { textContent: backlog }), " in backlog"),
    el("div", { className: "bar" },
      ...counts.filter((c) => c.n).map((c) => {
        const s = el("span", { title: `${c.n} ${c.label}` });
        s.style.flex = c.n;
        s.style.background = c.color;
        return s;
      })),
    el("div", { className: "legend" },
      ...counts.filter((c) => c.n).map((c) => {
        const dot = el("i");
        dot.style.background = c.color;
        return el("span", {}, dot, `${c.label} ${c.n}`);
      })),
  );
}

// ---------- carousel ----------

const WINDOW = 7; // cards rendered on each side of the active one
const cardEls = new Map(); // game id -> element

function hue(str) {
  let h = 0;
  for (const ch of str) h = (h * 31 + ch.codePointAt(0)) % 360;
  return h;
}

function fillCard(card, g) {
  const status = statusOf(g.status);
  card.style.setProperty("--status-color", status.color);
  const art = el("div", { className: "art" });

  const fallback = el("div", { className: "fallback" },
    el("span", { className: "fb-platform", textContent: g.platformAbbr || g.platform }),
    el("span", { className: "fb-title", textContent: g.title }));
  fallback.style.setProperty("--hue", hue(g.platform + g.title));
  art.append(fallback);

  if (g.coverUrl) {
    const img = el("img", { src: coverSrc(g.coverUrl), alt: "", loading: "lazy", decoding: "async" });
    img.onload = () => fallback.remove();
    img.onerror = () => img.remove();
    art.append(img);
  }

  art.append(el("span", { className: "badge", textContent: status.label }));
  if (isPlaying(g)) art.append(el("span", { className: "np-badge", textContent: "▶ Playing" }));
  if (g.progress > 0) {
    const fill = el("span");
    fill.style.width = `${g.progress}%`;
    art.append(el("div", { className: "progress-bar" }, fill));
  }
  card.replaceChildren(art);
  card.title = `${g.title} — ${g.platform}`;
}

function place(card, d) {
  const a = Math.abs(d);
  const s = Math.sign(d);
  const x = d === 0 ? 0 : s * (200 + (a - 1) * 78);
  const z = d === 0 ? 60 : -140 - a * 18;
  const rot = d === 0 ? 0 : -s * 52;
  card.style.transform = `translateX(${x}px) translateZ(${z}px) rotateY(${rot}deg)`;
  card.style.zIndex = String(100 - a);
  card.style.opacity = a >= WINDOW ? "0" : "1";
  card.classList.toggle("active", d === 0);
}

function resetCarousel() {
  cardEls.clear();
  $("#track").replaceChildren();
}

function renderCarousel() {
  const track = $("#track");
  const { filtered, index } = state;
  const visible = new Set();

  for (let i = Math.max(0, index - WINDOW); i <= Math.min(filtered.length - 1, index + WINDOW); i++) {
    const g = filtered[i];
    visible.add(g.id);
    let card = cardEls.get(g.id);
    if (!card) {
      card = el("div", { className: "card" });
      card.dataset.id = g.id;
      fillCard(card, g);
      // Start new cards at the edge they enter from so they slide in.
      place(card, Math.sign(i - index) * WINDOW);
      track.append(card);
      cardEls.set(g.id, card);
      card.getBoundingClientRect(); // commit the start position before transitioning
    }
    place(card, i - index);
  }
  for (const [id, card] of cardEls) {
    if (!visible.has(id)) {
      card.remove();
      cardEls.delete(id);
    }
  }

  $("#empty").hidden = filtered.length > 0;
  $("#prev").hidden = $("#next").hidden = filtered.length < 2;
  $("#counter").textContent = filtered.length ? `${index + 1} / ${filtered.length}` : "";
}

function go(i) {
  const n = state.filtered.length;
  if (!n) return;
  const next = Math.max(0, Math.min(n - 1, i));
  if (next === state.index) return;
  flushSave();
  state.index = next;
  render();
}

function render() {
  renderCarousel();
  renderDetail();
}

// ---------- detail panel ----------

let saveTimer = null;
let pendingSave = null;

function scheduleSave(g, delay = 400) {
  pendingSave = g;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(flushSave, delay);
}

async function flushSave() {
  clearTimeout(saveTimer);
  const g = pendingSave;
  pendingSave = null;
  if (!g) return;
  const saved = await call("update_game", { id: g.id, input: g }).catch(() => null);
  if (!saved) return;
  Object.assign(g, saved);
  const card = cardEls.get(g.id);
  if (card) fillCard(card, g);
  renderStats();
  renderShelfCounts();
  if (current()?.id === g.id) {
    renderMeta(g);
    loadLog(g.id); // status/priority changes add log entries server-side
  }
}

function edit(mutator, delay) {
  const g = current();
  if (!g) return;
  mutator(g);
  scheduleSave(g, delay);
}

function renderMeta(g) {
  const chips = [g.platform, g.format, g.region, g.ownership !== "owned" ? OWNERSHIP.find(([v]) => v === g.ownership)?.[1] : null]
    .filter(Boolean)
    .map((t) => el("span", { className: "chip", textContent: t }));
  if (g.lastPlayed) chips.push(el("span", { className: "chip", textContent: `Last played ${g.lastPlayed.slice(0, 10)}` }));
  if (g.backloggeryId) {
    chips.push(g.localChanges
      ? el("span", { className: "chip warn", textContent: "Edited locally", title: "Differs from Backloggery until you make the same change there" })
      : el("span", { className: "chip", textContent: "Backloggery" }));
  }
  $("#d-meta").replaceChildren(...chips);
  renderAlsoOn(g);
}

/// Other entries for the same game: other platforms, or a second copy.
function renderAlsoOn(g) {
  const others = state.games.filter((x) => x.id !== g.id && x.titleKey === g.titleKey);
  const box = $("#d-also");
  box.hidden = !others.length;
  box.replaceChildren(el("span", { className: "also-label", textContent: "Also on" }), ...others.map((o) => {
    const label = o.platform === g.platform ? `${o.platform} · ${o.format ?? "another copy"}` : o.platform;
    const b = el("button", { type: "button", className: "chip link", textContent: label, title: `${o.title} · ${statusOf(o.status).label}` });
    b.onclick = () => jumpTo(o.id);
    return b;
  }));
}

/// Selects a game, clearing filters first if it's hidden by them.
function jumpTo(id) {
  flushSave();
  if (!state.filtered.some((g) => g.id === id)) {
    state.search = $("#search").value = "";
    state.platform = $("#platform-filter").value = "";
    state.store = $("#store-filter").value = "";
    state.format = $("#format-filter").value = "";
    setShelf("all", false);
  }
  applyFilters({ keepId: id });
}

function renderStars(g) {
  const box = $("#d-rating");
  box.replaceChildren(...[1, 2, 3, 4, 5].map((n) => {
    const r = g.rating ?? 0;
    const cls = r >= n * 2 ? "full" : r === n * 2 - 1 ? "half" : "";
    const b = el("button", { type: "button", className: cls, textContent: "★", title: `${n} star${n > 1 ? "s" : ""}` });
    // Click cycles: full star → half star → cleared.
    b.onclick = () => edit((g) => {
      const full = n * 2;
      g.rating = g.rating === full ? full - 1 : g.rating === full - 1 ? null : full;
      renderStars(g);
    }, 0);
    return b;
  }));
}

function renderDetail() {
  const g = current();
  $("#detail").hidden = !g;
  if (!g) return;

  $("#d-title").textContent = g.title;
  renderMeta(g);

  $("#d-status").replaceChildren(...STATUSES.map((s) => {
    const dot = el("i");
    const b = el("button", { type: "button", className: s.value === g.status ? "active" : "" }, dot, s.label);
    b.style.setProperty("--c", s.color);
    b.onclick = () => edit((g) => {
      g.status = s.value;
      if ((s.value === "beaten" || s.value === "completed") && g.progress < 100) g.progress = 100;
      renderDetail();
    }, 0);
    return b;
  }));

  $("#d-priority").value = g.priority;
  $("#d-ownership").value = g.ownership;
  renderStars(g);
  $("#d-progress").value = g.progress;
  $("#d-progress-val").textContent = `${g.progress}%`;
  $("#d-hours").value = g.hoursPlayed || "";
  $("#d-ach-done").value = g.achievementsTotal ? g.achievementsDone : "";
  $("#d-ach-total").value = g.achievementsTotal || "";
  $("#d-notes").value = g.notes;

  loadLog(g.id);
  loadSources(g);
}

let sourcesRequest = 0;
async function loadSources(g) {
  const req = ++sourcesRequest;
  const box = $("#d-sources");
  if (!g.stores.length) {
    box.hidden = true;
    return;
  }
  const links = await call("list_game_sources", { gameId: g.id }).catch(() => []);
  if (req !== sourcesRequest) return;
  box.hidden = !links.length;
  $("#d-sources-list").replaceChildren(...links.map((l) => {
    const bits = [];
    if (l.playtimeMinutes) bits.push(`${(l.playtimeMinutes / 60).toFixed(1)} h`);
    if (l.lastPlayed) bits.push(l.lastPlayed.slice(0, 10));
    const store = l.store ?? l.source;
    const name = l.title === g.title ? store : `${store} · ${l.title}`;
    const split = el("button", {
      type: "button",
      className: "link-btn",
      textContent: "Split off",
      title: "This store copy is a different game: move it into its own entry",
    });
    split.onclick = async () => {
      if (!confirm(`Move the ${store} copy “${l.title}” out of “${g.title}” into its own game?`)) return;
      await flushSave();
      const created = await call("split_source", { gameId: g.id, source: l.source, externalId: l.externalId }).catch(() => null);
      if (!created) return;
      await reload();
      toast(`“${created.title}” is now its own game.`);
    };
    return el("li", { title: `${l.source}: ${l.externalId}` },
      el("span", { textContent: name }),
      el("span", { className: "src-right" }, bits.join(" · "), split));
  }));
}

let logRequest = 0;
async function loadLog(gameId) {
  const req = ++logRequest;
  const entries = await call("list_log", { gameId }).catch(() => []);
  if (req !== logRequest) return; // a newer selection already loaded
  $("#log-list").replaceChildren(...entries.map((e) =>
    el("li", {}, el("time", { textContent: e.at.slice(0, 10), title: e.at + " UTC" }), el("span", { textContent: e.note }))));
}

function bindDetail() {
  const fill = (select, items) => select.replaceChildren(...items.map(([v, l]) => el("option", { value: v, textContent: l })));
  fill($("#d-priority"), PRIORITIES);
  fill($("#d-ownership"), OWNERSHIP);

  $("#d-priority").onchange = (e) => edit((g) => (g.priority = e.target.value), 0);
  $("#d-ownership").onchange = (e) => edit((g) => (g.ownership = e.target.value), 0);
  $("#d-progress").oninput = (e) => edit((g) => {
    g.progress = Number(e.target.value);
    $("#d-progress-val").textContent = `${g.progress}%`;
  });
  $("#d-hours").oninput = (e) => edit((g) => (g.hoursPlayed = Number(e.target.value) || 0), 600);
  $("#d-ach-done").oninput = (e) => edit((g) => (g.achievementsDone = Number(e.target.value) || 0), 600);
  $("#d-ach-total").oninput = (e) => edit((g) => (g.achievementsTotal = Number(e.target.value) || 0), 600);
  $("#d-notes").oninput = (e) => edit((g) => (g.notes = e.target.value), 800);

  $("#log-form").onsubmit = async (e) => {
    e.preventDefault();
    const g = current();
    const note = $("#log-input").value.trim();
    if (!g || !note) return;
    await call("add_note", { gameId: g.id, note });
    $("#log-input").value = "";
    loadLog(g.id);
  };

  $("#d-cover").onclick = async () => {
    const g = current();
    if (!g) return;
    const btn = $("#d-cover");
    btn.disabled = true;
    try {
      const url = await call("find_cover", { id: g.id });
      if (url) {
        g.coverUrl = url;
        const card = cardEls.get(g.id);
        if (card) fillCard(card, g);
      } else {
        toast(`No cover found for “${g.title}”. You can paste a URL via Edit.`);
      }
    } finally {
      btn.disabled = false;
    }
  };

  $("#d-edit").onclick = () => openGameDialog(current());
  $("#d-merge").onclick = openMerge;
  $("#merge-close").onclick = () => $("#merge-dialog").close();
  $("#merge-search").oninput = () => renderMergeResults(current());

  $("#d-delete").onclick = async () => {
    const g = current();
    if (!g || !confirm(`Delete “${g.title}” from your library? Its play log is deleted too.`)) return;
    pendingSave = null;
    await call("delete_game", { id: g.id });
    state.games = state.games.filter((x) => x.id !== g.id);
    const next = state.filtered[state.index + 1] ?? state.filtered[state.index - 1];
    afterLibraryChange(next?.id);
  };
}

// ---------- merge ----------

function openMerge() {
  const g = current();
  if (!g) return;
  $("#merge-target").textContent = g.title;
  $("#merge-search").value = "";
  renderMergeResults(g);
  $("#merge-dialog").showModal();
  $("#merge-search").focus();
}

function renderMergeResults(g) {
  const words = $("#merge-search").value.trim().toLowerCase().split(/\s+/).filter(Boolean);
  // Likely duplicates first: the same normalized title, then a shared key word.
  const keyword = g.titleKey.split(" ").find((w) => !["the", "a", "an"].includes(w)) ?? "";
  const rank = (x) => (x.titleKey === g.titleKey ? 0 : keyword && x.titleKey.split(" ").includes(keyword) ? 1 : 2);
  const matches = state.games
    .filter((x) => x.id !== g.id && words.every((w) => x.title.toLowerCase().includes(w)))
    .filter((x) => words.length || rank(x) < 2)
    .sort((a, b) => rank(a) - rank(b) || SORTS.title(a, b))
    .slice(0, 50);

  $("#merge-results").replaceChildren(...(matches.length
    ? matches.map((x) => {
      const meta = [x.platform, x.format, storesOf(x).join(", "), statusOf(x.status).label].filter(Boolean).join(" · ");
      const b = el("button", { type: "button" },
        el("strong", { textContent: x.title }),
        el("span", { className: "hint", textContent: meta }));
      b.onclick = () => mergeInto(g, x);
      return el("li", {}, b);
    })
    : [el("li", { className: "hint", textContent: words.length ? "No matches." : "No likely duplicates. Search your library above." })]));
}

async function mergeInto(target, other) {
  const ok = confirm(`Merge “${other.title}” (${other.platform}) into “${target.title}” (${target.platform})?

“${other.title}” will be removed from your library.`);
  if (!ok) return;
  await flushSave();
  const merged = await call("merge_games", { targetId: target.id, otherId: other.id }).catch(() => null);
  if (!merged) return;
  $("#merge-dialog").close();
  await reload();
  toast(`Merged into “${merged.title}”.`);
}

// ---------- add / edit dialog ----------

let editing = null;

function openGameDialog(game) {
  editing = game ?? null;
  const form = $("#game-form");
  form.reset();
  $("#game-dialog-title").textContent = game ? "Edit game" : "Add game";
  const f = form.elements;
  if (game) {
    f.title.value = game.title;
    f.platform.value = game.platform;
    f.status.value = game.status;
    f.ownership.value = game.ownership;
    f.format.value = game.format ?? "";
    f.region.value = game.region ?? "";
    f.coverUrl.value = game.coverUrl ?? "";
  } else if (state.platform) {
    f.platform.value = state.platform;
  }
  $("#game-dialog").showModal();
  f.title.focus();
}

function bindGameDialog() {
  const f = $("#game-form").elements;
  f.status.replaceChildren(...STATUSES.map((s) => el("option", { value: s.value, textContent: s.label })));
  f.ownership.replaceChildren(...OWNERSHIP.map(([v, l]) => el("option", { value: v, textContent: l })));

  $("#game-dialog").addEventListener("close", async () => {
    if ($("#game-dialog").returnValue !== "save") return;
    const fields = {
      title: f.title.value,
      platform: f.platform.value,
      status: f.status.value,
      ownership: f.ownership.value,
      format: f.format.value || null,
      region: f.region.value || null,
      coverUrl: f.coverUrl.value || null,
    };
    if (editing) {
      await flushSave();
      const saved = await call("update_game", { id: editing.id, input: { ...editing, ...fields } }).catch(() => null);
      if (!saved) return;
      Object.assign(editing, saved);
      cardEls.get(editing.id)?.remove();
      cardEls.delete(editing.id);
      afterLibraryChange(editing.id);
    } else {
      const created = await call("create_game", { input: fields }).catch(() => null);
      if (!created) return;
      state.games.push(created);
      // Make sure the new game is visible in the current view.
      if (!SHELVES[state.shelf](created)) setShelf("all", false);
      state.search = $("#search").value = "";
      afterLibraryChange(created.id);
      toast(`Added “${created.title}”`);
    }
  });
}

// ---------- settings / sync ----------

async function openSettings() {
  const s = await call("get_settings").catch(() => ({}));
  const f = $("#settings-form").elements;
  f.backloggeryUsername.value = s.backloggeryUsername ?? "";
  f.steamgriddbKey.value = s.steamgriddbKey ?? "";
  $("#last-sync").textContent = s.lastSync ? `Last synced ${s.lastSync} UTC.` : "";
  $("#settings-dialog").showModal();
}

async function saveSettings() {
  const f = $("#settings-form").elements;
  await call("save_settings", {
    settings: {
      backloggeryUsername: f.backloggeryUsername.value.trim() || null,
      steamgriddbKey: f.steamgriddbKey.value.trim() || null,
    },
  });
}

async function sync() {
  const btn = $("#sync-btn");
  btn.disabled = true;
  btn.textContent = "⟳ Syncing…";
  try {
    await flushSave();
    const r = await call("sync_backloggery");
    let msg = `Backloggery sync: ${r.added} added, ${r.updated} updated, ${r.unchanged} unchanged.`;
    if (r.overwrittenLocal) msg += ` ${r.overwrittenLocal} local edits replaced by newer Backloggery changes.`;
    if (r.missingRemote) msg += ` ${r.missingRemote} games are no longer on Backloggery (kept here).`;
    toast(msg);
    await reload();
  } catch {
    // already toasted
  } finally {
    btn.disabled = false;
    btn.textContent = "⟳ Sync";
  }
}

function bindSettings() {
  $("#settings-dialog").addEventListener("close", () => {
    if ($("#settings-dialog").returnValue === "save") saveSettings().then(() => toast("Settings saved")).catch(() => {});
  });

  $("#fetch-covers").onclick = async () => {
    const btn = $("#fetch-covers");
    const bar = $("#cover-progress");
    btn.disabled = true;
    bar.hidden = false;
    bar.removeAttribute("value");
    try {
      await saveSettings();
      const r = await call("fetch_missing_covers");
      toast(`Covers: ${r.found} found, ${r.notFound} not found${r.failed ? `, ${r.failed} failed` : ""}.`);
    } catch {
      // already toasted
    } finally {
      btn.disabled = false;
      bar.hidden = true;
    }
  };

  listen("covers-progress", ({ payload }) => {
    const bar = $("#cover-progress");
    bar.max = payload.total;
    bar.value = payload.done;
    if (!payload.url) return;
    const g = state.games.find((x) => x.id === payload.id);
    if (!g) return;
    g.coverUrl = payload.url;
    const card = cardEls.get(g.id);
    if (card) fillCard(card, g);
  });
}

// ---------- links ----------

const URL_PATTERN = /https?:\/\/[^\s<>"]+/g;

/// Plain help text with any URLs turned into links (opened by `openExternal`).
function linkify(text) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    // Sentence punctuation right after a URL isn't part of it.
    const url = match[0].replace(/[.,;:!?)]+$/, "");
    nodes.push(text.slice(last, match.index));
    nodes.push(el("a", { href: url, textContent: url.replace(/^https?:\/\/(www\.)?/, ""), title: url }));
    last = match.index + url.length;
  }
  nodes.push(text.slice(last));
  return nodes.filter((n) => n !== "");
}

/// Web links open in the user's browser; navigating the app window would lose the library view.
function openExternal(e) {
  const a = e.target.closest("a[href]");
  if (!a || !/^(https?|mailto):/i.test(a.href)) return;
  e.preventDefault();
  window.__TAURI__.opener.openUrl(a.href).catch((err) => toast(`Couldn't open the link: ${err}`, "error"));
}

// ---------- import ----------

async function openImport() {
  const view = await call("list_sources").catch(() => null);
  if (!view) return;
  $("#plugins-dir").textContent = view.pluginsDir;
  $("#source-list").replaceChildren(...view.sources.map(sourceCard));
  $("#import-dialog").showModal();
}

function sourceCard(src) {
  const inputs = src.fields.map((f) => {
    const detected = src.detected[f.key];
    const input = el("input", {
      type: f.kind === "secret" ? "password" : "text",
      value: src.config[f.key] ?? "",
      placeholder: detected ? `${detected} (detected)` : f.placeholder ?? "",
      autocomplete: "off",
      spellcheck: false,
    });
    input.dataset.key = f.key;
    const label = el("label", {},
      el("span", { textContent: f.label + (f.optional ? " (optional)" : "") }), input,
      f.help ? el("small", {}, ...linkify(f.help)) : null);
    return { input, label };
  });

  const lastImport = src.lastImport ? `Last imported ${src.lastImport} UTC` : "Not imported yet";
  const status = el("p", { className: "hint source-status", textContent: lastImport });
  const button = el("button", { type: "button", className: "btn primary", textContent: "Import" });
  button.onclick = async () => {
    const config = Object.fromEntries(inputs.map(({ input }) => [input.dataset.key, input.value.trim()]));
    button.disabled = true;
    button.textContent = "Importing…";
    status.classList.remove("error");
    status.textContent = "Working… large libraries can take a minute.";
    try {
      await invoke("save_source_config", { id: src.id, config });
      await flushSave();
      const r = await invoke("run_import", { id: src.id });
      const parts = [`${r.added} added`, `${r.matched} matched existing`, `${r.updated} refreshed`];
      if (r.skipped) parts.push(`${r.skipped} skipped (you deleted them)`);
      status.textContent = parts.join(" · ");
      await reload();
    } catch (err) {
      status.textContent = String(err);
      status.classList.add("error");
    } finally {
      button.disabled = false;
      button.textContent = "Import";
    }
  };

  const builtIn = src.origin === "built-in";
  return el("section", { className: "source-card" },
    el("header", {},
      el("h3", { textContent: src.name }),
      el("span", { className: `tag ${builtIn ? "" : "plugin"}`, textContent: builtIn ? "Built-in" : "Plugin", title: src.origin })),
    el("p", { className: "hint" }, ...linkify(src.description)),
    ...inputs.map((i) => i.label),
    el("div", { className: "source-actions" }, button, status));
}

// ---------- input ----------

function setShelf(shelf, apply = true) {
  state.shelf = shelf;
  for (const b of document.querySelectorAll("#shelves button")) b.classList.toggle("active", b.dataset.shelf === shelf);
  if (apply) applyFilters({ keepId: current()?.id });
}

function bindInput() {
  $("#prev").onclick = () => go(state.index - 1);
  $("#next").onclick = () => go(state.index + 1);

  $("#track").addEventListener("click", (e) => {
    const card = e.target.closest(".card");
    if (!card || dragMoved) return;
    const i = state.filtered.findIndex((g) => g.id === Number(card.dataset.id));
    if (i >= 0) go(i);
  });

  // Wheel / trackpad: accumulate so a flick moves a few cards, not dozens.
  let wheelAcc = 0;
  $("#stage").addEventListener("wheel", (e) => {
    e.preventDefault();
    wheelAcc += Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    const steps = Math.trunc(wheelAcc / 80);
    if (steps) {
      wheelAcc -= steps * 80;
      go(state.index + steps);
    }
  }, { passive: false });

  // Drag / swipe.
  let dragX = null;
  let dragMoved = false;
  const stage = $("#stage");
  stage.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button")) return;
    dragX = e.clientX;
    dragMoved = false;
  });
  window.addEventListener("pointermove", (e) => {
    if (dragX == null) return;
    const dx = e.clientX - dragX;
    if (Math.abs(dx) > 70) {
      go(state.index - Math.sign(dx));
      dragX = e.clientX;
      dragMoved = true;
    }
  });
  window.addEventListener("pointerup", () => {
    dragX = null;
    setTimeout(() => (dragMoved = false));
  });

  document.addEventListener("keydown", (e) => {
    const typing = e.target.matches("input, textarea, select") || document.querySelector("dialog[open]");
    if (typing) {
      if (e.key === "Escape" && e.target === $("#search")) e.target.blur();
      return;
    }
    const keys = {
      ArrowLeft: () => go(state.index - 1),
      ArrowRight: () => go(state.index + 1),
      PageUp: () => go(state.index - 10),
      PageDown: () => go(state.index + 10),
      Home: () => go(0),
      End: () => go(state.filtered.length - 1),
      "/": () => $("#search").focus(),
      n: () => openGameDialog(),
    };
    // Number keys 1-5 jump between shelves.
    const shelfKeys = Object.keys(SHELVES);
    const action = keys[e.key] ?? (e.key >= "1" && e.key <= "5" ? () => setShelf(shelfKeys[Number(e.key) - 1]) : null);
    if (action) {
      e.preventDefault();
      action();
    }
  });

  let searchTimer;
  $("#search").addEventListener("input", (e) => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      state.search = e.target.value;
      applyFilters();
    }, 150);
  });
  $("#platform-filter").onchange = (e) => {
    state.platform = e.target.value;
    applyFilters();
  };
  $("#store-filter").onchange = (e) => {
    state.store = e.target.value;
    applyFilters();
  };
  $("#format-filter").onchange = (e) => {
    state.format = e.target.value;
    applyFilters();
  };
  $("#sort").onchange = (e) => {
    state.sort = e.target.value;
    applyFilters({ keepId: current()?.id });
  };
  for (const b of document.querySelectorAll("#shelves button")) b.onclick = () => setShelf(b.dataset.shelf);

  $("#add-btn").onclick = () => openGameDialog();
  $("#sync-btn").onclick = sync;
  $("#import-btn").onclick = openImport;
  $("#import-close").onclick = () => $("#import-dialog").close();
  $("#settings-btn").onclick = openSettings;
  $("#empty").addEventListener("click", (e) => {
    const action = e.target.closest("[data-action]")?.dataset.action;
    if (action === "add") openGameDialog();
    if (action === "import") openImport();
    if (action === "settings") openSettings();
  });

  document.addEventListener("click", openExternal);
  window.addEventListener("beforeunload", () => flushSave());
}

// ---------- startup ----------

function afterLibraryChange(keepId) {
  refreshPlatforms();
  renderStats();
  renderShelfCounts();
  applyFilters({ keepId });
}

async function reload() {
  const keepId = current()?.id;
  state.games = await call("list_games");
  afterLibraryChange(keepId);
}

bindDetail();
bindGameDialog();
bindSettings();
bindInput();
reload().then(() => $("#stage").focus());
