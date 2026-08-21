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
    renderMcpStatus(settings);
    renderMcpConfigSnippet(settings);
  } catch (err) {
    showToast(String(err), "error");
  }
}

function renderMcpStatus(settings) {
  const status = document.getElementById("mcpStatus");
  status.textContent = settings.mcp_enabled
    ? `Enabled on port ${settings.mcp_port} — restart this app if you just changed this.`
    : "Disabled.";
}

function renderMcpConfigSnippet(settings) {
  const card = document.getElementById("mcpConfigCard");
  card.classList.toggle("d-none", !settings.mcp_enabled);
  const snippet = {
    mcpServers: {
      swarm: {
        type: "streamableHttp",
        url: `http://<this-machine's-LAN-IP>:${settings.mcp_port}/mcp`,
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
