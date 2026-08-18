// ---- Swarm tab: STUN link, membership, per-swarm device roster -------------
//
// Join-code *generation* is intentionally not here: the STUN server only
// lets the swarm's owning user (a session-cookie browser login) mint codes
// (`POST /swarms/{id}/codes` has no Bearer path), and this device only ever
// holds a device Bearer token — it was never designed to also hold a user
// login. Codes are generated from the STUN server's own admin page; this tab
// links there for convenience instead of duplicating that flow.

async function refreshSwarm() {
  const content = document.getElementById("swarmContent");
  let link;
  try {
    link = await invoke("get_swarm_link");
  } catch (err) {
    content.innerHTML = `<p class="error">${esc(err)}</p>`;
    return;
  }

  if (!link) {
    content.innerHTML = `
      <p class="muted">Not linked to a STUN server yet.</p>
      <div class="row">
        <input id="joinBaseUrl" placeholder="https://swarm.example.com">
        <input id="joinCode" placeholder="12345678" maxlength="8">
        <button id="joinBtn">Join swarm</button>
      </div>
      <p id="joinError" class="error"></p>`;
    document.getElementById("joinBtn").addEventListener("click", async () => {
      const errorEl = document.getElementById("joinError");
      try {
        await invoke("join_swarm", {
          baseUrl: document.getElementById("joinBaseUrl").value,
          code: document.getElementById("joinCode").value,
          deviceName: "SWARM Server",
        });
        await refreshSwarm();
      } catch (err) {
        errorEl.textContent = String(err);
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
      <button id="joinMoreBtn" class="secondary">Join another swarm</button>
      <button id="resyncBtn" class="secondary">Resync now</button>
    </div>
    <p id="swarmActionError" class="error"></p>`;

  const swarmList = document.getElementById("swarmList");
  swarmList.innerHTML = link.swarms.map(s => `
    <div class="card" style="background:var(--surface-muted); margin-bottom:10px">
      <div class="card-head">
        <strong>${esc(s.name)}</strong>
        <button class="danger" data-leave-swarm="${esc(s.id)}">Leave</button>
      </div>
      <div id="roster-${esc(s.id)}" class="muted">Loading roster…</div>
    </div>`).join("");

  for (const s of link.swarms) {
    loadRoster(s.id);
  }
  swarmList.querySelectorAll("[data-leave-swarm]").forEach(btn => {
    btn.addEventListener("click", async () => {
      const errorEl = document.getElementById("swarmActionError");
      try {
        await invoke("leave_swarm", { swarmId: btn.dataset.leaveSwarm });
        await refreshSwarm();
      } catch (err) {
        errorEl.textContent = String(err);
      }
    });
  });

  document.getElementById("joinMoreBtn").addEventListener("click", async () => {
    const errorEl = document.getElementById("swarmActionError");
    try {
      await invoke("join_additional_swarm", { code: document.getElementById("moreCode").value });
      await refreshSwarm();
    } catch (err) {
      errorEl.textContent = String(err);
    }
  });
  document.getElementById("resyncBtn").addEventListener("click", async () => {
    const errorEl = document.getElementById("swarmActionError");
    try {
      await invoke("resync_swarm");
      await refreshSwarm();
    } catch (err) {
      errorEl.textContent = String(err);
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
    el.innerHTML = `<span class="error">${esc(err)}</span>`;
  }
}
