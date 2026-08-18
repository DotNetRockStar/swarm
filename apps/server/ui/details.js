// ---- Details tab: status + media-root configuration ------------------------

async function refreshDetails() {
  await Promise.all([refreshStatus(), refreshMediaRoots()]);
}

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
        try {
          await invoke("remove_media_root", { label: btn.dataset.removeRoot });
          await refreshMediaRoots();
        } catch (err) {
          errorEl.textContent = String(err);
        }
      });
    });
  } catch (err) {
    list.innerHTML = `<p class="error">${esc(err)}</p>`;
  }
}

document.getElementById("addRootBtn").addEventListener("click", async () => {
  const errorEl = document.getElementById("mediaRootsError");
  errorEl.textContent = "";
  const label = document.getElementById("addRootLabel");
  const path = document.getElementById("addRootPath");
  try {
    await invoke("add_media_root", { label: label.value, path: path.value });
    label.value = "";
    path.value = "";
    await refreshMediaRoots();
  } catch (err) {
    errorEl.textContent = String(err);
  }
});

document.getElementById("chooseAddRootBtn").addEventListener("click", async () => {
  const picked = await invoke("pick_folder_path").catch(() => null);
  if (picked) document.getElementById("addRootPath").value = picked;
});
