//! Live update hub: one tokio broadcast channel fans out every recorded
//! change event; SSE subscribers filter by project. The persistent log in
//! SQLite (`events` table) backs `since`-polling for API/MCP clients.

use tokio::sync::broadcast;

use crate::db::Event;

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Event>,
}

impl EventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}
