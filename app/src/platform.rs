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

/// System Settings → Accessibility → Display → Reduce motion.
#[cfg(target_os = "macos")]
pub fn reduce_motion() -> bool {
    use objc::runtime::{Object, BOOL, NO};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        if ws.is_null() {
            return false;
        }
        let v: BOOL = msg_send![ws, accessibilityDisplayShouldReduceMotion];
        v != NO
    }
}

#[cfg(not(target_os = "macos"))]
pub fn reduce_motion() -> bool {
    false
}

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

// ───────────────────────── Show in Finder ─────────────────────────

/// What Finder should select for a target's paths: the path itself when there's one, all of them when they
/// share a folder, otherwise their closest common folder (or the first path, if that would be your whole home).
pub fn reveal_selection(paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let paths: Vec<_> = paths.iter().filter(|p| p.exists()).cloned().collect();
    if paths.len() <= 1 {
        return paths;
    }
    let parent = |p: &std::path::PathBuf| p.parent().map(|p| p.to_path_buf());
    if paths.iter().all(|p| parent(p) == parent(&paths[0])) {
        return paths;
    }
    let mut common = paths[0].clone();
    while !paths.iter().all(|p| p.starts_with(&common)) {
        if !common.pop() {
            break;
        }
    }
    if common.components().count() <= 1 || common == dusty_core::home() {
        return vec![paths[0].clone()];
    }
    vec![common]
}

/// Open Finder with `paths` selected (see `reveal_selection`). False when there's nothing on disk to show.
/// Finder is asked after GPUI finishes the current event: AppKit can spin a nested run loop while it talks to
/// Finder, and GPUI's timers must not find the app still borrowed by the click handler.
#[cfg(target_os = "macos")]
pub fn reveal_in_finder(paths: &[std::path::PathBuf], cx: &mut App) -> bool {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    let sel = reveal_selection(paths);
    if sel.is_empty() {
        return false;
    }
    cx.spawn(async move |_| unsafe {
        let urls: Vec<*mut Object> = sel.iter().map(|p| msg_send![class!(NSURL), fileURLWithPath: ns_string(&p.to_string_lossy())]).collect();
        let arr: *mut Object = msg_send![class!(NSArray), arrayWithObjects: urls.as_ptr() count: urls.len()];
        let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let _: () = msg_send![ws, activateFileViewerSelectingURLs: arr];
    })
    .detach();
    true
}

#[cfg(not(target_os = "macos"))]
pub fn reveal_in_finder(paths: &[std::path::PathBuf], cx: &mut App) -> bool {
    let sel = reveal_selection(paths);
    if let Some(p) = sel.first() {
        cx.reveal_path(p);
    }
    !sel.is_empty()
}

// ───────────────────────── app icons ─────────────────────────

/// The icon Finder shows for `path` (an app bundle), as PNG bytes about `side` px square. Works for apps that
/// only have an asset-catalog icon too. NSWorkspace and NSImage drawing are fine off the main thread, so the
/// uninstaller loads these on a background thread.
#[cfg(target_os = "macos")]
pub fn app_icon_png(path: &std::path::Path, side: f64) -> Option<Vec<u8>> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::os::raw::c_void;
    #[repr(C)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    objc::rc::autoreleasepool(|| unsafe {
        let ws: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let img: *mut Object = msg_send![ws, iconForFile: ns_string(&path.to_string_lossy())];
        if img.is_null() {
            return None;
        }
        let mut rect = Rect { x: 0., y: 0., w: side, h: side };
        let nil: *mut Object = std::ptr::null_mut();
        let cg: *mut c_void = msg_send![img, CGImageForProposedRect: &mut rect as *mut Rect as *mut c_void context: nil hints: nil];
        if cg.is_null() {
            return None;
        }
        let rep: *mut Object = msg_send![class!(NSBitmapImageRep), alloc];
        let rep: *mut Object = msg_send![rep, initWithCGImage: cg];
        if rep.is_null() {
            return None;
        }
        let props: *mut Object = msg_send![class!(NSDictionary), dictionary];
        // NSBitmapImageFileTypePNG = 4
        let data: *mut Object = msg_send![rep, representationUsingType: 4u64 properties: props];
        let out = if data.is_null() {
            None
        } else {
            let len: usize = msg_send![data, length];
            let bytes: *const u8 = msg_send![data, bytes];
            (!bytes.is_null() && len > 0).then(|| std::slice::from_raw_parts(bytes, len).to_vec())
        };
        let _: () = msg_send![rep, release];
        out
    })
}

#[cfg(not(target_os = "macos"))]
pub fn app_icon_png(_path: &std::path::Path, _side: f64) -> Option<Vec<u8>> {
    None
}

// ───────────────────────── menu bar extra ─────────────────────────

/// What the menu bar extra asked for (read and cleared by the app's poll loop).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusRequest {
    None,
    Open,
    CheckUp,
    Quit,
}

static STATUS_REQUEST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// The NSStatusItem, retained while it's shown (0 = hidden).
#[cfg(target_os = "macos")]
static STATUS_ITEM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn take_status_request() -> StatusRequest {
    match STATUS_REQUEST.swap(0, std::sync::atomic::Ordering::Relaxed) {
        1 => StatusRequest::CheckUp,
        2 => StatusRequest::Quit,
        3 => StatusRequest::Open,
        _ => StatusRequest::None,
    }
}

const MENU_BAR_PREF: &str = "menuBarExtra";

/// Whether the 🐾 free-space item shows in the menu bar: on unless turned off in Dusty → Show in Menu Bar
/// (saved in user defaults); `DUSTY_MENU_BAR=0|1` overrides it.
#[cfg(target_os = "macos")]
pub fn menu_bar_enabled() -> bool {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    match std::env::var("DUSTY_MENU_BAR").as_deref() {
        Ok("0") => return false,
        Ok("1") => return true,
        _ => {}
    }
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let v: *mut Object = msg_send![defaults, objectForKey: ns_string(MENU_BAR_PREF)];
        if v.is_null() {
            return true;
        }
        let b: objc::runtime::BOOL = msg_send![v, boolValue];
        b != objc::runtime::NO
    }
}

#[cfg(target_os = "macos")]
pub fn set_menu_bar_enabled(on: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let v: objc::runtime::BOOL = if on { objc::runtime::YES } else { objc::runtime::NO };
        let _: () = msg_send![defaults, setBool: v forKey: ns_string(MENU_BAR_PREF)];
    }
}

/// Un-hide Dusty and un-minimize its window; the app then activates the window itself (GPUI side).
#[cfg(target_os = "macos")]
fn bring_to_front() {
    use objc::runtime::{Object, NO};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, unhide: std::ptr::null_mut::<Object>()];
        let windows: *mut Object = msg_send![app, windows];
        let n: usize = msg_send![windows, count];
        for i in 0..n {
            let w: *mut Object = msg_send![windows, objectAtIndex: i];
            let mini: objc::runtime::BOOL = msg_send![w, isMiniaturized];
            if mini != NO {
                let _: () = msg_send![w, deminiaturize: std::ptr::null_mut::<Object>()];
            }
        }
    }
}

/// The target the status menu's items call into.
#[cfg(target_os = "macos")]
fn status_target() -> *mut objc::runtime::Object {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    static TARGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    extern "C" fn open(_: &Object, _: Sel, _: *mut Object) {
        bring_to_front();
        STATUS_REQUEST.store(3, std::sync::atomic::Ordering::Relaxed);
    }
    extern "C" fn check(_: &Object, _: Sel, _: *mut Object) {
        bring_to_front();
        STATUS_REQUEST.store(1, std::sync::atomic::Ordering::Relaxed);
    }
    extern "C" fn quit(_: &Object, _: Sel, _: *mut Object) {
        STATUS_REQUEST.store(2, std::sync::atomic::Ordering::Relaxed);
    }
    *TARGET.get_or_init(|| unsafe {
        let cls: &Class = match Class::get("DustyStatusTarget") {
            Some(c) => c,
            None => {
                let mut d = ClassDecl::new("DustyStatusTarget", class!(NSObject)).expect("declare DustyStatusTarget");
                d.add_method(sel!(openDusty:), open as extern "C" fn(&Object, Sel, *mut Object));
                d.add_method(sel!(checkUp:), check as extern "C" fn(&Object, Sel, *mut Object));
                d.add_method(sel!(quitDusty:), quit as extern "C" fn(&Object, Sel, *mut Object));
                d.register()
            }
        };
        let obj: *mut Object = msg_send![cls, new];
        obj as usize
    }) as *mut Object
}

/// Show (or hide) the menu bar extra: "🐾 124 GB", with Open Dusty / Check up now / Quit.
#[cfg(target_os = "macos")]
pub fn status_item_show(on: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::sync::atomic::Ordering;
    unsafe {
        let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
        let cur = STATUS_ITEM.load(Ordering::Relaxed) as *mut Object;
        if !on {
            if !cur.is_null() {
                let _: () = msg_send![bar, removeStatusItem: cur];
                let _: () = msg_send![cur, release];
                STATUS_ITEM.store(0, Ordering::Relaxed);
            }
            return;
        }
        if !cur.is_null() {
            return;
        }
        // NSVariableStatusItemLength
        let item: *mut Object = msg_send![bar, statusItemWithLength: -1.0f64];
        let item: *mut Object = msg_send![item, retain];
        let target = status_target();
        let menu: *mut Object = msg_send![class!(NSMenu), new];
        let entry = |title: &str, action: objc::runtime::Sel, key: &str| {
            let mi: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let mi: *mut Object = msg_send![mi, initWithTitle: ns_string(title) action: action keyEquivalent: ns_string(key)];
            let _: () = msg_send![mi, setTarget: target];
            let _: () = msg_send![menu, addItem: mi];
            let _: () = msg_send![mi, release];
        };
        entry("Open Dusty", sel!(openDusty:), "");
        entry("Check up now", sel!(checkUp:), "");
        let sep: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: sep];
        entry("Quit Dusty", sel!(quitDusty:), "");
        let _: () = msg_send![item, setMenu: menu];
        let _: () = msg_send![menu, release];
        STATUS_ITEM.store(item as usize, Ordering::Relaxed);
    }
}

/// Set the menu bar extra's text (no-op while it's hidden).
#[cfg(target_os = "macos")]
pub fn status_item_title(title: &str) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    let item = STATUS_ITEM.load(std::sync::atomic::Ordering::Relaxed) as *mut Object;
    if item.is_null() {
        return;
    }
    unsafe {
        let button: *mut Object = msg_send![item, button];
        if !button.is_null() {
            let _: () = msg_send![button, setTitle: ns_string(title)];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn menu_bar_enabled() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn set_menu_bar_enabled(_on: bool) {}
#[cfg(not(target_os = "macos"))]
pub fn status_item_show(_on: bool) {}
#[cfg(not(target_os = "macos"))]
pub fn status_item_title(_title: &str) {}

#[cfg(test)]
mod tests {
    use super::reveal_selection;

    #[test]
    fn reveal_picks_what_finder_should_select() {
        let d = std::env::temp_dir().join(format!("dusty-reveal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["a/x/build", "a/x/dist", "a/y/build", "b"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        let j = |p: &str| d.join(p);
        // one path: itself; missing paths are dropped
        assert_eq!(reveal_selection(&[j("b"), j("gone")]), vec![j("b")]);
        assert!(reveal_selection(&[j("gone")]).is_empty());
        // same folder: select them all
        assert_eq!(reveal_selection(&[j("a/x/build"), j("a/x/dist")]), vec![j("a/x/build"), j("a/x/dist")]);
        // different folders: their closest common folder
        assert_eq!(reveal_selection(&[j("a/x/build"), j("a/y/build")]), vec![j("a")]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
