//! In-memory signaling hub: which devices hold a live WSS session, and a
//! sender handle to push messages to each. Swarm-membership checks stay in
//! the route layer (they need the database); the hub is pure connection
//! bookkeeping.

use std::collections::HashMap;
use std::sync::Mutex;
use swarm_core::signal::SignalMessage;
use tokio::sync::mpsc;

pub struct Hub {
    inner: Mutex<HashMap<String, Connected>>,
}

struct Connected {
    session_id: String,
    sender: mpsc::UnboundedSender<SignalMessage>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Register a device connection. A newer connection replaces an older one
    /// (the old receiver drops, ending its socket task).
    pub fn connect(
        &self,
        device_id: &str,
        session_id: &str,
    ) -> mpsc::UnboundedReceiver<SignalMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.lock().unwrap().insert(
            device_id.to_string(),
            Connected {
                session_id: session_id.to_string(),
                sender: tx,
            },
        );
        rx
    }

    /// Remove the connection, but only if it is still the one identified by
    /// `session_id` — a reconnect that already replaced us must not be torn
    /// down by the old socket's cleanup.
    pub fn disconnect(&self, device_id: &str, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .get(device_id)
            .is_some_and(|c| c.session_id == session_id)
        {
            inner.remove(device_id);
        }
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.inner.lock().unwrap().contains_key(device_id)
    }

    /// Push a message to a connected device. Returns false if it is offline
    /// (or its channel already closed).
    pub fn send_to(&self, device_id: &str, message: SignalMessage) -> bool {
        let inner = self.inner.lock().unwrap();
        match inner.get(device_id) {
            Some(connected) => connected.sender.send(message).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_and_stale_disconnect() {
        let hub = Hub::new();
        let _rx1 = hub.connect("dev", "s1");
        let _rx2 = hub.connect("dev", "s2"); // reconnect replaces s1
        hub.disconnect("dev", "s1"); // stale cleanup must be a no-op
        assert!(hub.is_online("dev"));
        hub.disconnect("dev", "s2");
        assert!(!hub.is_online("dev"));
    }

    #[test]
    fn send_to_offline_is_false() {
        let hub = Hub::new();
        assert!(!hub.send_to("nobody", SignalMessage::Ping { seq: 1 }));
    }
}
