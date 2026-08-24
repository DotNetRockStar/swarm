// Persistent media-server notifications and client-reported errors. Rows
// show a three-line preview; selecting one opens its complete report.

const notificationModalBackdrop = document.getElementById("notificationModalBackdrop");

function closeNotificationModal() {
  notificationModalBackdrop.classList.add("d-none");
}

function openNotificationModal({ title, body, meta, level = "error", icon = "bi-bell-fill" }) {
  document.getElementById("notificationModalTitle").textContent = title;
  document.getElementById("notificationModalBody").textContent = body || "No additional details.";
  document.getElementById("notificationModalMeta").textContent = meta || "";
  const iconEl = document.getElementById("notificationModalIcon");
  iconEl.className = `bi ${icon}`;
  iconEl.parentElement.style.color = level === "error" ? "#ff5d7a" : level === "warning" ? "#f5c451" : "var(--green)";
  notificationModalBackdrop.classList.remove("d-none");
}

document.getElementById("notificationModalClose").addEventListener("click", closeNotificationModal);
document.getElementById("notificationModalCopy").addEventListener("click", async () => {
  const notificationText = [
    document.getElementById("notificationModalTitle").textContent,
    document.getElementById("notificationModalMeta").textContent,
    document.getElementById("notificationModalBody").textContent,
  ].filter(Boolean).join("\n\n");
  try {
    await navigator.clipboard.writeText(notificationText);
    showToast("Notification copied.", "success");
  } catch (error) {
    showToast(`Could not copy the notification: ${error}`, "error");
  }
});
notificationModalBackdrop.addEventListener("click", event => {
  if (event.target === notificationModalBackdrop) closeNotificationModal();
});
document.addEventListener("keydown", event => {
  if (event.key === "Escape" && !notificationModalBackdrop.classList.contains("d-none")) closeNotificationModal();
});

async function refreshNotifications() {
  await Promise.all([loadServerNotifications(), loadClientErrors()]);
  await refreshNotificationBadge();
}

async function loadServerNotifications() {
  const list = document.getElementById("serverNotificationsList");
  const countEl = document.getElementById("serverNotificationsCount");
  const clearBtn = document.getElementById("clearServerNotificationsBtn");
  try {
    const notifications = await invoke("list_server_notifications");
    countEl.textContent = notifications.length ? `${notifications.length} notification${notifications.length === 1 ? "" : "s"}` : "";
    clearBtn.classList.toggle("d-none", notifications.length === 0);
    list.innerHTML = notifications.length ? notifications.map(notification => `
      <div class="client-error-row notification-level-${esc(notification.level)}" data-open-notification="server-${notification.id}" tabindex="0" role="button">
        <div style="flex:1; min-width:0">
          <div class="client-error-message">${esc(notification.title)}</div>
          <div class="client-error-meta"><span><i class="bi bi-clock"></i> ${esc(new Date(notification.created_at_ms).toLocaleString())}</span></div>
          <div class="notification-preview">${esc(notification.message)}</div>
        </div>
        <button class="danger" data-delete-server-notification="${notification.id}" style="padding:5px 10px; font-size:.75rem" title="Delete"><i class="bi bi-x-lg"></i></button>
      </div>`).join("") : `<p class="muted">No server notifications.</p>`;

    notifications.forEach(notification => {
      const row = list.querySelector(`[data-open-notification="server-${notification.id}"]`);
      const open = () => openNotificationModal({
        title: notification.title,
        body: notification.message,
        meta: new Date(notification.created_at_ms).toLocaleString(),
        level: notification.level,
        icon: notification.level === "error" ? "bi-x-circle-fill" : notification.level === "warning" ? "bi-exclamation-triangle-fill" : "bi-check-circle-fill",
      });
      row.addEventListener("click", open);
      row.addEventListener("keydown", event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); open(); }
      });
    });
    list.querySelectorAll("[data-delete-server-notification]").forEach(button => {
      button.addEventListener("click", async event => {
        event.stopPropagation();
        try {
          await invoke("delete_server_notification", { id: Number(button.dataset.deleteServerNotification) });
          await loadServerNotifications();
          await refreshNotificationBadge();
        } catch (error) {
          showToast(String(error), "error");
        }
      });
    });
  } catch (error) {
    list.innerHTML = `<p class="muted">Unable to load server notifications.</p>`;
    showToast(String(error), "error");
  }
}

async function loadClientErrors() {
  const list = document.getElementById("clientErrorsList");
  const countEl = document.getElementById("clientErrorsCount");
  const clearBtn = document.getElementById("clearClientErrorsBtn");
  try {
    const errors = await invoke("list_client_errors");
    countEl.textContent = errors.length ? `${errors.length} error${errors.length === 1 ? "" : "s"}` : "";
    clearBtn.classList.toggle("d-none", errors.length === 0);
    list.innerHTML = errors.length ? errors.map(error => `
      <div class="client-error-row notification-level-error" data-open-notification="client-${error.id}" tabindex="0" role="button">
        <div style="flex:1; min-width:0">
          <div class="client-error-message">${esc(error.asset_title || error.device_name || "Client error")}</div>
          <div class="client-error-meta">
            <span><i class="bi bi-clock"></i> ${esc(new Date(error.occurred_at_ms).toLocaleString())}</span>
            <span><i class="bi bi-tv"></i> ${esc(error.device_name)}</span>
            ${error.kind ? `<span>${esc(error.kind)}</span>` : ""}
          </div>
          <div class="notification-preview">${esc([error.message, error.context].filter(Boolean).join("\n"))}</div>
        </div>
        <button class="danger" data-delete-error="${error.id}" style="padding:5px 10px; font-size:.75rem" title="Delete"><i class="bi bi-x-lg"></i></button>
      </div>`).join("") : `<p class="muted">No client errors reported.</p>`;

    errors.forEach(error => {
      const row = list.querySelector(`[data-open-notification="client-${error.id}"]`);
      const open = () => openNotificationModal({
        title: error.asset_title || "Client error",
        body: [error.message, error.context].filter(Boolean).join("\n\n"),
        meta: `${new Date(error.occurred_at_ms).toLocaleString()} • ${error.device_name}${error.kind ? ` • ${error.kind}` : ""}`,
        level: "error",
        icon: "bi-tv",
      });
      row.addEventListener("click", open);
      row.addEventListener("keydown", event => {
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); open(); }
      });
    });
    list.querySelectorAll("[data-delete-error]").forEach(button => {
      button.addEventListener("click", async event => {
        event.stopPropagation();
        try {
          await invoke("delete_client_error", { id: Number(button.dataset.deleteError) });
          await loadClientErrors();
          await refreshNotificationBadge();
        } catch (error) {
          showToast(String(error), "error");
        }
      });
    });
  } catch (error) {
    list.innerHTML = `<p class="muted">Unable to load client errors.</p>`;
    showToast(String(error), "error");
  }
}

document.getElementById("clearServerNotificationsBtn").addEventListener("click", async () => {
  try {
    await invoke("clear_server_notifications");
    showToast("Cleared server notifications.", "success");
    await refreshNotifications();
  } catch (error) {
    showToast(String(error), "error");
  }
});

document.getElementById("clearClientErrorsBtn").addEventListener("click", async () => {
  try {
    await invoke("clear_client_errors");
    showToast("Cleared client errors.", "success");
    await refreshNotifications();
  } catch (error) {
    showToast(String(error), "error");
  }
});
