// ---- Media tab: Netflix-style hierarchical browse (primary view) plus the
// original flat library table (secondary "All entries" view, Phase 3) with
// per-row metadata/artwork/rescrape CRUD. The browse view is built entirely
// client-side over the same flat `list_entries` data — see the plan's
// "hierarchy is grouped client-side" decision — and reuses the flat table's
// existing manage-panel functions (`manageRow`/`saveEdit`/`uploadArtwork`/
// `rescrapeEntry`) for per-entry actions rather than duplicating them.

let libraryEntries = [];
let openManageKey = null;
let pickedArtworkPath = null;
let mediaSection = "browse"; // "browse" | "table"
let browsePath = { kind: "root" }; // breadcrumb state — see renderBrowse()
let searchQuery = "";
let kindFilter = "all"; // "all" | "movie" | "episode" | "track"
// Categories = genres — auto-populated by scraping, and/or assigned by hand
// in the edit panel (see manageRow's category picker). Not a separate
// concept/table: every distinct genre value in use anywhere in the library
// doubles as a browsable, filterable "category".
let categoryFilter = "all";
let allCategories = []; // every distinct genre/category currently in use — refreshed alongside libraryEntries
const KIND_FILTER_LABELS = { all: "Filter: all kinds", movie: "Filter: Movies", episode: "Filter: Shows", track: "Filter: Music" };

// Applies to both the Browse root view and the All-entries table. Matches
// against every identifying name field, not just title, so searching a
// show/artist name keeps every episode/track under it (their show_title/
// artist is stable across all of them, even though the episode/track's own
// title might not mention it).
function filteredEntries() {
  const q = searchQuery.trim().toLowerCase();
  return libraryEntries.filter(e => {
    if (kindFilter !== "all" && e.kind !== kindFilter) return false;
    if (categoryFilter !== "all" && !(e.genres || []).includes(categoryFilter)) return false;
    if (!q) return true;
    return [e.scraped_title, e.title, e.artist, e.album, e.show_title]
      .filter(Boolean)
      .some(field => field.toLowerCase().includes(q));
  });
}

async function refreshMedia() {
  await refreshLibrary();
}

async function refreshLibrary() {
  const library = document.getElementById("library");
  try {
    [libraryEntries, allCategories] = await Promise.all([invoke("list_entries"), invoke("list_categories")]);
  } catch (err) {
    library.innerHTML = `<p class="muted">Unable to load library.</p>`;
    showToast(String(err), "error");
    return;
  }
  renderMediaTab();
}

// Re-renders just the current view's results (Browse or the flat table),
// never the toggle/search row itself — called on every search keystroke and
// filter change, so the search <input> is never destroyed/recreated (which
// would drop keyboard focus and cursor position mid-type).
function renderMediaResults() {
  if (mediaSection === "table") {
    renderLibrary();
  } else {
    renderBrowse();
  }
}

function renderMediaTab() {
  const container = document.getElementById("library");
  const toggle = `<div class="row" style="margin-bottom:14px">
    <button class="${mediaSection === "browse" ? "" : "secondary"}" id="mediaSectionBrowseBtn" style="flex:0 0 auto"><i class="bi bi-grid"></i>Browse</button>
    <button class="${mediaSection === "table" ? "" : "secondary"}" id="mediaSectionTableBtn" style="flex:0 0 auto"><i class="bi bi-list-ul"></i>All entries</button>
  </div>
  <div class="row" style="margin-bottom:14px; align-items:center">
    <div class="search-input-wrap" style="flex:2">
      <i class="bi bi-search search-input-icon"></i>
      <input id="mediaSearchInput" class="search-input" placeholder="Search title, artist, show…" value="${esc(searchQuery)}">
    </div>
    <div class="icon-select-wrap">
      <select id="mediaKindFilter" class="icon-select${kindFilter !== "all" ? " icon-select-active" : ""}" title="${KIND_FILTER_LABELS[kindFilter]}">
        <option value="all">All kinds</option>
        <option value="movie">Movies</option>
        <option value="episode">Shows</option>
        <option value="track">Music</option>
      </select>
      <i class="bi bi-funnel icon-select-icon"></i>
    </div>
    <div class="icon-select-wrap">
      <select id="mediaCategoryFilter" class="icon-select${categoryFilter !== "all" ? " icon-select-active" : ""}" title="${categoryFilter === "all" ? "Filter: all categories" : `Filter: ${categoryFilter}`}">
        <option value="all">All categories</option>
        ${allCategories.map(c => `<option value="${esc(c)}"${c === categoryFilter ? " selected" : ""}>${esc(c)}</option>`).join("")}
      </select>
      <i class="bi bi-tags icon-select-icon"></i>
    </div>
  </div>`;
  container.innerHTML = toggle + `<div id="mediaSectionBody"></div>`;
  document.getElementById("mediaSectionBrowseBtn").addEventListener("click", () => {
    mediaSection = "browse";
    browsePath = { kind: "root" };
    renderMediaTab();
  });
  document.getElementById("mediaSectionTableBtn").addEventListener("click", () => {
    mediaSection = "table";
    renderMediaTab();
  });
  document.getElementById("mediaSearchInput").addEventListener("input", (event) => {
    searchQuery = event.target.value;
    if (mediaSection === "browse") browsePath = { kind: "root" };
    renderMediaResults();
  });
  const kindFilterSelect = document.getElementById("mediaKindFilter");
  kindFilterSelect.value = kindFilter;
  kindFilterSelect.addEventListener("change", (event) => {
    kindFilter = event.target.value;
    kindFilterSelect.title = KIND_FILTER_LABELS[kindFilter];
    kindFilterSelect.classList.toggle("icon-select-active", kindFilter !== "all");
    if (mediaSection === "browse") browsePath = { kind: "root" };
    renderMediaResults();
  });
  const categoryFilterSelect = document.getElementById("mediaCategoryFilter");
  categoryFilterSelect.addEventListener("change", (event) => {
    categoryFilter = event.target.value;
    categoryFilterSelect.title = categoryFilter === "all" ? "Filter: all categories" : `Filter: ${categoryFilter}`;
    categoryFilterSelect.classList.toggle("icon-select-active", categoryFilter !== "all");
    if (mediaSection === "browse") browsePath = { kind: "root" };
    renderMediaResults();
  });
  renderMediaResults();
}

// ---- pure grouping helpers (no DOM) ----------------------------------------
// Entries with no artist/album/show_title/season are bucketed under an
// "Unknown …" label rather than dropped — every entry must remain reachable.

function groupTracks(entries) {
  const byArtist = new Map();
  for (const e of entries.filter(e => e.kind === "track")) {
    const artist = e.artist || "Unknown Artist";
    const album = e.album || "Unknown Album";
    if (!byArtist.has(artist)) byArtist.set(artist, new Map());
    const albums = byArtist.get(artist);
    if (!albums.has(album)) albums.set(album, []);
    albums.get(album).push(e);
  }
  for (const albums of byArtist.values()) {
    for (const tracks of albums.values()) {
      tracks.sort((a, b) => (a.track_number ?? Infinity) - (b.track_number ?? Infinity));
    }
  }
  return byArtist; // Map<artist, Map<album, EntrySummary[]>>
}

// Two on-disk show folders can hold the same real series under different
// names ("Law & Order SVU" vs. "Law & Order Special Victims Unit") — the
// path-derived `show_title` alone can never tell them apart (and by design
// never should: a bad scrape must never be able to split or corrupt a
// grouping that's otherwise correct — see classify.rs's module doc
// comment). A *matching* scrape is different: if episodes under two
// different show_titles agree on the same TMDb-confirmed `scraped_title`,
// that's real external corroboration, not something a bad scrape could
// coincidentally produce for two unrelated folders. So: fold any show_title
// whose episodes have a clear scraped_title consensus into that canonical
// key, purely for display grouping — the underlying entries and their
// path-derived fields are never touched.
function canonicalShowKeys(episodes) {
  const scrapedCounts = new Map(); // show_title -> Map<scraped_title, count>
  for (const e of episodes) {
    const show = e.show_title || "Unknown Show";
    if (!e.scraped_title) continue;
    if (!scrapedCounts.has(show)) scrapedCounts.set(show, new Map());
    const counts = scrapedCounts.get(show);
    counts.set(e.scraped_title, (counts.get(e.scraped_title) || 0) + 1);
  }
  const canonicalFor = new Map(); // show_title -> canonical display key
  for (const [show, counts] of scrapedCounts) {
    const [topScraped] = [...counts.entries()].sort((a, b) => b[1] - a[1])[0];
    canonicalFor.set(show, topScraped);
  }
  return canonicalFor;
}

function groupEpisodes(entries) {
  const episodes = entries.filter(e => e.kind === "episode");
  const canonicalFor = canonicalShowKeys(episodes);
  const byShow = new Map();
  for (const e of episodes) {
    const rawShow = e.show_title || "Unknown Show";
    const show = canonicalFor.get(rawShow) || rawShow;
    const season = e.season ?? -1; // -1 = "Unknown Season" bucket, sorts first
    if (!byShow.has(show)) byShow.set(show, new Map());
    const seasons = byShow.get(show);
    if (!seasons.has(season)) seasons.set(season, []);
    seasons.get(season).push(e);
  }
  for (const seasons of byShow.values()) {
    for (const episodes of seasons.values()) {
      episodes.sort((a, b) => (a.episode ?? Infinity) - (b.episode ?? Infinity));
    }
  }
  return byShow; // Map<show, Map<season, EntrySummary[]>>
}

// ---- artwork loading --------------------------------------------------------
// The webview has no HTTP path to `/art/...` (that's P2P-QUIC-only — see
// `get_artwork_bytes`'s doc comment in gui.rs), so cards render a blank
// placeholder synchronously, then swap in a blob: URL once bytes arrive.
// Failure (no artwork of that kind) just leaves the placeholder — never an
// error the user needs to see.
//
// Grid thumbnails (`card-art` — Movies/Shows/Music browse grids, where
// dozens can render on screen at once) load on demand, as they actually
// scroll into view, rather than all at once: `artImg` registers them with a
// shared `IntersectionObserver` instead of fetching immediately. Single-
// image detail views (a movie's own poster/backdrop, `detail-poster`/
// `detail-backdrop`) still load eagerly — there's only ever one or two on
// screen, so there's no batch-load cost to avoid, and deferring them would
// just make detail pages look broken on open.

// rootMargin starts the fetch a little before a card is actually on
// screen, so scrolling doesn't visibly outrun the image loading in.
const artworkObserver = new IntersectionObserver(
  (entries) => {
    for (const observed of entries) {
      if (!observed.isIntersecting) continue;
      artworkObserver.unobserve(observed.target);
      const { entryKey, kind } = observed.target.dataset;
      loadArtworkInto(observed.target, entryKey, kind);
    }
  },
  { rootMargin: "200px" },
);
//
// A fetched image is cached in memory for `ARTWORK_CACHE_TTL_MS` (5
// minutes, matching the TV client's own `ArtworkCache` TTL default) keyed
// by entry+kind, so re-hovering the same card — or navigating back to a
// grid you already browsed — doesn't re-read the file over IPC every time.
// `clearArtworkCache()` is called after anything that can change artwork on
// disk (a bulk/pinpoint scrape, a manual upload, a scrape revert) so a
// stale cached image can never outlive the data it was fetched for.

const ARTWORK_CACHE_TTL_MS = 5 * 60 * 1000;
const artworkCache = new Map(); // `${entryKey}:${kind}` -> { blobUrl, expiresAt }

function clearArtworkCache() {
  for (const { blobUrl } of artworkCache.values()) URL.revokeObjectURL(blobUrl);
  artworkCache.clear();
}

async function loadArtworkInto(imgEl, entryKey, kind) {
  const cacheKey = `${entryKey}:${kind}`;
  const cached = artworkCache.get(cacheKey);
  if (cached && cached.expiresAt > Date.now()) {
    imgEl.src = cached.blobUrl;
    imgEl.classList.remove("art-placeholder");
    return;
  }
  try {
    const bytes = await invoke("get_artwork_bytes", { entryKey, kind });
    if (!bytes || !bytes.length) return;
    // Declared MIME doesn't need to match the actual bytes exactly for a
    // blob: URL fed to <img> — every engine this webview runs on (WebKit/
    // Chromium/Gecko) sniffs the real image format for rendering.
    const blob = new Blob([new Uint8Array(bytes)], { type: "image/jpeg" });
    if (cached) URL.revokeObjectURL(cached.blobUrl);
    const blobUrl = URL.createObjectURL(blob);
    artworkCache.set(cacheKey, { blobUrl, expiresAt: Date.now() + ARTWORK_CACHE_TTL_MS });
    imgEl.src = blobUrl;
    imgEl.classList.remove("art-placeholder");
  } catch {
    // no artwork of this kind — leave the placeholder in place
  }
}

function artImg(entryKey, kind, className) {
  const id = `art-${kind}-${entryKey}-${Math.random().toString(36).slice(2)}`;
  const lazy = className.includes("card-art");
  queueMicrotask(() => {
    const el = document.getElementById(id);
    if (!el) return;
    if (lazy) {
      artworkObserver.observe(el);
    } else {
      loadArtworkInto(el, entryKey, kind);
    }
  });
  return `<img id="${id}" class="${className} art-placeholder" data-entry-key="${esc(entryKey)}" data-kind="${esc(kind)}" alt="">`;
}

/** A small heart+count overlay on a browse-grid card's artwork — only for
 * cards backed by one real entry_key (movies, episodes), since a like is
 * per-file and a grouped show/artist/album card has no single count to
 * show. Omitted entirely when nobody's liked it, same as `.badge-count`'s
 * own d-none-when-zero convention elsewhere in this app. */
function likeBadge(likeCount) {
  if (!likeCount) return "";
  return `<span class="like-badge"><i class="bi bi-heart-fill"></i> ${likeCount}</span>`;
}

// ---- browse: root (Movies / Music / Shows section pickers) -----------------

function renderBrowse() {
  const body = document.getElementById("mediaSectionBody");
  if (!libraryEntries.length) {
    body.innerHTML = `<span class="muted">No media found under the configured media roots.</span>`;
    return;
  }
  if (browsePath.kind === "root") return renderBrowseRoot(body);
  if (browsePath.kind === "movie-detail") return renderMovieDetail(body, browsePath.key);
  if (browsePath.kind === "artist") return renderArtist(body, browsePath.artist);
  if (browsePath.kind === "album") return renderAlbum(body, browsePath.artist, browsePath.album);
  if (browsePath.kind === "show") return renderShow(body, browsePath.show);
  if (browsePath.kind === "season") return renderSeason(body, browsePath.show, browsePath.season);
  if (browsePath.kind === "episode-detail") return renderEpisodeDetail(body, browsePath.key);
  body.innerHTML = `<span class="muted">Unknown view.</span>`;
}

function breadcrumb(parts) {
  // parts: [{label, onClick} | {label} (current, no link)]
  return `<div class="breadcrumb">` + parts.map((p, i) => {
    const sep = i > 0 ? `<span class="crumb-sep">›</span>` : "";
    if (p.onClick) return `${sep}<button class="crumb-link" data-crumb="${i}">${esc(p.label)}</button>`;
    return `${sep}<span class="crumb-current">${esc(p.label)}</span>`;
  }).join("") + `</div>`;
}

function wireBreadcrumb(container, parts) {
  container.querySelectorAll("[data-crumb]").forEach(btn => {
    btn.addEventListener("click", () => {
      parts[Number(btn.dataset.crumb)].onClick();
      renderBrowse();
    });
  });
}

function renderBrowseRoot(body) {
  const entries = filteredEntries();
  const movies = entries.filter(e => e.kind === "movie");
  const tracks = groupTracks(entries);
  const shows = groupEpisodes(entries);

  const movieCards = movies.map(m => `
    <div class="media-card" data-movie="${esc(m.entry_key)}">
      ${artImg(m.entry_key, "poster", "card-art")}
      ${likeBadge(m.like_count)}
      <div class="card-title" title="${esc(m.scraped_title || m.title)}">${esc(m.scraped_title || m.title)}${m.year ? ` <span class="muted">(${m.year})</span>` : ""}</div>
    </div>`).join("");

  const artistCards = [...tracks.keys()].sort().map(artist => {
    const firstTrack = [...tracks.get(artist).values()][0]?.[0];
    return `
    <div class="media-card" data-artist="${esc(artist)}">
      ${firstTrack ? artImg(firstTrack.entry_key, "artist", "card-art round") : `<div class="card-art art-placeholder round"></div>`}
      <div class="card-title" title="${esc(artist)}">${esc(artist)}</div>
      <div class="muted" style="font-size:.75rem">${tracks.get(artist).size} album${tracks.get(artist).size === 1 ? "" : "s"}</div>
    </div>`;
  }).join("");

  const showCards = [...shows.keys()].sort().map(show => {
    const first = [...shows.get(show).values()][0]?.[0];
    return `
    <div class="media-card" data-show="${esc(show)}">
      ${first ? artImg(first.entry_key, "poster", "card-art") : `<div class="card-art art-placeholder"></div>`}
      <div class="card-title" title="${esc(show)}">${esc(show)}</div>
      <div class="muted" style="font-size:.75rem">${shows.get(show).size} season${shows.get(show).size === 1 ? "" : "s"}</div>
    </div>`;
  }).join("");

  // One .shelf-section per kind, each a full wrapping grid (no horizontal
  // scroll — every tile visible). Deeper drill-down views (an artist's
  // albums, a show's seasons, etc.) stay as plain .media-grid too.
  const nothingMatched = !movies.length && !tracks.size && !shows.size;
  const emptyMessage = searchQuery.trim() || kindFilter !== "all"
    ? "No matches for the current search/filter."
    : "No movies, music, or shows found yet.";
  body.innerHTML = `
    ${movies.length ? `<div class="shelf-section"><h2 style="margin-top:0">Movies</h2><div class="media-grid">${movieCards}</div></div>` : ""}
    ${shows.size ? `<div class="shelf-section"><h2>Shows</h2><div class="media-grid">${showCards}</div></div>` : ""}
    ${tracks.size ? `<div class="shelf-section"><h2>Music</h2><div class="media-grid">${artistCards}</div></div>` : ""}
    ${nothingMatched ? `<span class="muted">${emptyMessage}</span>` : ""}
  `;

  body.querySelectorAll("[data-movie]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "movie-detail", key: el.dataset.movie };
    renderBrowse();
  }));
  body.querySelectorAll("[data-artist]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "artist", artist: el.dataset.artist };
    renderBrowse();
  }));
  body.querySelectorAll("[data-show]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "show", show: el.dataset.show };
    renderBrowse();
  }));
}

// ---- browse: movie detail ---------------------------------------------------

function detailView(entry, backCrumbs) {
  const cast = (entry.cast || []).slice(0, 10);
  const slash = entry.relative_path.lastIndexOf("/");
  const fileName = slash === -1 ? entry.relative_path : entry.relative_path.slice(slash + 1);
  const fileLocation = slash === -1 ? "(root)" : entry.relative_path.slice(0, slash);
  return `
    ${breadcrumb(backCrumbs)}
    <div class="detail-view">
      ${artImg(entry.entry_key, "backdrop", "detail-backdrop")}
      <div class="detail-body">
        ${artImg(entry.entry_key, "poster", "detail-poster")}
        <div>
          <h2 style="margin-top:0; text-transform:none; font-size:1.2rem; color:var(--text)">
            ${esc(entry.scraped_title || entry.title)}${entry.year ? ` <span class="muted">(${entry.year})</span>` : ""}
            ${entry.rating ? `<span class="tag" style="margin-left:8px; vertical-align:middle">${esc(entry.rating)}</span>` : ""}
            ${entry.like_count ? `<span class="muted" style="margin-left:8px; font-size:.85rem; vertical-align:middle"><i class="bi bi-heart-fill" style="color:#ff5d7a"></i> ${entry.like_count}</span>` : ""}
          </h2>
          ${entry.genres.length ? `<div class="category-chips">${entry.genres.map(g => `<button class="category-chip" data-filter-category="${esc(g)}">${esc(g)}</button>`).join("")}</div>` : ""}
          ${entry.overview ? `<p class="muted" style="margin-top:8px">${esc(entry.overview)}</p>` : ""}
          ${cast.length ? `<h2>Cast</h2><p>${cast.map(c => esc(c.character ? `${c.name} as ${c.character}` : c.name)).join(", ")}</p>` : ""}
          <h2>File</h2>
          <p class="mono muted" style="font-size:.78rem" title="${esc(entry.relative_path)}">
            ${esc(fileName)}<br><span style="opacity:.75">${esc(fileLocation)}</span>
          </p>
        </div>
      </div>
      <div id="detailManage"></div>
    </div>`;
}

function wireDetailManage(entry) {
  const target = document.getElementById("detailManage");
  if (!target) return;
  const wasOpen = openManageKey === entry.entry_key;
  target.innerHTML = `<div class="row" style="margin-top:14px">
    <button class="secondary" data-detail-manage="${esc(entry.entry_key)}">${wasOpen ? '<i class="bi bi-x-lg"></i>Close' : '<i class="bi bi-sliders"></i>Manage metadata / artwork / rescrape'}</button>
  </div>${wasOpen ? manageRow(entry) : ""}`;
  target.querySelector("[data-detail-manage]").addEventListener("click", () => {
    openManageKey = wasOpen ? null : entry.entry_key;
    renderBrowse();
  });
  if (wasOpen) {
    document.getElementById("editSaveBtn")?.addEventListener("click", () => saveEdit(entry.entry_key));
    document.getElementById("pickArtworkBtn")?.addEventListener("click", pickArtwork);
    document.getElementById("uploadArtworkBtn")?.addEventListener("click", () => uploadArtwork(entry.entry_key));
    document.getElementById("rescrapeBtn")?.addEventListener("click", () => rescrapeEntry(entry.entry_key));
    document.getElementById("revertScrapeBtn")?.addEventListener("click", () => revertScrape(entry.entry_key));
    wireAddCategoryBtn();
    wireMoveKindFields(entry.entry_key);
  }
}

function renderMovieDetail(body, entryKey) {
  const entry = libraryEntries.find(e => e.entry_key === entryKey);
  if (!entry) { browsePath = { kind: "root" }; return renderBrowse(); }
  const crumbs = [{ label: "Media", onClick: () => browsePath = { kind: "root" } }, { label: entry.scraped_title || entry.title }];
  body.innerHTML = detailView(entry, crumbs);
  wireBreadcrumb(body, crumbs);
  wireDetailManage(entry);
  body.querySelectorAll("[data-filter-category]").forEach(el => el.addEventListener("click", () => {
    categoryFilter = el.dataset.filterCategory;
    browsePath = { kind: "root" };
    renderMediaTab();
  }));
}

// ---- browse: music (artist → album → tracks) -------------------------------

// ---- group-level artwork upload (artist photo, album cover) ---------------
// Neither an artist nor an album has an `entry_key` of its own — both are
// groupings computed client-side over the flat entry list (see the file's
// top comment) — so uploading artwork "for the artist"/"for the album"
// means applying it to every track entry in that group at once, via
// `upload_group_artwork` (apps/server/src/gui.rs), the group counterpart of
// the per-entry `uploadArtwork` below. Uses its own element ids and state
// (`pickedGroupArtworkPath`, distinct from `pickedArtworkPath`) so this
// panel and an open per-track manage panel (which also has an upload
// control) can coexist on the same page — renderAlbum shows both at once.

let pickedGroupArtworkPath = null;

function groupArtworkPanel(label) {
  return `
    <div class="row" style="margin:10px 0 4px; align-items:center">
      <button id="groupArtworkPickBtn" class="secondary"><i class="bi bi-image"></i>Choose image…</button>
      <button id="groupArtworkUploadBtn"><i class="bi bi-upload"></i>Upload ${label}</button>
      <span class="muted" id="groupArtworkPickedNote" style="font-size:.8rem">${pickedGroupArtworkPath ? esc(pickedGroupArtworkPath) : "No file chosen."}</span>
    </div>`;
}

async function pickGroupArtwork() {
  try {
    pickedGroupArtworkPath = await invoke("pick_file_path");
    document.getElementById("groupArtworkPickedNote").textContent = pickedGroupArtworkPath || "No file chosen.";
  } catch (err) {
    showToast(String(err), "error");
  }
}

async function uploadGroupArtwork(entryKeys, kind) {
  if (!pickedGroupArtworkPath) {
    showToast("Choose an image first.", "warning");
    return;
  }
  try {
    const bytes = await invoke("read_file_bytes", { path: pickedGroupArtworkPath });
    const extension = (pickedGroupArtworkPath.split(".").pop() || "jpg").toLowerCase();
    await invoke("upload_group_artwork", { entryKeys, kind, extension, bytes });
    pickedGroupArtworkPath = null;
    clearArtworkCache();
    await refreshLibrary();
    showToast("Artwork uploaded.", "success");
  } catch (err) {
    showToast(String(err), "error");
  }
}

function wireGroupArtworkHandlers(entryKeys, kind) {
  document.getElementById("groupArtworkPickBtn")?.addEventListener("click", pickGroupArtwork);
  document.getElementById("groupArtworkUploadBtn")?.addEventListener("click", () => uploadGroupArtwork(entryKeys, kind));
}

function renderArtist(body, artist) {
  const albums = groupTracks(libraryEntries).get(artist);
  if (!albums) { browsePath = { kind: "root" }; return renderBrowse(); }
  const crumbs = [{ label: "Media", onClick: () => browsePath = { kind: "root" } }, { label: artist }];
  const cards = [...albums.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([album, tracks]) => `
    <div class="media-card" data-album="${esc(album)}">
      ${artImg(tracks[0].entry_key, "cover", "card-art")}
      <div class="card-title" title="${esc(album)}">${esc(album)}</div>
      <div class="muted" style="font-size:.75rem">${tracks.length} track${tracks.length === 1 ? "" : "s"}</div>
    </div>`).join("");
  const artistEntryKeys = [...albums.values()].flat().map(t => t.entry_key);
  body.innerHTML = `${breadcrumb(crumbs)}${groupArtworkPanel("artist photo")}<div class="media-grid">${cards}</div>`;
  wireBreadcrumb(body, crumbs);
  wireGroupArtworkHandlers(artistEntryKeys, "artist");
  body.querySelectorAll("[data-album]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "album", artist, album: el.dataset.album };
    renderBrowse();
  }));
}

function renderAlbum(body, artist, album) {
  const tracks = groupTracks(libraryEntries).get(artist)?.get(album);
  if (!tracks) { browsePath = { kind: "artist", artist }; return renderBrowse(); }
  const crumbs = [
    { label: "Media", onClick: () => browsePath = { kind: "root" } },
    { label: artist, onClick: () => browsePath = { kind: "artist", artist } },
    { label: album },
  ];
  const rows = tracks.map(t => `
    <tr>
      <td class="mono">${t.track_number ?? "—"}</td>
      <td>${esc(t.scraped_title || t.title)}</td>
      <td>${t.duration_secs ? formatDuration(t.duration_secs) : "—"}</td>
      <td><button class="secondary" data-manage="${esc(t.entry_key)}">${openManageKey === t.entry_key ? '<i class="bi bi-x-lg"></i>Close' : '<i class="bi bi-sliders"></i>Manage'}</button></td>
    </tr>
    ${openManageKey === t.entry_key ? `<tr><td colspan="4">${manageRow(t)}</td></tr>` : ""}
  `).join("");
  const albumEntryKeys = tracks.map(t => t.entry_key);
  body.innerHTML = `${breadcrumb(crumbs)}${groupArtworkPanel("album cover")}
    <table><thead><tr><th>#</th><th>Title</th><th>Duration</th><th></th></tr></thead><tbody>${rows}</tbody></table>`;
  wireBreadcrumb(body, crumbs);
  wireGroupArtworkHandlers(albumEntryKeys, "cover");
  wireTrackManageHandlers(body);
}

function wireTrackManageHandlers(container) {
  container.querySelectorAll("[data-manage]").forEach(btn => {
    btn.addEventListener("click", () => {
      openManageKey = openManageKey === btn.dataset.manage ? null : btn.dataset.manage;
      pickedArtworkPath = null;
      renderBrowse();
    });
  });
  if (openManageKey) {
    document.getElementById("editSaveBtn")?.addEventListener("click", () => saveEdit(openManageKey));
    document.getElementById("pickArtworkBtn")?.addEventListener("click", pickArtwork);
    document.getElementById("uploadArtworkBtn")?.addEventListener("click", () => uploadArtwork(openManageKey));
    document.getElementById("rescrapeBtn")?.addEventListener("click", () => rescrapeEntry(openManageKey));
    document.getElementById("revertScrapeBtn")?.addEventListener("click", () => revertScrape(openManageKey));
    wireAddCategoryBtn();
    wireMoveKindFields(openManageKey);
  }
}

function formatDuration(secs) {
  const m = Math.floor(secs / 60), s = Math.round(secs % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

// Season 0 is the real-world Plex/Kodi/TheTVDB convention for a show's
// bonus/extra content (featurettes, interviews, deleted scenes...) — a
// single show-level bucket, not tied to any one season. -1 means classify()
// genuinely found no season signal at all, a different, unrelated case.
function seasonLabel(season) {
  if (season === -1) return "Unknown Season";
  if (season === 0) return "Specials";
  return `Season ${season}`;
}

// ---- browse: shows (show → season → episodes) ------------------------------

function renderShow(body, show) {
  const seasons = groupEpisodes(libraryEntries).get(show);
  if (!seasons) { browsePath = { kind: "root" }; return renderBrowse(); }
  const crumbs = [{ label: "Media", onClick: () => browsePath = { kind: "root" } }, { label: show }];
  const cards = [...seasons.entries()].sort(([a], [b]) => a - b).map(([season, episodes]) => `
    <div class="media-card" data-season="${season}">
      ${artImg(episodes[0].entry_key, "poster", "card-art")}
      <div class="card-title">${seasonLabel(season)}</div>
      <div class="muted" style="font-size:.75rem">${episodes.length} episode${episodes.length === 1 ? "" : "s"}</div>
    </div>`).join("");
  body.innerHTML = `${breadcrumb(crumbs)}<div class="media-grid">${cards}</div>`;
  wireBreadcrumb(body, crumbs);
  body.querySelectorAll("[data-season]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "season", show, season: Number(el.dataset.season) };
    renderBrowse();
  }));
}

function renderSeason(body, show, season) {
  const episodes = groupEpisodes(libraryEntries).get(show)?.get(season);
  if (!episodes) { browsePath = { kind: "show", show }; return renderBrowse(); }
  const crumbs = [
    { label: "Media", onClick: () => browsePath = { kind: "root" } },
    { label: show, onClick: () => browsePath = { kind: "show", show } },
    { label: seasonLabel(season) },
  ];
  const cards = episodes.map(ep => `
    <div class="media-card" data-episode="${esc(ep.entry_key)}">
      ${artImg(ep.entry_key, "poster", "card-art")}
      ${likeBadge(ep.like_count)}
      <div class="card-title" title="${esc(ep.scraped_title || ep.title)}">${ep.episode ? `E${ep.episode} — ` : ""}${esc(ep.scraped_title || ep.title)}</div>
    </div>`).join("");
  body.innerHTML = `${breadcrumb(crumbs)}<div class="media-grid">${cards}</div>`;
  wireBreadcrumb(body, crumbs);
  body.querySelectorAll("[data-episode]").forEach(el => el.addEventListener("click", () => {
    browsePath = { kind: "episode-detail", key: el.dataset.episode };
    renderBrowse();
  }));
}

function renderEpisodeDetail(body, entryKey) {
  const entry = libraryEntries.find(e => e.entry_key === entryKey);
  if (!entry) { browsePath = { kind: "root" }; return renderBrowse(); }
  const show = entry.show_title || "Unknown Show";
  const season = entry.season ?? -1;
  const crumbs = [
    { label: "Media", onClick: () => browsePath = { kind: "root" } },
    { label: show, onClick: () => browsePath = { kind: "show", show } },
    { label: season === -1 ? "Unknown Season" : `Season ${season}`, onClick: () => browsePath = { kind: "season", show, season } },
    { label: entry.scraped_title || entry.title },
  ];
  body.innerHTML = detailView(entry, crumbs);
  wireBreadcrumb(body, crumbs);
  wireDetailManage(entry);
  body.querySelectorAll("[data-filter-category]").forEach(el => el.addEventListener("click", () => {
    categoryFilter = el.dataset.filterCategory;
    browsePath = { kind: "root" };
    renderMediaTab();
  }));
}

function renderLibrary() {
  const library = document.getElementById("mediaSectionBody");
  if (!libraryEntries.length) {
    library.innerHTML = `<span class="muted">No media found under the configured media roots.</span>`;
    return;
  }
  const entries = filteredEntries();
  if (!entries.length) {
    library.innerHTML = `<span class="muted">No matches for the current search/filter.</span>`;
    return;
  }
  library.innerHTML = `<div class="table-scroll"><table>
    <thead><tr><th>Title</th><th>Kind</th><th>Genres</th><th>Art</th><th>Path</th><th>Size</th><th></th></tr></thead>
    <tbody>` + entries.map(e => `
      <tr data-entry-row="${esc(e.entry_key)}">
        <td>${esc(e.scraped_title || e.title)}</td>
        <td>${esc(e.kind)}</td>
        <td>${e.genres.map(esc).join(", ") || "—"}</td>
        <td>${e.has_artwork ? "✓" : "—"}</td>
        <td class="mono" title="${esc(e.relative_path)}">${esc(e.relative_path)}</td>
        <td>${(e.size / 1048576).toFixed(1)} MB</td>
        <td><button class="secondary" data-manage="${esc(e.entry_key)}">${openManageKey === e.entry_key ? '<i class="bi bi-x-lg"></i>Close' : '<i class="bi bi-sliders"></i>Manage'}</button></td>
      </tr>
      ${openManageKey === e.entry_key ? `<tr><td colspan="7">${manageRow(e)}</td></tr>` : ""}
    `).join("") + `</tbody></table></div>`;

  library.querySelectorAll("[data-manage]").forEach(btn => {
    btn.addEventListener("click", () => {
      openManageKey = openManageKey === btn.dataset.manage ? null : btn.dataset.manage;
      pickedArtworkPath = null;
      renderLibrary();
    });
  });
  if (openManageKey) {
    document.getElementById("editSaveBtn")?.addEventListener("click", () => saveEdit(openManageKey));
    document.getElementById("pickArtworkBtn")?.addEventListener("click", pickArtwork);
    document.getElementById("uploadArtworkBtn")?.addEventListener("click", () => uploadArtwork(openManageKey));
    document.getElementById("rescrapeBtn")?.addEventListener("click", () => rescrapeEntry(openManageKey));
    document.getElementById("revertScrapeBtn")?.addEventListener("click", () => revertScrape(openManageKey));
    wireAddCategoryBtn();
    wireMoveKindFields(openManageKey);
  }
}

function manageRow(entry) {
  // Every category already in use anywhere in the library, plus any this
  // entry already carries that aren't in that list yet (can happen right
  // after a fresh scrape adds a brand new genre value, before this panel's
  // own allCategories cache has been refreshed) — so the picker never
  // silently hides a category this entry is actually tagged with.
  const pickerCategories = [...new Set([...allCategories, ...entry.genres])].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  return `
    <div class="inline-edit">
      <h2 style="margin-top:0">Edit metadata</h2>
      <label>Title</label>
      <input id="editTitleInput" value="${esc(entry.scraped_title || entry.title)}">
      <label>Description</label>
      <textarea id="editOverviewInput" rows="4" style="width:100%; background:var(--surface-muted); color:var(--text); border:1px solid var(--border); border-radius:8px; padding:8px 10px; font-family:inherit; resize:vertical">${esc(entry.overview || "")}</textarea>
      ${entry.kind !== "track" ? `
      <label>Content rating</label>
      <input id="editRatingInput" placeholder="${entry.kind === "episode" ? "e.g. TV-14" : "e.g. PG-13"}" value="${esc(entry.rating || "")}">
      ` : ""}
      <label>Categories</label>
      <div class="category-picker">
        ${pickerCategories.length ? pickerCategories.map(c => `
          <label class="checkbox-label category-picker-item">
            <input type="checkbox" class="editCategoryCheck" value="${esc(c)}"${entry.genres.includes(c) ? " checked" : ""}>
            ${esc(c)}
          </label>`).join("") : `<span class="muted category-picker-empty" style="font-size:.85rem">No categories yet — add one below.</span>`}
      </div>
      <div class="row" style="margin-top:8px">
        <input id="editNewCategoryInput" placeholder="New category name">
        <button id="editAddCategoryBtn" class="secondary" type="button"><i class="bi bi-plus-lg"></i>Add</button>
      </div>
      <div class="row" style="margin-top:10px">
        <button id="editSaveBtn"><i class="bi bi-check-lg"></i>Save metadata</button>
      </div>

      <h2>Upload artwork</h2>
      <div class="row">
        <label style="flex:0 0 100%">Kind</label>
        <select id="artworkKindSelect" style="flex:1; background:var(--surface-muted); color:var(--text); border:1px solid var(--border); border-radius:8px; padding:8px 10px">
          <option value="poster">Poster</option>
          <option value="backdrop">Backdrop</option>
          <option value="cover">Cover</option>
          <option value="artist">Artist photo</option>
        </select>
        <button id="pickArtworkBtn" class="secondary"><i class="bi bi-image"></i>Choose image…</button>
        <button id="uploadArtworkBtn"><i class="bi bi-upload"></i>Upload</button>
      </div>
      <p class="muted" id="artworkPickedNote">${pickedArtworkPath ? esc(pickedArtworkPath) : "No file chosen."}</p>

      <h2>Rescrape</h2>
      <label>TMDb URL override (optional — leave blank to search normally)</label>
      <input id="rescrapeUrlInput" placeholder="https://www.themoviedb.org/movie/27205-inception">
      <div class="row" style="margin-top:10px">
        <button id="rescrapeBtn" class="secondary"><i class="bi bi-arrow-repeat"></i>Rescrape this entry</button>
        ${entry.scraped_title || entry.genres.length || entry.has_artwork
          ? `<button id="revertScrapeBtn" class="danger"><i class="bi bi-arrow-counterclockwise"></i>Revert to unscraped</button>`
          : ""}
      </div>

      <h2>Asset type</h2>
      <p class="muted" style="margin-top:-6px">For a file that ended up in the wrong section — most often a music video stored as a movie/show. Moving it here sticks: it's remembered across rescans and "Fix classifications".</p>
      <select id="moveKindSelect" style="width:100%; background:var(--surface-muted); color:var(--text); border:1px solid var(--border); border-radius:8px; padding:8px 10px">
        <option value="movie"${entry.kind === "movie" ? " selected" : ""}>Movie</option>
        <option value="episode"${entry.kind === "episode" ? " selected" : ""}>TV show episode</option>
        <option value="track"${entry.kind === "track" ? " selected" : ""}>Music track</option>
      </select>
      <div id="moveTrackFields" class="row" style="margin-top:8px; display:${entry.kind === "track" ? "flex" : "none"}">
        <input id="moveArtistInput" placeholder="Artist" value="${entry.kind === "track" ? esc(entry.artist || "") : ""}">
        <input id="moveAlbumInput" placeholder="Album" value="${entry.kind === "track" ? esc(entry.album || "") : ""}">
      </div>
      <div id="moveEpisodeFields" class="row" style="margin-top:8px; display:${entry.kind === "episode" ? "flex" : "none"}">
        <input id="moveShowInput" placeholder="Show name" value="${entry.kind === "episode" ? esc(entry.show_title || "") : ""}">
      </div>
      <div class="row" style="margin-top:10px">
        <button id="moveKindBtn" class="secondary"><i class="bi bi-arrow-left-right"></i>Move to this type</button>
      </div>

      <h2>File</h2>
      <p class="mono muted" style="font-size:.78rem" title="${esc(entry.relative_path)}">${esc(entry.relative_path)}</p>
    </div>`;
}

async function revertScrape(entryKey) {
  try {
    await invoke("clear_scraped_metadata", { entryKey });
    clearArtworkCache();
    await refreshLibrary();
    showToast("Reverted to unscraped.", "success");
  } catch (err) {
    showToast(String(err), "error");
  }
}

async function saveEdit(entryKey) {
  try {
    const title = document.getElementById("editTitleInput").value.trim();
    const overview = document.getElementById("editOverviewInput").value.trim();
    const rating = document.getElementById("editRatingInput")?.value.trim() ?? null;
    const genres = [...document.querySelectorAll(".editCategoryCheck:checked")].map(cb => cb.value);
    await invoke("set_manual_metadata", { entryKey, title, genres, overview, rating });
    await refreshLibrary();
    showToast("Metadata saved.", "success");
  } catch (err) {
    showToast(String(err), "error");
  }
}

function wireMoveKindFields(entryKey) {
  const select = document.getElementById("moveKindSelect");
  if (!select) return;
  const trackFields = document.getElementById("moveTrackFields");
  const episodeFields = document.getElementById("moveEpisodeFields");
  const sync = () => {
    trackFields.style.display = select.value === "track" ? "flex" : "none";
    episodeFields.style.display = select.value === "episode" ? "flex" : "none";
  };
  select.addEventListener("change", sync);
  document.getElementById("moveKindBtn")?.addEventListener("click", () => moveKind(entryKey));
}

async function moveKind(entryKey) {
  try {
    const kind = document.getElementById("moveKindSelect").value;
    const artist = document.getElementById("moveArtistInput").value.trim();
    const album = document.getElementById("moveAlbumInput").value.trim();
    const showTitle = document.getElementById("moveShowInput").value.trim();
    await invoke("set_manual_kind", { entryKey, kind, artist: artist || null, album: album || null, showTitle: showTitle || null });
    await refreshLibrary();
    showToast(`Moved to ${kind === "track" ? "Music" : kind === "episode" ? "Shows" : "Movies"}.`, "success");
  } catch (err) {
    showToast(String(err), "error");
  }
}

// Lets a category be created right from the picker instead of needing a
// separate "manage categories" flow — typing a brand new name and checking
// it here is the entire "create a category" action; it only actually starts
// existing once this entry's edit is saved (see saveEdit), same as every
// other category, matching the "categories = genres, no separate registry"
// design (Library::distinct_genres' doc comment).
function wireAddCategoryBtn() {
  document.getElementById("editAddCategoryBtn")?.addEventListener("click", () => {
    const input = document.getElementById("editNewCategoryInput");
    const name = input.value.trim();
    if (!name) return;
    const picker = document.querySelector(".category-picker");
    const existing = [...picker.querySelectorAll(".editCategoryCheck")].find(cb => cb.value.toLowerCase() === name.toLowerCase());
    if (existing) {
      existing.checked = true;
    } else {
      picker.querySelector(".category-picker-empty")?.remove();
      const label = document.createElement("label");
      label.className = "checkbox-label category-picker-item";
      label.innerHTML = `<input type="checkbox" class="editCategoryCheck" value="${esc(name)}" checked> ${esc(name)}`;
      picker.appendChild(label);
    }
    input.value = "";
    input.focus();
  });
}

async function pickArtwork() {
  try {
    pickedArtworkPath = await invoke("pick_file_path");
    document.getElementById("artworkPickedNote").textContent = pickedArtworkPath || "No file chosen.";
  } catch (err) {
    showToast(String(err), "error");
  }
}

async function uploadArtwork(entryKey) {
  if (!pickedArtworkPath) {
    showToast("Choose an image first.", "warning");
    return;
  }
  try {
    const bytes = await invoke("read_file_bytes", { path: pickedArtworkPath });
    const extension = (pickedArtworkPath.split(".").pop() || "jpg").toLowerCase();
    const kind = document.getElementById("artworkKindSelect").value;
    await invoke("upload_artwork", { entryKey, kind, extension, bytes });
    pickedArtworkPath = null;
    clearArtworkCache();
    await refreshLibrary();
    showToast("Artwork uploaded.", "success");
  } catch (err) {
    showToast(String(err), "error");
  }
}

async function rescrapeEntry(entryKey) {
  const tmdbUrl = document.getElementById("rescrapeUrlInput").value.trim();
  const btn = document.getElementById("rescrapeBtn");
  if (btn) btn.disabled = true;
  try {
    await invoke("rescrape_entry", { entryKey, tmdbUrl: tmdbUrl || null });
    clearArtworkCache();
    await refreshLibrary();
    showToast("Rescraped.", "success");
  } catch (err) {
    showToast(String(err), "error");
  } finally {
    if (btn) btn.disabled = false;
  }
}

// Real bug this fixes: a rescan over a slow network mount (SMB/NFS) can
// legitimately take minutes — confirmed live against a real ~3,700-file
// remote share — and gave no indication anything was happening until it
// finished. Mirrors the Scrape button's own `scrape-progress` bar exactly
// (`scan-progress` events from the Rust side, one per discovered file).
document.getElementById("rescanBtn").addEventListener("click", async () => {
  const progressBox = document.getElementById("scanProgress");
  const progressFill = document.getElementById("scanProgressFill");
  const progressText = document.getElementById("scanProgressText");
  const rescanBtn = document.getElementById("rescanBtn");
  progressFill.style.width = "0%";
  progressText.textContent = "Starting…";
  progressBox.classList.remove("d-none");
  rescanBtn.disabled = true;

  const unlisten = await listen("scan-progress", ({ payload }) => {
    if (payload.phase === "discovering") {
      progressFill.style.width = "0%";
      progressText.textContent = `Finding files… ${payload.found} found so far`;
    } else {
      progressFill.style.width = `${Math.round((payload.processed / payload.total) * 100)}%`;
      progressText.textContent = `Scanning ${payload.processed} of ${payload.total} files…`;
    }
  });
  try {
    const r = await invoke("rescan");
    showToast(`+${r.added} added, ${r.updated} updated, ${r.removed} removed`, "success");
    await refreshLibrary();
  } catch (err) {
    showToast(String(err), "error");
  } finally {
    unlisten();
    progressBox.classList.add("d-none");
    rescanBtn.disabled = false;
  }
});

document.getElementById("reclassifyBtn").addEventListener("click", async () => {
  try {
    const r = await invoke("reclassify_library");
    showToast(
      `${r.changed} corrected, ${r.unchanged} already correct` +
      (r.changed ? " — run Scrape metadata again to re-match the corrected entries." : ""),
      "success",
    );
    await refreshLibrary();
  } catch (err) {
    showToast(String(err), "error");
  }
});

function renderScrapeIssues(issues) {
  const wrap = document.getElementById("scrapeIssuesWrap");
  const list = document.getElementById("scrapeIssues");
  if (!issues || issues.length === 0) {
    wrap.classList.add("d-none");
    list.innerHTML = "";
    return;
  }
  wrap.classList.remove("d-none");
  document.getElementById("scrapeIssuesCount").textContent =
    `${issues.length} issue${issues.length === 1 ? "" : "s"} from the last scrape`;
  list.innerHTML = issues
    .map(i => `<li><span class="issue-title">${esc(i.title)}</span> — <span class="issue-reason">${esc(i.reason)}</span></li>`)
    .join("");
}

document.getElementById("dismissScrapeIssuesBtn").addEventListener("click", () => renderScrapeIssues(null));

// Applies one `scrape-progress` event immediately, without waiting for the
// scrape to finish or doing a full `refreshLibrary()` re-fetch — this is
// what makes a match visibly "land" the moment it's pulled and written,
// instead of every card appearing to freeze until the whole run completes
// and then all re-render/re-fetch-artwork at once.
function patchEntryLive(p) {
  const entry = libraryEntries.find(e => e.entry_key === p.entry_key);
  if (entry) {
    if (p.scraped_title) entry.scraped_title = p.scraped_title;
    if (p.genres && p.genres.length) entry.genres = p.genres;
    if (p.cast && p.cast.length) entry.cast = p.cast;
    if (p.outcome === "matched") entry.has_artwork = true;
  }
  const title = (p.scraped_title || entry?.title || p.title) + (entry?.year ? ` (${entry.year})` : "");

  // Flat "All entries" table row, if rendered.
  const row = document.querySelector(`tr[data-entry-row="${p.entry_key}"]`);
  if (row) {
    const cells = row.querySelectorAll("td");
    if (cells[0]) cells[0].textContent = p.scraped_title || entry?.title || p.title;
    if (cells[2] && p.genres && p.genres.length) cells[2].textContent = p.genres.join(", ");
    if (cells[3] && p.outcome === "matched") cells[3].textContent = "✓";
  }

  // Browse grid's movie card, if rendered.
  const movieCard = document.querySelector(`[data-movie="${p.entry_key}"] .card-title`);
  if (movieCard) movieCard.textContent = title;

  // Any rendered artwork placeholder for this entry, in whichever view is
  // currently open (root grid, detail view, album cover, etc.) — re-trigger
  // the load now that the bytes may actually exist on disk.
  if (p.outcome === "matched") {
    // Fresh bytes just landed on disk — drop any cached copy from before
    // this match so the reload below can't serve a stale (or empty-
    // placeholder-miss) cache entry instead of the real artwork.
    for (const kind of ["poster", "backdrop", "cover"]) {
      const cached = artworkCache.get(`${p.entry_key}:${kind}`);
      if (cached) { URL.revokeObjectURL(cached.blobUrl); artworkCache.delete(`${p.entry_key}:${kind}`); }
    }
    document.querySelectorAll(
      `img[id^="art-poster-${p.entry_key}-"], img[id^="art-backdrop-${p.entry_key}-"], img[id^="art-cover-${p.entry_key}-"]`,
    ).forEach(img => {
      const kind = img.id.startsWith("art-poster-") ? "poster" : img.id.startsWith("art-backdrop-") ? "backdrop" : "cover";
      artworkObserver.unobserve(img);
      loadArtworkInto(img, p.entry_key, kind);
    });
  }
}

document.getElementById("scrapeBtn").addEventListener("click", async () => {
  const progressBox = document.getElementById("scrapeProgress");
  const progressFill = document.getElementById("scrapeProgressFill");
  const progressText = document.getElementById("scrapeProgressText");
  renderScrapeIssues(null);
  progressFill.style.width = "0%";
  progressText.textContent = "Starting…";
  progressBox.classList.remove("d-none");

  const unlisten = await listen("scrape-progress", ({ payload }) => {
    progressFill.style.width = `${Math.round((payload.processed / payload.total) * 100)}%`;
    progressText.textContent = `Scraping ${payload.processed} of ${payload.total} — ${payload.scraped_title || payload.title}`;
    patchEntryLive(payload);
  });
  try {
    const force = document.getElementById("forceScrapeCheck").checked;
    const r = await invoke("run_scrape", { force });
    showToast(
      `matched ${r.matched}, not found ${r.not_found}, failed ${r.failed}, skipped ${r.skipped}`,
      r.failed > 0 ? "warning" : "success",
    );
    renderScrapeIssues(r.issues);
    // A final full refresh as a safety net (picks up anything not visible
    // in the current view during the run) — most of what it would show is
    // already on screen from live patching by this point, so it no longer
    // reads as content suddenly appearing out of nowhere.
    await refreshLibrary();
  } catch (err) {
    showToast(String(err), "error");
  } finally {
    unlisten();
    progressBox.classList.add("d-none");
  }
});
