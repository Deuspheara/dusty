//! Wakes the UI when AppKit callbacks (menu bar extra, Sparkle) have news, so the window doesn't poll for it.

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use std::sync::Mutex;

static TX: Mutex<Option<UnboundedSender<()>>> = Mutex::new(None);

/// Tell the UI something changed. Safe from any thread; a no-op while no UI listens.
pub fn ping() {
    if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.unbounded_send(());
    }
}

/// The UI's end of the channel; a newer listener replaces the old one.
pub fn listen() -> UnboundedReceiver<()> {
    let (tx, rx) = unbounded();
    *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    rx
}
