// ---- Swarm tab: STUN link, membership, per-swarm device roster -------------
//
// Join-code *generation* is intentionally not here: the STUN server only
// lets the swarm's owning user (a session-cookie browser login) mint codes
// (`POST /swarms/{id}/codes` has no Bearer path), and this device only ever
// holds a device Bearer token — it was never designed to also hold a user
// login. Codes are generated from the STUN server's own admin page; this tab
// links there for convenience instead of duplicating that flow.

// Client-reported errors used to have a panel on this tab (they arrive over
// the same authenticated peer connection swarm membership uses) — moved to
// their own Notifications tab (notifications.js) once the badge count grew
// into its own "things to look at" surface distinct from swarm
// membership/roster management.

function formatFingerprint(fingerprint) {
  return `${fingerprint.slice(0, 12)}…${fingerprint.slice(-8)}`;
}

async function loadLocalPeers() {
  const list = document.getElementById("localPeersList");
  try {
    const peers = await invoke("list_local_peers");
    list.innerHTML = peers.length ? `<table>
      <thead><tr><th>Name</th><th>Paired</th><th class="info-trigger" data-info="device-fingerprint" tabindex="0" role="button">Certificate <i class="bi bi-info-circle info-affordance"></i></th><th></th></tr></thead>
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

document.getElementById("approveLanPairingBtn").addEventListener("click", async () => {
  const input = document.getElementById("lanActivationCode");
  const code = input.value.replace(/\D/g, "");
  const status = document.getElementById("lanPairingStatus");
  if (code.length !== 8) {
    showToast("Enter the 8-digit code shown on the TV.", "error");
    return;
  }
  try {
    const pairing = await invoke("approve_lan_pairing", { code });
    status.classList.remove("d-none");
    status.innerHTML = `<div class="note">
      <strong>${esc(pairing.name)}</strong> was approved. The TV will connect automatically.
    </div>`;
    input.value = "";
    showToast(`${pairing.name} was paired on the local network.`, "success");
    await loadLocalPeers();
  } catch (err) {
    showToast(String(err), "error");
  }
});

async function refreshSwarm() {
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
      <p class="muted"><i class="bi bi-link-45deg"></i> Not linked to a SWARM server yet.</p>
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
      ${stat("SWARM server", link.base_url, false, "stun-server-address")}
      ${stat("Trusted peers", link.allowed_peer_count, false, "trusted-peers")}
    </div>
    <div class="card" style="background:var(--surface-muted); margin-top:14px">
      <div class="card-head"><strong><i class="bi bi-tv"></i> Add a TV</strong></div>
      <p class="muted">On the TV, choose <strong>Connect through SWARM</strong>. Enter the temporary code shown there to review and approve that TV.</p>
      <div class="row">
        <input id="activationCode" inputmode="numeric" placeholder="8-digit TV code" maxlength="8">
        <button id="approveTvBtn"><i class="bi bi-shield-check"></i>Review TV</button>
      </div>
      <div id="activationReview" class="d-none" style="margin-top:10px"></div>
    </div>
    <div id="swarmList" style="margin-top:12px"></div>
    <div class="row" style="margin-top:12px">
      <input id="moreCode" placeholder="Join code for another swarm" maxlength="8">
      <button id="joinMoreBtn" class="secondary"><i class="bi bi-box-arrow-in-right"></i>Join another swarm</button>
      <button id="resyncBtn" class="secondary"><i class="bi bi-arrow-repeat"></i>Resync now</button>
    </div>`;

  const swarmList = document.getElementById("swarmList");
  document.getElementById("approveTvBtn").addEventListener("click", async () => {
    const code = document.getElementById("activationCode").value.replace(/\D/g, "");
    if (code.length !== 8) {
      showToast("Enter the 8-digit code shown on the TV.", "error");
      return;
    }
    const review = document.getElementById("activationReview");
    try {
      const pending = await invoke("lookup_tv_activation", { code });
      review.classList.remove("d-none");
      review.innerHTML = `<div class="note">
        <strong>${esc(pending.device_name)}</strong><br>
        <span class="muted">${esc(pending.platform)} · expires ${esc(new Date(pending.expires_at).toLocaleTimeString())}</span>
        <button id="confirmTvBtn" style="margin-left:12px"><i class="bi bi-check-lg"></i>Approve this TV</button>
      </div>`;
      document.getElementById("confirmTvBtn").addEventListener("click", async () => {
        try {
          await invoke("approve_tv_activation", { activationId: pending.activation_id });
          showToast(`${pending.device_name} was added to this swarm.`, "success");
          document.getElementById("activationCode").value = "";
          review.classList.add("d-none");
          for (const swarm of link.swarms) loadRoster(swarm.id);
        } catch (err) {
          showToast(String(err), "error");
        }
      });
    } catch (err) {
      review.classList.add("d-none");
      showToast(String(err), "error");
    }
  });
  swarmList.innerHTML = link.swarms.map(s => `
    <div class="card" style="background:var(--surface-muted); margin-bottom:10px">
      <div class="card-head">
        <strong class="swarm-name"><i class="bi bi-diagram-3"></i><span>${esc(s.name)}</span></strong>
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
        <th>Name</th><th>Type</th><th>Online</th><th>Last seen</th>
        <th class="info-trigger" data-info="device-fingerprint" tabindex="0" role="button">Fingerprint <i class="bi bi-info-circle info-affordance"></i></th>
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
