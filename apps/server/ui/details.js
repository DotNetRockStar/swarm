// ---- Details tab: status + media-root configuration ------------------------

async function refreshDetails() {
  await Promise.all([refreshStatus(), refreshMediaRoots(), refreshTmdbKeyField(), refreshTranscriptionSetting()]);
}

async function refreshTmdbKeyField() {
  const settings = await invoke("get_settings");
  document.getElementById("uploadBudgetEnabledCheck").checked = settings.streaming_upload_budget_enabled;
  const status = document.getElementById("tmdbKeyStatus");
  status.textContent = settings.has_tmdb_key ? "A key is saved. Scraping is enabled." : "No key saved yet — scraping is disabled until one is added.";
  status.classList.toggle("error", !settings.has_tmdb_key);
}

async function refreshTranscriptionSetting() {
  const settings = await invoke("get_settings");
  document.getElementById("localTranscriptionEnabledCheck").checked = settings.local_transcription_enabled;
  document.getElementById("transcriptionPauseWhileStreamingCheck").checked = settings.transcription_pause_while_streaming;
  const statusEl = document.getElementById("localTranscriptionSettingStatus");
  try {
    const status = await invoke("get_transcription_status");
    if (!settings.local_transcription_enabled) {
      statusEl.textContent = status.model_installed
        ? "Paused. The installed model and completed work are preserved."
        : "Off. The ~466 MB Whisper model will download automatically the first time this is enabled.";
    } else {
      statusEl.textContent = status.message;
    }
  } catch (err) {
    statusEl.textContent = "Unable to load subtitle-generation status.";
  }
}

document.getElementById("localTranscriptionEnabledCheck").addEventListener("change", async (event) => {
  const enabled = event.currentTarget.checked;
  try {
    await invoke("set_local_transcription_enabled", { enabled });
    if (enabled) {
      openInfoModal("local-subtitles");
      showToast("Local subtitles enabled. SWARM will download the Whisper model automatically if needed.", "success", { duration: 7000 });
    } else {
      showToast("Local subtitle generation paused. Progress has been saved.", "success");
    }
    await Promise.all([refreshTranscriptionSetting(), refreshTranscriptionProgress()]);
  } catch (err) {
    event.currentTarget.checked = !enabled;
    showToast(String(err), "error");
  }
});

document.getElementById("transcriptionPauseWhileStreamingCheck").addEventListener("change", async (event) => {
  const enabled = event.currentTarget.checked;
  try {
    await invoke("set_transcription_pause_while_streaming", { enabled });
    showToast(
      enabled
        ? "Subtitle generation will pause while clients are streaming."
        : "Subtitle generation may now use CPU while clients are streaming.",
      "success",
    );
    await refreshTranscriptionSetting();
  } catch (err) {
    event.currentTarget.checked = !enabled;
    showToast(String(err), "error");
  }
});

document.getElementById("uploadBudgetEnabledCheck").addEventListener("change", async (event) => {
  const enabled = event.currentTarget.checked;
  try {
    await invoke("set_streaming_upload_budget_enabled", { enabled });
    await refreshStatus();
    showToast(enabled ? "Internet streaming budget enabled." : "Internet streaming budget disabled.", "success");
  } catch (err) {
    event.currentTarget.checked = !enabled;
    showToast(String(err), "error");
  }
});

document.getElementById("saveTmdbKeyBtn").addEventListener("click", async () => {
  const input = document.getElementById("tmdbKeyInput");
  try {
    await invoke("set_tmdb_api_key", { key: input.value });
    input.value = "";
    await refreshTmdbKeyField();
    showToast("TMDb key saved.", "success");
  } catch (err) {
    showToast(String(err), "error");
  }
});

async function refreshStatus() {
  const grid = document.getElementById("statusGrid");
  try {
    const status = await invoke("get_status");
    const totalGb = (await invoke("list_entries").catch(() => []))
      .reduce((sum, e) => sum + e.size, 0) / 1073741824;
    // "Media roots" used to be a stat tile here too, joining every root
    // path into one comma-separated string — the exact "word wrapped
    // panel" complaint real use surfaced, since a tile sized for a short
    // label/value pair isn't a reasonable place for one or more full
    // filesystem paths. It's dropped: the "Media roots" card right below
    // this one already lists every root in full, with room to breathe.
    grid.innerHTML =
      stat("Entries", status.entry_count, false, "entries") +
      stat("Library size", totalGb.toFixed(2) + " GB", false, "library-size") +
      stat("Streaming upload budget", status.streaming_upload_budget_enabled ? (status.streaming_upload_budget_bps / 1000000).toFixed(1) + " Mbps" : "Unlimited", false, "upload-budget") +
      stat("Active playback sessions", status.active_playback_sessions, false, "active-sessions") +
      stat("Device fingerprint", status.fingerprint, true, "device-fingerprint") +
      stat("Library thumbprint", status.thumbprint.slice(0, 24) + "…", true, "library-thumbprint");
  } catch (err) {
    grid.innerHTML = `<p class="muted">Unable to load status.</p>`;
    showToast(String(err), "error");
  }
}

async function refreshMediaRoots() {
  const list = document.getElementById("mediaRootsList");
  try {
    const roots = await invoke("list_media_roots");
    list.innerHTML = roots.map(r => `
      <div class="media-root-row">
        <div class="media-root-info">
          <div class="media-root-label">${esc(r.label)}</div>
          <div class="mono muted media-root-path">${esc(r.path)}</div>
        </div>
        <button class="danger" data-remove-root="${esc(r.label)}" ${roots.length <= 1 ? "disabled" : ""}><i class="bi bi-trash"></i>Remove</button>
      </div>`).join("");
    list.querySelectorAll("[data-remove-root]").forEach(btn => {
      btn.addEventListener("click", async () => {
        try {
          const result = await invoke("remove_media_root", { label: btn.dataset.removeRoot });
          await refreshMediaRoots();
          describeRootChange(result);
        } catch (err) {
          showToast(String(err), "error");
        }
      });
    });
  } catch (err) {
    list.innerHTML = `<p class="muted">Unable to load media roots.</p>`;
    showToast(String(err), "error");
  }
}

// Shows how a live add/remove actually landed — a rescan report if a core
// was already running (this change took effect immediately, no restart), or
// nothing if it's still first-run onboarding (there's no core yet to apply
// it to; the choice is just saved for when one starts).
function describeRootChange(result) {
  if (!result.rescan) return;
  const { added, updated, removed, unchanged } = result.rescan;
  showToast(`Applied — scanned now: +${added} added, ${updated} updated, ${removed} removed, ${unchanged} unchanged.`, "success");
}

document.getElementById("addRootBtn").addEventListener("click", async () => {
  const label = document.getElementById("addRootLabel");
  const path = document.getElementById("addRootPath");
  try {
    const result = await invoke("add_media_root", { label: label.value, path: path.value });
    label.value = "";
    path.value = "";
    await refreshMediaRoots();
    describeRootChange(result);
  } catch (err) {
    showToast(String(err), "error");
  }
});

document.getElementById("chooseAddRootBtn").addEventListener("click", async () => {
  const picked = await invoke("pick_folder_path").catch(() => null);
  if (picked) document.getElementById("addRootPath").value = picked;
});
