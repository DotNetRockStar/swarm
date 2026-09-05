// ---- AI tab: MCP server enable/config ---------------------------------------
//
// The MCP server itself (apps/server/src/mcp.rs) only starts once, inside
// AppState::core, at the same time ServerCore does — see that file's doc
// comment. Saving a setting here takes effect on the *next* restart, not
// live; this tab is honest about that rather than implying an instant toggle.

async function refreshAi() {
  try {
    const settings = await invoke("get_settings");
    document.getElementById("mcpEnabledCheck").checked = settings.mcp_enabled;
    document.getElementById("mcpPortInput").value = settings.mcp_port;
    const tokenInput = document.getElementById("mcpAccessTokenInput");
    tokenInput.value = settings.mcp_access_token || "";
    document.getElementById("generateMcpTokenBtn").innerHTML = settings.mcp_access_token
      ? '<i class="bi bi-arrow-repeat"></i>Regenerate token'
      : '<i class="bi bi-key-fill"></i>Create access token';
    document.getElementById("copyMcpTokenBtn").disabled = !settings.mcp_access_token;
    renderMcpStatus(settings);
    renderMcpConfigSnippet(settings);
    renderAiProviders(settings);
    await refreshScanAssist(settings);
    await refreshReorganize(settings);
  } catch (err) {
    showToast(String(err), "error");
  }
}

// ---- AI tab: provider configuration (issue #235) ---------------------------
//
// One row per configured provider (Claude/Codex/Grok), all built from the
// same `settings.ai_providers` shape so adding a fourth provider later is a
// backend-only change (see `settings::default_ai_providers`). An API key
// input is always shown blank — `has_api_key` only says whether one is
// already saved, the raw key is never sent back to the UI, matching the
// TMDb/OpenSubtitles key fields on the Details tab.

function renderAiProviders(settings) {
  const list = document.getElementById("aiProvidersList");
  list.innerHTML = settings.ai_providers
    .map(
      p => `
    <div class="row ai-provider-row" data-provider-id="${esc(p.id)}" style="align-items:center; margin-bottom:6px; gap:8px">
      <label class="checkbox-label" style="flex:0 0 110px"><input type="checkbox" class="ai-provider-enabled" ${p.enabled ? "checked" : ""}> ${esc(p.label)}</label>
      <input class="ai-provider-model" value="${esc(p.model)}" placeholder="Model" style="flex:1">
      <input class="ai-provider-key mono" type="password" placeholder="${p.has_api_key ? "Key saved — leave blank to keep" : "API key"}" style="flex:1">
      <button class="secondary ai-provider-save"><i class="bi bi-check-lg"></i>Save</button>
      <button class="secondary ai-provider-test"><i class="bi bi-broadcast"></i>Test</button>
    </div>
    <p class="note" data-provider-status="${esc(p.id)}" style="margin:0 0 12px"></p>`
    )
    .join("");

  list.querySelectorAll(".ai-provider-save").forEach(btn => {
    btn.addEventListener("click", async () => {
      const row = btn.closest(".ai-provider-row");
      const id = row.dataset.providerId;
      const enabled = row.querySelector(".ai-provider-enabled").checked;
      const model = row.querySelector(".ai-provider-model").value.trim();
      const key = row.querySelector(".ai-provider-key").value;
      try {
        await invoke("set_ai_provider_enabled", { id, enabled });
        if (model) await invoke("set_ai_provider_model", { id, model });
        if (key) await invoke("set_ai_provider_api_key", { id, key });
        showToast("Saved.", "success");
        await refreshAi();
      } catch (err) {
        showToast(String(err), "error");
      }
    });
  });

  list.querySelectorAll(".ai-provider-test").forEach(btn => {
    btn.addEventListener("click", async () => {
      const row = btn.closest(".ai-provider-row");
      const id = row.dataset.providerId;
      const status = list.querySelector(`[data-provider-status="${id}"]`);
      status.textContent = "Testing…";
      status.classList.remove("error");
      btn.disabled = true;
      try {
        const reply = await invoke("test_ai_provider", { id });
        status.textContent = `Connected — replied "${reply}".`;
      } catch (err) {
        status.textContent = String(err);
        status.classList.add("error");
      } finally {
        btn.disabled = false;
      }
    });
  });
}

// ---- AI tab: scan & scrape assist -------------------------------------------
//
// Offers AI help only for entries the last `run_scrape` (or library
// maintenance) pass actually failed to match — see `list_scrape_issues` in
// gui.rs, backed by `AppState::last_scrape_issues` (in-memory, current
// session only). Applying a suggestion reuses the existing `rescrape_entry`
// command with the AI-confirmed TMDb id, exactly like a manual "fix match"
// would — this feature only ever proposes, the user always clicks Apply.

async function refreshScanAssist(settings) {
  document.getElementById("aiScanAssistCheck").checked = settings.ai_scan_assist_enabled;
  const status = document.getElementById("aiScanAssistStatus");
  const hasProvider = settings.ai_providers.some(p => p.enabled && p.has_api_key);
  if (settings.ai_scan_assist_enabled && !hasProvider) {
    status.textContent = "Enabled, but no AI provider is configured yet — add one above.";
    status.classList.add("error");
  } else {
    status.textContent = settings.ai_scan_assist_enabled ? "Enabled." : "Disabled.";
    status.classList.remove("error");
  }

  const wrap = document.getElementById("scrapeAssistWrap");
  if (!settings.ai_scan_assist_enabled) {
    wrap.classList.add("d-none");
    return;
  }
  let issues = [];
  try {
    issues = await invoke("list_scrape_issues");
  } catch (err) {
    showToast(String(err), "error");
  }
  wrap.classList.toggle("d-none", issues.length === 0);
  const list = document.getElementById("scrapeAssistList");
  list.innerHTML = issues
    .map(
      issue => `
    <li data-entry-key="${esc(issue.entry_key)}">
      <span class="issue-title">${esc(issue.title)}</span> — <span class="issue-reason">${esc(issue.reason)}</span>
      <button class="secondary ask-ai-btn" style="margin-left:8px; padding:2px 8px; font-size:.75rem"><i class="bi bi-stars"></i>Ask AI</button>
      <div class="ai-suggestion muted" style="margin-top:4px; font-size:.8rem"></div>
    </li>`
    )
    .join("");

  list.querySelectorAll(".ask-ai-btn").forEach(btn => {
    btn.addEventListener("click", async () => {
      const li = btn.closest("li");
      const entryKey = li.dataset.entryKey;
      const suggestionBox = li.querySelector(".ai-suggestion");
      btn.disabled = true;
      suggestionBox.textContent = "Asking AI…";
      try {
        const suggestion = await invoke("ai_scrape_assist", { entryKey });
        suggestionBox.innerHTML = `Suggested: <strong>${esc(suggestion.tmdb_title)}</strong>${
          suggestion.suggested_year ? ` (${esc(String(suggestion.suggested_year))})` : ""
        } <button class="secondary apply-ai-suggestion-btn" style="padding:2px 8px; font-size:.75rem"><i class="bi bi-check-lg"></i>Apply</button>`;
        suggestionBox.querySelector(".apply-ai-suggestion-btn").addEventListener("click", async () => {
          try {
            await invoke("rescrape_entry", { entryKey, tmdbUrl: suggestion.tmdb_url });
            showToast("Applied.", "success");
            await refreshAi();
            await refreshLibrary();
          } catch (err) {
            showToast(String(err), "error");
          }
        });
      } catch (err) {
        suggestionBox.textContent = String(err);
      } finally {
        btn.disabled = false;
      }
    });
  });
}

document.getElementById("saveAiScanAssistBtn").addEventListener("click", async () => {
  try {
    await invoke("set_ai_scan_assist_enabled", { enabled: document.getElementById("aiScanAssistCheck").checked });
    showToast("Saved.", "success");
    await refreshAi();
  } catch (err) {
    showToast(String(err), "error");
  }
});

// ---- AI tab: reorganize media ------------------------------------------------
//
// A plan only ever proposes; nothing on disk changes until
// `approve_ai_reorg_plan` runs (never a delete, never an overwrite — see
// `reorganize.rs`). Plans live in memory only (`AppState::reorg_plans`), so
// they don't survive a restart — a fresh scan is cheap enough that this
// isn't worth persisting.

async function refreshReorganize(settings) {
  document.getElementById("aiReorganizeCheck").checked = settings.ai_reorganize_enabled;
  document.getElementById("aiReorganizeStatus").textContent = settings.ai_reorganize_enabled ? "Enabled." : "Disabled.";

  const scanWrap = document.getElementById("aiReorganizeScanWrap");
  scanWrap.classList.toggle("d-none", !settings.ai_reorganize_enabled);
  if (settings.ai_reorganize_enabled) {
    try {
      const roots = await invoke("list_media_roots");
      document.getElementById("aiReorganizeRootSelect").innerHTML = roots
        .map(r => `<option value="${esc(r.label)}">${esc(r.label)}</option>`)
        .join("");
    } catch (err) {
      showToast(String(err), "error");
    }
  }

  let plans = [];
  try {
    plans = await invoke("list_ai_reorg_plans");
  } catch (err) {
    showToast(String(err), "error");
  }
  renderReorgPlans(plans);
}

function renderReorgPlans(plans) {
  const wrap = document.getElementById("aiReorgPlansList");
  if (!plans || plans.length === 0) {
    wrap.innerHTML = "";
    return;
  }
  wrap.innerHTML = plans
    .slice()
    .reverse()
    .map(plan => {
      const itemsHtml =
        plan.items
          .map(
            item => `
        <li>
          <span class="mono">${esc(item.from)}</span> → <span class="mono">${esc(item.to)}</span>
          ${item.ai_assisted ? '<span class="muted" style="font-size:.72rem"> (AI-assisted)</span>' : ""}
          ${item.conflict ? `<br><span class="issue-reason">${esc(item.conflict)} — left in place</span>` : ""}
        </li>`
          )
          .join("") || '<li class="muted">Nothing to reorganize — this root already looks consistent.</li>';
      const summaryHtml = plan.apply_summary
        ? `<p class="muted">${plan.apply_summary.applied} moved, ${plan.apply_summary.skipped} skipped.${
            plan.apply_summary.errors.length ? `<br>${plan.apply_summary.errors.map(esc).join("<br>")}` : ""
          }</p>`
        : "";
      const actionsHtml =
        plan.status === "proposed"
          ? `<button class="secondary approve-reorg-btn" data-plan-id="${plan.id}"><i class="bi bi-check-lg"></i>Approve &amp; apply</button>
           <button class="secondary reject-reorg-btn" data-plan-id="${plan.id}"><i class="bi bi-x-lg"></i>Reject</button>`
          : "";
      return `
        <div class="card" style="margin-top:12px; background:var(--surface-muted)">
          <div class="row" style="justify-content:space-between; align-items:center">
            <strong>${esc(plan.root_label)}</strong>
            <span class="muted">${plan.items.length} item(s), ${plan.ai_assisted_count} AI-assisted, ${plan.conflict_count} conflict(s) — <em>${esc(plan.status)}</em></span>
          </div>
          <ul class="issue-list" style="margin-top:8px">${itemsHtml}</ul>
          ${summaryHtml}
          <div class="row" style="margin-top:8px">${actionsHtml}</div>
        </div>`;
    })
    .join("");

  wrap.querySelectorAll(".approve-reorg-btn").forEach(btn => {
    btn.addEventListener("click", async () => {
      const id = Number(btn.dataset.planId);
      btn.disabled = true;
      try {
        await invoke("approve_ai_reorg_plan", { id });
        showToast("Reorganize applied — rescanning library.", "success");
        await refreshAi();
        await refreshLibrary();
      } catch (err) {
        showToast(String(err), "error");
        btn.disabled = false;
      }
    });
  });
  wrap.querySelectorAll(".reject-reorg-btn").forEach(btn => {
    btn.addEventListener("click", async () => {
      const id = Number(btn.dataset.planId);
      try {
        await invoke("reject_ai_reorg_plan", { id });
        await refreshAi();
      } catch (err) {
        showToast(String(err), "error");
      }
    });
  });
}

document.getElementById("aiReorganizeScanBtn").addEventListener("click", async () => {
  const btn = document.getElementById("aiReorganizeScanBtn");
  const rootLabel = document.getElementById("aiReorganizeRootSelect").value;
  if (!rootLabel) {
    showToast("Add a media root first.", "error");
    return;
  }
  btn.disabled = true;
  try {
    await invoke("ai_reorganize_scan", { rootLabel });
    await refreshAi();
  } catch (err) {
    showToast(String(err), "error");
  } finally {
    btn.disabled = false;
  }
});

document.getElementById("saveAiReorganizeBtn").addEventListener("click", async () => {
  try {
    await invoke("set_ai_reorganize_enabled", { enabled: document.getElementById("aiReorganizeCheck").checked });
    showToast("Saved.", "success");
    await refreshAi();
  } catch (err) {
    showToast(String(err), "error");
  }
});

function renderMcpStatus(settings) {
  const status = document.getElementById("mcpStatus");
  status.textContent = settings.mcp_enabled
    ? settings.mcp_access_token
      ? `Enabled on port ${settings.mcp_port} — restart SWARM after changing the server or token.`
      : "Access token required before the MCP Server can start."
    : "Disabled.";
  status.classList.toggle("error", settings.mcp_enabled && !settings.mcp_access_token);
}

function renderMcpConfigSnippet(settings) {
  const card = document.getElementById("mcpConfigCard");
  card.classList.toggle("d-none", !settings.mcp_enabled || !settings.mcp_access_token);
  const snippet = {
    mcpServers: {
      swarm: {
        type: "streamableHttp",
        url: `http://<this-machine's-LAN-IP>:${settings.mcp_port}/mcp`,
        headers: {
          Authorization: `Bearer ${settings.mcp_access_token || "<access-token>"}`,
        },
      },
    },
  };
  document.getElementById("mcpConfigSnippet").textContent =
    JSON.stringify(snippet, null, 2) +
    "\n\n// Replace <this-machine's-LAN-IP> with this computer's network address\n// (check your OS's network settings — \"localhost\" only works if the\n// MCP client runs on this same machine).";
}

document.getElementById("saveMcpSettingsBtn").addEventListener("click", async () => {
  try {
    const enabled = document.getElementById("mcpEnabledCheck").checked;
    if (enabled && !document.getElementById("mcpAccessTokenInput").value) {
      showToast("Create an access token before enabling the MCP Server.", "error");
      return;
    }
    const portValue = document.getElementById("mcpPortInput").value.trim();
    const port = portValue ? Number(portValue) : 7890;
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      showToast("Port must be a whole number between 1 and 65535.", "error");
      return;
    }
    await invoke("set_mcp_enabled", { enabled });
    await invoke("set_mcp_port", { port });
    showToast("Saved. Restart the app for this to take effect.", "success");
    await refreshAi();
  } catch (err) {
    showToast(String(err), "error");
  }
});

document.getElementById("generateMcpTokenBtn").addEventListener("click", async () => {
  try {
    const token = await invoke("generate_mcp_access_token");
    document.getElementById("mcpAccessTokenInput").value = token;
    showToast("Access token created. Restart SWARM if the MCP Server is already enabled.", "success");
    await refreshAi();
  } catch (err) {
    showToast(String(err), "error");
  }
});

document.getElementById("copyMcpTokenBtn").addEventListener("click", async () => {
  const token = document.getElementById("mcpAccessTokenInput").value;
  if (!token) return;
  try {
    await navigator.clipboard.writeText(token);
    showToast("Access token copied.", "success");
  } catch (err) {
    showToast(`Could not copy the token: ${err}`, "error");
  }
});
