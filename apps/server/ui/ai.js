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
  } catch (err) {
    showToast(String(err), "error");
  }
}

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
