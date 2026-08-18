// ---- Details tab: status + media-root configuration ------------------------

async function refreshDetails() {
  await Promise.all([refreshStatus(), refreshMediaRoots(), refreshTmdbKeyField()]);
}

async function refreshTmdbKeyField() {
  const settings = await invoke("get_settings");
  const status = document.getElementById("tmdbKeyStatus");
  status.textContent = settings.has_tmdb_key ? "A key is saved. Scraping is enabled." : "No key saved yet — scraping is disabled until one is added.";
  status.classList.toggle("error", !settings.has_tmdb_key);
}

document.getElementById("saveTmdbKeyBtn").addEventListener("click", async () => {
  const input = document.getElementById("tmdbKeyInput");
  const status = document.getElementById("tmdbKeyStatus");
  try {
    await invoke("set_tmdb_api_key", { key: input.value });
    input.value = "";
    await refreshTmdbKeyField();
  } catch (err) {
    status.classList.add("error");
    status.textContent = String(err);
  }
});

async function refreshStatus() {
  const grid = document.getElementById("statusGrid");
  try {
    const status = await invoke("get_status");
    const totalGb = (await invoke("list_entries").catch(() => []))
      .reduce((sum, e) => sum + e.size, 0) / 1073741824;
    grid.innerHTML =
      stat("Entries", status.entry_count) +
      stat("Library size", totalGb.toFixed(2) + " GB") +
      stat("Listening (QUIC)", status.listen_addr) +
      stat("Streaming upload budget", (status.streaming_upload_budget_bps / 1000000).toFixed(1) + " Mbps") +
      stat("Active playback sessions", status.active_playback_sessions) +
      stat("Media roots", status.media_roots.join(", ")) +
      stat("Device fingerprint", status.fingerprint, true) +
      stat("Library thumbprint", status.thumbprint.slice(0, 24) + "…", true);
  } catch (err) {
    grid.innerHTML = `<div class="stat"><div class="label error">Error</div><div class="value">${esc(err)}</div></div>`;
  }
}

async function refreshMediaRoots() {
  const list = document.getElementById("mediaRootsList");
  try {
    const roots = await invoke("list_media_roots");
    list.innerHTML = roots.map(r => `
      <div class="row" style="align-items:center">
        <div class="mono muted" style="flex:2">${esc(r.label)}: ${esc(r.path)}</div>
        <button class="danger" data-remove-root="${esc(r.label)}" ${roots.length <= 1 ? "disabled" : ""}>Remove</button>
      </div>`).join("");
    list.querySelectorAll("[data-remove-root]").forEach(btn => {
      btn.addEventListener("click", async () => {
        const errorEl = document.getElementById("mediaRootsError");
        errorEl.classList.add("error");
        errorEl.textContent = "";
        try {
          const result = await invoke("remove_media_root", { label: btn.dataset.removeRoot });
          await refreshMediaRoots();
          describeRootChange(errorEl, result);
        } catch (err) {
          errorEl.classList.add("error");
          errorEl.textContent = String(err);
        }
      });
    });
  } catch (err) {
    list.innerHTML = `<p class="error">${esc(err)}</p>`;
  }
}

// Shows how a live add/remove actually landed — a rescan report if a core
// was already running (this change took effect immediately, no restart), or
// nothing if it's still first-run onboarding (there's no core yet to apply
// it to; the choice is just saved for when one starts).
function describeRootChange(errorEl, result) {
  if (!result.rescan) return;
  const { added, updated, removed, unchanged } = result.rescan;
  errorEl.classList.remove("error");
  errorEl.textContent = `Applied — scanned now: +${added} added, ${updated} updated, ${removed} removed, ${unchanged} unchanged.`;
}

document.getElementById("addRootBtn").addEventListener("click", async () => {
  const errorEl = document.getElementById("mediaRootsError");
  errorEl.classList.add("error");
  errorEl.textContent = "";
  const label = document.getElementById("addRootLabel");
  const path = document.getElementById("addRootPath");
  try {
    const result = await invoke("add_media_root", { label: label.value, path: path.value });
    label.value = "";
    path.value = "";
    await refreshMediaRoots();
    describeRootChange(errorEl, result);
  } catch (err) {
    errorEl.classList.add("error");
    errorEl.textContent = String(err);
  }
});

document.getElementById("chooseAddRootBtn").addEventListener("click", async () => {
  const picked = await invoke("pick_folder_path").catch(() => null);
  if (picked) document.getElementById("addRootPath").value = picked;
});
