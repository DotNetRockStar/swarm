// ---- Swarm tab: STUN link, membership, per-swarm device roster -------------
//
// Join-code *generation* is intentionally not here: the STUN server only
// lets the swarm's owning user (a session-cookie browser login) mint codes
// (`POST /swarms/{id}/codes` has no Bearer path), and this device only ever
// holds a device Bearer token — it was never designed to also hold a user
// login. Codes are generated from the STUN server's own admin page; this tab
// links there for convenience instead of duplicating that flow.

// ---- Swarm tab: client-reported errors --------------------------------------
//
// Clients (currently the Fire TV app) call `/errors/report` over their
// existing authenticated peer connection whenever something fails on their
// end — playback prep, an unreachable server, etc. — so it lands somewhere
// a human will actually see it instead of only ever existing in on-device
// logs. This panel is that somewhere: newest first, delete once triaged.
//
// refreshErrorBadge() itself lives in app.js, not here — see that file's
// comment for why (real bug, found live: this file loads after app.js, so a
// call to a same-named function *defined* here but invoked from app.js's own
// boot path could run before this file had finished loading/executing).

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
  await refreshErrorBadge();
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

function formatFingerprint(fingerprint) {
  return `${fingerprint.slice(0, 12)}…${fingerprint.slice(-8)}`;
}

async function loadLocalPeers() {
  const list = document.getElementById("localPeersList");
  try {
    const peers = await invoke("list_local_peers");
    list.innerHTML = peers.length ? `<table>
      <thead><tr><th>Name</th><th>Paired</th><th>Certificate</th><th></th></tr></thead>
      <tbody>${peers.map(peer => `<tr>
        <td>${esc(peer.name)}</td>
        <td>${esc(new Date(peer.paired_at * 1000).toLocaleString())}</td>
        <td class="mono" title="${esc(peer.fingerprint)}">${esc(formatFingerprint(peer.fingerprint))}</td>
        <td><button class="danger" data-revoke-local="${esc(peer.fingerprint)}"><i class="bi bi-x-lg"></i>Revoke</button></td>
      </tr>`).join("")}</tbody>
    </table>` : `<p class="muted">No LAN clients have been paired yet.</p>`;
    list.querySelectorAll("[data-revoke-local]").forEach(btn => {
      btn.addEventListener("click", async () => {
        try {
          await invoke("revoke_local_peer", { fingerprint: btn.dataset.revokeLocal });
          showToast("LAN client revoked.", "success");
          await loadLocalPeers();
        } catch (err) {
          showToast(String(err), "error");
        }
      });
    });
  } catch (err) {
    list.innerHTML = `<p class="muted">Unable to load paired LAN clients.</p>`;
    showToast(String(err), "error");
  }
}

document.getElementById("openLanPairingBtn").addEventListener("click", async () => {
  const status = document.getElementById("lanPairingStatus");
  try {
    const pairing = await invoke("open_lan_pairing");
    status.classList.remove("d-none");
    status.innerHTML = `<div class="note">
      Enter this code on the client within five minutes:
      <strong class="mono" style="font-size:1.4rem; margin-left:8px">${esc(pairing.code)}</strong>
    </div>`;
  } catch (err) {
    showToast(String(err), "error");
  }
});

async function refreshSwarm() {
  loadClientErrors();
  loadLocalPeers();
  const content = document.getElementById("swarmContent");
  let link;
  try {
    link = await invoke("get_swarm_link");
  } catch (err) {
    content.innerHTML = `<p class="muted">Unable to load swarm status.</p>`;
    showToast(String(err), "error");
    return;
  }

  if (!link) {
    content.innerHTML = `
      <p class="muted">Not linked to a STUN server yet.</p>
      <div class="row">
        <input id="joinBaseUrl" placeholder="https://swarm.example.com">
        <input id="joinCode" placeholder="12345678" maxlength="8">
        <button id="joinBtn"><i class="bi bi-box-arrow-in-right"></i>Join swarm</button>
      </div>`;
    document.getElementById("joinBtn").addEventListener("click", async () => {
      try {
        await invoke("join_swarm", {
          baseUrl: document.getElementById("joinBaseUrl").value,
          code: document.getElementById("joinCode").value,
          deviceName: "SWARM Server",
        });
        showToast("Joined swarm.", "success");
        await refreshSwarm();
      } catch (err) {
        showToast(String(err), "error");
      }
    });
    return;
  }

  content.innerHTML = `
    <div class="grid">
      ${stat("STUN server", link.base_url)}
      ${stat("Trusted peers", link.allowed_peer_count)}
    </div>
    <p class="muted" style="margin-top:10px">
      Join codes are generated from the STUN server's own admin page:
      <span class="mono">${esc(link.base_url)}</span>
    </p>
    <div id="swarmList" style="margin-top:12px"></div>
    <div class="row" style="margin-top:12px">
      <input id="moreCode" placeholder="Join code for another swarm" maxlength="8">
      <button id="joinMoreBtn" class="secondary"><i class="bi bi-box-arrow-in-right"></i>Join another swarm</button>
      <button id="resyncBtn" class="secondary"><i class="bi bi-arrow-repeat"></i>Resync now</button>
    </div>`;

  const swarmList = document.getElementById("swarmList");
  swarmList.innerHTML = link.swarms.map(s => `
    <div class="card" style="background:var(--surface-muted); margin-bottom:10px">
      <div class="card-head">
        <strong>${esc(s.name)}</strong>
        <button class="danger" data-leave-swarm="${esc(s.id)}"><i class="bi bi-box-arrow-right"></i>Leave</button>
      </div>
      <div id="roster-${esc(s.id)}" class="muted">Loading roster…</div>
    </div>`).join("");

  for (const s of link.swarms) {
    loadRoster(s.id);
  }
  swarmList.querySelectorAll("[data-leave-swarm]").forEach(btn => {
    btn.addEventListener("click", async () => {
      try {
        await invoke("leave_swarm", { swarmId: btn.dataset.leaveSwarm });
        showToast("Left swarm.", "success");
        await refreshSwarm();
      } catch (err) {
        showToast(String(err), "error");
      }
    });
  });

  document.getElementById("joinMoreBtn").addEventListener("click", async () => {
    try {
      await invoke("join_additional_swarm", { code: document.getElementById("moreCode").value });
      showToast("Joined swarm.", "success");
      await refreshSwarm();
    } catch (err) {
      showToast(String(err), "error");
    }
  });
  document.getElementById("resyncBtn").addEventListener("click", async () => {
    try {
      await invoke("resync_swarm");
      showToast("Resynced.", "success");
      await refreshSwarm();
    } catch (err) {
      showToast(String(err), "error");
    }
  });
}

async function loadRoster(swarmId) {
  const el = document.getElementById(`roster-${swarmId}`);
  if (!el) return;
  try {
    const roster = await invoke("get_swarm_devices", { swarmId });
    if (!roster.devices.length) {
      el.innerHTML = `<span class="muted">No devices yet.</span>`;
      return;
    }
    const metaKeys = [...new Set(roster.devices.flatMap(d => Object.keys(d.metadata || {})))];
    el.innerHTML = `<table>
      <thead><tr>
        <th>Name</th><th>Type</th><th>Online</th><th>Last seen</th><th>Fingerprint</th>
        ${metaKeys.map(k => `<th>${esc(k)}</th>`).join("")}
      </tr></thead>
      <tbody>` + roster.devices.map(d => `<tr>
        <td>${esc(d.name)}</td>
        <td>${esc(d.device_type)}</td>
        <td>${d.online ? "✓" : "—"}</td>
        <td class="mono">${esc(d.last_seen_at || "—")}</td>
        <td class="mono">${esc((d.cert_fingerprint || "").slice(0, 12))}…</td>
        ${metaKeys.map(k => `<td class="mono">${esc((d.metadata || {})[k] ?? "—")}</td>`).join("")}
      </tr>`).join("") + `</tbody></table>`;
  } catch (err) {
    el.innerHTML = `<span class="muted">Unable to load roster.</span>`;
    showToast(String(err), "error");
  }
}
