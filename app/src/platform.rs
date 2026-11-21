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

// ───────────────────────── app icon variants ─────────────────────────

/// Icon colors the user can pick from the Dusty → Icon menu. The bundle icon (Finder) stays purple;
/// the choice changes the Dock icon while Dusty runs and is remembered in user defaults.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconVariant {
    Purple,
    Mint,
    Cream,
}

impl IconVariant {
    pub const ALL: [IconVariant; 3] = [IconVariant::Purple, IconVariant::Mint, IconVariant::Cream];
    pub fn key(self) -> &'static str {
        match self {
            IconVariant::Purple => "purple",
            IconVariant::Mint => "mint",
            IconVariant::Cream => "cream",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            IconVariant::Purple => "Purple",
            IconVariant::Mint => "Mint",
            IconVariant::Cream => "Cream",
        }
    }
    fn png(self) -> &'static [u8] {
        match self {
            IconVariant::Purple => include_bytes!("../assets/icons/purple.png"),
            IconVariant::Mint => include_bytes!("../assets/icons/mint.png"),
            IconVariant::Cream => include_bytes!("../assets/icons/cream.png"),
        }
    }
}

const ICON_PREF: &str = "iconVariant";

#[cfg(target_os = "macos")]
fn ns_string(s: &str) -> *mut objc::runtime::Object {
    use objc::{class, msg_send, sel, sel_impl};
    let c = std::ffi::CString::new(s).unwrap();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// The saved icon choice (purple when nothing is saved).
#[cfg(target_os = "macos")]
pub fn saved_icon() -> IconVariant {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let v: *mut Object = msg_send![defaults, stringForKey: ns_string(ICON_PREF)];
        if v.is_null() {
            return IconVariant::Purple;
        }
        let p: *const std::os::raw::c_char = msg_send![v, UTF8String];
        let s = std::ffi::CStr::from_ptr(p).to_string_lossy();
        IconVariant::ALL.into_iter().find(|i| i.key() == s).unwrap_or(IconVariant::Purple)
    }
}

/// Show `icon` in the Dock now and remember it for next launch.
#[cfg(target_os = "macos")]
pub fn apply_icon(icon: IconVariant, save: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let bytes = icon.png();
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: bytes.as_ptr() length: bytes.len()];
        let img: *mut Object = msg_send![class!(NSImage), alloc];
        let img: *mut Object = msg_send![img, initWithData: data];
        if !img.is_null() {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, setApplicationIconImage: img];
            let _: () = msg_send![img, release];
        }
        if save {
            let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
            let _: () = msg_send![defaults, setObject: ns_string(icon.key()) forKey: ns_string(ICON_PREF)];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn saved_icon() -> IconVariant {
    IconVariant::Purple
}

#[cfg(not(target_os = "macos"))]
pub fn apply_icon(_icon: IconVariant, _save: bool) {}
