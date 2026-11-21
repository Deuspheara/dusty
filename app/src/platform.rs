//! macOS bits GPUI 0.2 doesn't expose: dragging the window from our own (taller) top band.

use gpui::*;

/// Start a native window drag from the current mouse-down. AppKit runs its own tracking loop, so
/// this is deferred until GPUI has finished handling the event (re-entrant callbacks would
/// otherwise find the app already borrowed).
#[cfg(target_os = "macos")]
pub fn begin_window_drag(window: &Window, cx: &mut App) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    let view = h.ns_view.as_ptr() as *mut Object;
    // retain the mouse-down event so it outlives this handler
    let event: *mut Object = unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let ev: *mut Object = msg_send![app, currentEvent];
        if ev.is_null() {
            return;
        }
        msg_send![ev, retain]
    };
    let view_addr = view as usize;
    let event_addr = event as usize;
    cx.spawn(async move |_| unsafe {
        let view = view_addr as *mut Object;
        let event = event_addr as *mut Object;
        let win: *mut Object = msg_send![view, window];
        if !win.is_null() {
            let _: () = msg_send![win, performWindowDragWithEvent: event];
        }
        let _: () = msg_send![event, release];
    })
    .detach();
}

#[cfg(not(target_os = "macos"))]
pub fn begin_window_drag(window: &Window, _cx: &mut App) {
    window.start_window_move();
}

/// Trackpad haptics (Force Touch trackpads; no-op elsewhere). Different controls feel different:
/// a firm "level change" for primary actions, a soft tick for toggles and checkboxes.
#[derive(Clone, Copy)]
pub enum Haptic {
    /// soft tick: toggles, checkboxes, chips, disclosure
    Tick,
    /// neutral click: secondary buttons, poking Dusty
    Click,
    /// firm thump: primary actions
    Thump,
    /// the destructive confirm: a firm pulse on release too (press already thumps), so it feels like two
    Double,
}

#[cfg(target_os = "macos")]
pub fn haptic(kind: Haptic) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // NSHapticFeedbackPattern: Generic 0, Alignment 1, LevelChange 2; performance time Now = 1
    let pattern: i64 = match kind {
        Haptic::Tick => 1,
        Haptic::Click => 0,
        Haptic::Thump | Haptic::Double => 2,
    };
    unsafe {
        let performer: *mut Object = msg_send![class!(NSHapticFeedbackManager), defaultPerformer];
        if performer.is_null() {
            return;
        }
        let _: () = msg_send![performer, performFeedbackPattern: pattern performanceTime: 1i64];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn haptic(_kind: Haptic) {}
