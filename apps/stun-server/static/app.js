"use strict";

// ---- helpers -------------------------------------------------------------

function escapeHtml(value) {
  return String(value ?? "").replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[ch]);
}

function csrfToken() {
  const match = document.cookie.match(/(?:^|;\s*)swarm_csrf=([^;]+)/);
  return match ? decodeURIComponent(match[1]) : "";
}

async function api(path, options = {}) {
  const headers = { ...(options.headers || {}) };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  if ((options.method || "GET") !== "GET") headers["x-swarm-csrf"] = csrfToken();
  const response = await fetch(path, {
    ...options,
    headers,
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
  });
  let payload = null;
  try { payload = await response.json(); } catch { /* bodyless */ }
  if (!response.ok) {
    throw new Error(payload?.message || `request failed (${response.status})`);
  }
  return payload;
}

function alertBox(kind, message) {
  const box = document.getElementById("alerts");
  box.innerHTML = `<div class="alert alert-${kind} alert-dismissible" role="alert">${escapeHtml(message)}
    <button type="button" class="btn-close" data-bs-dismiss="alert"></button></div>`;
}

// ---- auth view -----------------------------------------------------------

let authMode = "login";

document.getElementById("authTabs").addEventListener("click", (event) => {
  const tab = event.target.closest("[data-tab]");
  if (!tab) return;
  authMode = tab.dataset.tab;
  document.querySelectorAll("#authTabs .nav-link").forEach((el) => el.classList.toggle("active", el === tab));
  document.getElementById("authSubmit").textContent = authMode === "login" ? "Sign in" : "Create account";
  document.getElementById("authHint").textContent = authMode === "login" ? "" : "At least 10 characters.";
});

document.getElementById("authForm").addEventListener("submit", async (event) => {
  event.preventDefault();
  const email = document.getElementById("authEmail").value;
  const password = document.getElementById("authPassword").value;
  try {
    if (authMode === "register") {
      await api("/api/v1/auth/register", { method: "POST", body: { email, password } });
      alertBox("success", "Account created — signing you in…");
    }
    await api("/api/v1/auth/login", { method: "POST", body: { email, password } });
    await boot();
  } catch (err) {
    alertBox("danger", err.message);
  }
});

document.getElementById("forgotLink").addEventListener("click", async (event) => {
  event.preventDefault();
  const email = document.getElementById("authEmail").value;
  if (!email) { alertBox("warning", "Enter your email first, then click Forgot password."); return; }
  await api("/api/v1/auth/request-reset", { method: "POST", body: { email } });
  alertBox("info", "If that account exists, a reset link was generated (check the server log while email delivery is unconfigured).");
});

document.getElementById("logoutBtn").addEventListener("click", async () => {
  await api("/api/v1/auth/logout", { method: "POST" });
  location.hash = "";
  await boot();
});

// ---- dashboard -----------------------------------------------------------

async function loadSwarms() {
  const { swarms } = await api("/api/v1/swarms");
  const container = document.getElementById("swarmList");
  if (!swarms.length) {
    container.innerHTML = `<span class="text-muted">No swarms yet — create one above, then generate a join code for each device.</span>`;
    return;
  }
  container.classList.remove("p-3");
  container.innerHTML = `<div class="accordion accordion-flush" id="swarmAccordion">` + swarms.map((swarm, index) => `
    <div class="accordion-item" style="background: transparent">
      <h2 class="accordion-header">
        <button class="accordion-button collapsed" data-bs-toggle="collapse" data-bs-target="#swarm-${index}"
                style="background: var(--swarm-surface); color: var(--swarm-text)">
          <i class="bi bi-diagram-3 me-2"></i>${escapeHtml(swarm.name)}
          <span class="badge text-bg-dark border ms-2">${swarm.device_count} device${swarm.device_count === 1 ? "" : "s"}</span>
        </button>
      </h2>
      <div id="swarm-${index}" class="accordion-collapse collapse" data-bs-parent="#swarmAccordion">
        <div class="accordion-body">
          <div class="d-flex gap-2 mb-3">
            <button class="btn btn-sm btn-swarm" data-action="code" data-id="${escapeHtml(swarm.id)}" data-name="${escapeHtml(swarm.name)}">
              <i class="bi bi-key me-1"></i>Generate join code</button>
            <button class="btn btn-sm btn-outline-danger ms-auto" data-action="delete-swarm" data-id="${escapeHtml(swarm.id)}" data-name="${escapeHtml(swarm.name)}">
              <i class="bi bi-trash me-1"></i>Delete swarm</button>
          </div>
          <div data-devices-for="${escapeHtml(swarm.id)}" class="text-muted small">Loading devices…</div>
        </div>
      </div>
    </div>`).join("") + `</div>`;

  for (const swarm of swarms) loadSwarmDevices(swarm.id);
}

async function loadSwarmDevices(swarmId) {
  const target = document.querySelector(`[data-devices-for="${CSS.escape(swarmId)}"]`);
  if (!target) return;
  const { devices } = await api(`/api/v1/swarms/${encodeURIComponent(swarmId)}/devices`);
  if (!devices.length) {
    target.textContent = "No devices in this swarm yet.";
    return;
  }
  target.classList.remove("small", "text-muted");
  target.innerHTML = `<div class="table-responsive"><table class="table table-sm align-middle mb-0">
    <thead><tr class="text-muted small"><th>Device</th><th>Type</th><th>Status</th><th>Last seen</th><th>Fingerprint</th></tr></thead>
    <tbody>` + devices.map((device) => `
      <tr>
        <td>${escapeHtml(device.name)}</td>
        <td><span class="badge text-bg-dark border">${escapeHtml(device.device_type)}</span></td>
        <td><span class="badge ${device.online ? "badge-online" : "badge-offline"}">${device.online ? "online" : "offline"}</span></td>
        <td class="small text-muted">${escapeHtml(device.last_seen_at || "never")}</td>
        <td class="fingerprint" title="${escapeHtml(device.cert_fingerprint)}">${escapeHtml(device.cert_fingerprint.slice(0, 16))}…</td>
      </tr>`).join("") + `</tbody></table></div>`;
}

async function loadMyDevices() {
  const { devices } = await api("/api/v1/me/devices");
  const container = document.getElementById("deviceList");
  if (!devices.length) {
    container.innerHTML = `<span class="text-muted">No devices registered. Generate a join code in a swarm and enter it on a device.</span>`;
    return;
  }
  container.classList.remove("p-3");
  container.innerHTML = `<div class="table-responsive"><table class="table table-sm align-middle mb-0">
    <thead><tr class="text-muted small"><th>Device</th><th>Type</th><th>Platform</th><th>Status</th><th>Swarms</th><th></th></tr></thead>
    <tbody>` + devices.map((device) => `
      <tr>
        <td>${escapeHtml(device.name)}</td>
        <td><span class="badge text-bg-dark border">${escapeHtml(device.device_type)}</span></td>
        <td class="small">${escapeHtml(device.platform)}</td>
        <td><span class="badge ${device.online ? "badge-online" : "badge-offline"}">${device.online ? "online" : "offline"}</span></td>
        <td class="small">${device.swarms.map((s) => escapeHtml(s.name)).join(", ") || "—"}</td>
        <td class="text-end"><button class="btn btn-sm btn-outline-danger" data-action="revoke" data-id="${escapeHtml(device.device_id)}" data-name="${escapeHtml(device.name)}">Revoke</button></td>
      </tr>`).join("") + `</tbody></table></div>`;
}

document.getElementById("createSwarmForm").addEventListener("submit", async (event) => {
  event.preventDefault();
  const input = document.getElementById("newSwarmName");
  try {
    await api("/api/v1/swarms", { method: "POST", body: { name: input.value } });
    input.value = "";
    await loadSwarms();
  } catch (err) { alertBox("danger", err.message); }
});

document.getElementById("passwordForm").addEventListener("submit", async (event) => {
  event.preventDefault();
  try {
    await api("/api/v1/auth/password", { method: "POST", body: {
      current_password: document.getElementById("curPassword").value,
      new_password: document.getElementById("newPassword").value,
    }});
    document.getElementById("curPassword").value = "";
    document.getElementById("newPassword").value = "";
    alertBox("success", "Password changed. Other sessions were signed out.");
  } catch (err) { alertBox("danger", err.message); }
});

document.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-action]");
  if (!button) return;
  const { action, id, name } = button.dataset;
  try {
    if (action === "code") {
      const result = await api(`/api/v1/swarms/${encodeURIComponent(id)}/codes`, { method: "POST", body: {} });
      document.getElementById("codeSwarmName").textContent = name;
      document.getElementById("codeValue").textContent = result.code.replace(/(\d{4})(\d{4})/, "$1 $2");
      document.getElementById("codeExpiry").textContent = new Date(result.expires_at).toLocaleTimeString();
      bootstrap.Modal.getOrCreateInstance(document.getElementById("codeModal")).show();
    } else if (action === "delete-swarm") {
      if (!confirm(`Delete swarm "${name}"? Devices stay registered but leave this swarm.`)) return;
      await api(`/api/v1/swarms/${encodeURIComponent(id)}`, { method: "DELETE" });
      await loadSwarms();
    } else if (action === "revoke") {
      if (!confirm(`Revoke device "${name}"? Its access token stops working immediately.`)) return;
      await api(`/api/v1/devices/${encodeURIComponent(id)}`, { method: "DELETE" });
      await Promise.all([loadMyDevices(), loadSwarms()]);
    }
  } catch (err) { alertBox("danger", err.message); }
});

// ---- verify / reset deep links (#verify=…, #reset=…) ---------------------

async function handleHashTokens() {
  const verifyMatch = location.hash.match(/^#verify=([0-9a-f]+)$/);
  if (verifyMatch) {
    try {
      await api("/api/v1/auth/verify", { method: "POST", body: { token: verifyMatch[1] } });
      alertBox("success", "Email verified.");
    } catch (err) { alertBox("danger", err.message); }
    location.hash = "";
  }
  const resetMatch = location.hash.match(/^#reset=([0-9a-f]+)$/);
  if (resetMatch) {
    const newPassword = prompt("Enter a new password (at least 10 characters):");
    if (newPassword) {
      try {
        await api("/api/v1/auth/reset", { method: "POST", body: { token: resetMatch[1], new_password: newPassword } });
        alertBox("success", "Password reset — sign in with the new password.");
      } catch (err) { alertBox("danger", err.message); }
    }
    location.hash = "";
  }
}

// ---- boot ----------------------------------------------------------------

async function boot() {
  await handleHashTokens();
  const session = await api("/api/v1/auth/session");
  document.getElementById("authView").classList.toggle("d-none", session.authenticated);
  document.getElementById("dashView").classList.toggle("d-none", !session.authenticated);
  document.getElementById("navRight").classList.toggle("d-none", !session.authenticated);
  if (session.authenticated) {
    document.getElementById("navEmail").textContent = session.email;
    await Promise.all([loadSwarms(), loadMyDevices()]);
  }
}

boot().catch((err) => alertBox("danger", err.message));
