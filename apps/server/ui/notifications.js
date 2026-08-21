// ---- Notifications tab: client-reported errors ------------------------------
//
// Clients (currently the Fire TV app) call `/errors/report` over their
// existing authenticated peer connection whenever something fails on their
// end — playback prep, an unreachable server, etc. — so it lands somewhere
// a human will actually see it instead of only ever existing in on-device
// logs. This panel is that somewhere: newest first, delete once triaged.
// Previously lived on the Swarm tab (client errors are reported over the
// same peer connection swarm membership uses); split out to its own tab
// once the badge count grew into its own "things to look at" surface
// distinct from swarm membership/roster management.
//
// refreshNotificationBadge() itself lives in app.js, not here — see that
// file's comment for why (real bug, found live: this file loads after
// app.js, so a call to a same-named function *defined* here but invoked
// from app.js's own boot path could run before this file had finished
// loading/executing).

async function refreshNotifications() {
  await loadClientErrors();
}

async function loadClientErrors() {
  const list = document.getElementById("clientErrorsList");
  const countEl = document.getElementById("clientErrorsCount");
  const clearBtn = document.getElementById("clearClientErrorsBtn");
  try {
    const errors = await invoke("list_client_errors");
    countEl.textContent = errors.length ? `${errors.length} error${errors.length === 1 ? "" : "s"}` : "";
    clearBtn.classList.toggle("d-none", errors.length === 0);
    list.innerHTML = errors.length ? errors.map(e => `
      <div class="client-error-row">
        <div style="flex:1; min-width:0">
          <div class="client-error-message">${esc(e.message)}</div>
          <div class="client-error-meta">
            <span><i class="bi bi-clock"></i> ${esc(new Date(e.occurred_at_ms).toLocaleString())}</span>
            <span><i class="bi bi-tv"></i> ${esc(e.device_name)}</span>
            ${e.asset_title ? `<span><i class="bi bi-film"></i> ${esc(e.asset_title)}</span>` : ""}
            ${e.kind ? `<span>${esc(e.kind)}</span>` : ""}
          </div>
          ${e.context ? `<div class="client-error-context">${esc(e.context)}</div>` : ""}
        </div>
        <button class="danger" data-delete-error="${e.id}" style="padding:5px 10px; font-size:.75rem" title="Delete"><i class="bi bi-x-lg"></i></button>
      </div>`).join("") : `<p class="muted">No client errors reported.</p>`;
    list.querySelectorAll("[data-delete-error]").forEach(btn => {
      btn.addEventListener("click", async () => {
        try {
          await invoke("delete_client_error", { id: Number(btn.dataset.deleteError) });
          await loadClientErrors();
        } catch (err) {
          showToast(String(err), "error");
        }
      });
    });
  } catch (err) {
    list.innerHTML = `<p class="muted">Unable to load client errors.</p>`;
    showToast(String(err), "error");
  }
  await refreshNotificationBadge();
}

document.getElementById("clearClientErrorsBtn").addEventListener("click", async () => {
  try {
    await invoke("clear_client_errors");
    showToast("Cleared client errors.", "success");
    await loadClientErrors();
  } catch (err) {
    showToast(String(err), "error");
  }
});
