//! Automatic updates through Sparkle 2, drawn with Dusty's own UI.
//!
//! Sparkle does the hard parts: reading the feed, checking the EdDSA signature, downloading, installing and
//! relaunching. Instead of its stock windows we give it a *user driver*, an Objective-C class declared here.
//! Sparkle calls it to say what should be on screen; we record that in `STATE`, and the GPUI view (which polls
//! `generation()`) draws the update sheet. The user's answer goes back through the reply block Sparkle gave us.
//!
//! `bundle.sh` embeds Sparkle.framework in Contents/Frameworks. It is loaded at runtime rather than linked, so
//! `cargo run` (no bundle, no framework) still works; updates are then simply unavailable.
//!
//! Testing hooks (like DUSTY_APPEARANCE elsewhere):
//!   DUSTY_UPDATE_DEMO=checking|found|found-beta|downloading|extracting|ready|installing|uptodate|error
//!                     show one state without Sparkle; add -hidden to start with the sheet away (the pill)
//!   DUSTY_UPDATE_FEED=<url>   use another appcast (e.g. a local test feed)
//!   DUSTY_UPDATE_CHECK=1      check right after launch, as if the menu item was chosen

use std::sync::Mutex;

/// What the update UI should show.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    /// a check the user asked for is running
    Checking,
    /// a newer version exists; waiting for the user's choice
    Found(Found),
    Downloading { received: u64, expected: u64 },
    Extracting(f64),
    /// downloaded and unpacked; installing quits and relaunches Dusty
    Ready,
    Installing,
    /// the check the user asked for found nothing newer
    UpToDate,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub version: String,
    pub size: u64,
    /// a few release-note lines, plain text
    pub notes: Vec<String>,
    pub beta: bool,
    pub critical: bool,
    /// already downloaded earlier (Sparkle resumes it): installing is instant
    pub downloaded: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub phase: Phase,
    /// whether the update sheet should be up. Background finds only raise the "Update" pill.
    pub open: bool,
}

/// What the user did in the update UI.
#[derive(Clone, Copy, Debug)]
pub enum Action {
    /// "Update now" / "Install and relaunch"
    Install,
    /// "Later": keep the update for another time (a ready update then installs when Dusty quits)
    Later,
    /// "Skip this version"
    Skip,
    /// stop a check or a download
    Cancel,
    /// close an "up to date" / error message
    Acknowledge,
    /// put the sheet away; whatever is running keeps going and the pill shows it
    Hide,
    /// bring the sheet back from the pill
    Show,
}

struct State {
    phase: Phase,
    open: bool,
    generation: u64,
    /// the user already said "Update now" in this session: don't ask again once it's downloaded
    install_chosen: bool,
}

static STATE: Mutex<State> = Mutex::new(State { phase: Phase::Idle, open: false, generation: 0, install_chosen: false });

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let r = f(&mut s);
    s.generation += 1;
    r
}

/// Changes whenever the update UI should redraw.
pub fn generation() -> u64 {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).generation
}

pub fn snapshot() -> Snapshot {
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    Snapshot { phase: s.phase.clone(), open: s.open }
}

/// The version this build was made from (Cargo.toml), e.g. "0.1.0-beta.1".
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "0.2.0-beta.2" → "0.2.0 beta 2", for headlines.
pub fn pretty(v: &str) -> String {
    match v.split_once('-') {
        Some((base, pre)) => format!("{base} {}", pre.replace('.', " ")),
        None => v.to_string(),
    }
}

/// Pre-release builds follow the beta channel unless the user turned it off.
pub fn is_prerelease() -> bool {
    current_version().contains('-')
}

/// Turn release notes (HTML or the Markdown GitHub generates) into a few plain lines.
pub fn note_lines(raw: &str) -> Vec<String> {
    let mut text = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                text.push('\n');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("**Full Changelog") && !l.starts_with("<!--"))
        .map(|l| {
            let l = l.trim_start_matches(['*', '-', '•']).trim();
            // GitHub's "Fix thing by @someone in https://…/pull/12"
            let l = l.split(" by @").next().unwrap_or(l);
            let l = l.split(" in https://").next().unwrap_or(l);
            // [text](url) → text
            let mut out = String::new();
            let mut rest = l;
            while let Some(a) = rest.find('[') {
                let Some(b) = rest[a..].find("](").map(|i| a + i) else { break };
                let Some(c) = rest[b..].find(')').map(|i| b + i) else { break };
                out.push_str(&rest[..a]);
                out.push_str(&rest[a + 1..b]);
                rest = &rest[c + 1..];
            }
            out.push_str(rest);
            out.replace("**", "").replace('`', "")
        })
        .filter(|l| l.len() > 1)
        .take(5)
        .collect()
}

fn demo_phase(name: &str) -> Option<Phase> {
    let found = Found {
        version: "0.2.0".into(),
        size: 9_400_000,
        notes: vec!["Faster checkups on big home folders".into(), "Dusty naps when the window is hidden".into(), "Fixed Docker sizes on Apple silicon".into()],
        beta: false,
        critical: false,
        downloaded: false,
    };
    Some(match name {
        "checking" => Phase::Checking,
        "found" => Phase::Found(found),
        "found-beta" => Phase::Found(Found { version: "0.2.0-beta.2".into(), beta: true, ..found }),
        "downloading" => Phase::Downloading { received: 3_900_000, expected: 9_400_000 },
        "extracting" => Phase::Extracting(0.6),
        "ready" => Phase::Ready,
        "installing" => Phase::Installing,
        "uptodate" => Phase::UpToDate,
        "error" => Phase::Failed("The update server couldn't be reached. Check your connection and try again.".into()),
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Protocol, Sel, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{c_void, CStr, CString};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// The SPUUpdater, retained for the life of the app (0 = updates unavailable).
    static UPDATER: AtomicUsize = AtomicUsize::new(0);
    static DEMO: AtomicBool = AtomicBool::new(false);

    // ── Objective-C blocks ──
    // Sparkle hands us reply blocks; we copy them to the heap, keep them until the user answers, call them once.

    #[repr(C)]
    struct BlockLiteral {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: *const c_void,
    }

    extern "C" {
        fn _Block_copy(block: *const c_void) -> *mut c_void;
        fn _Block_release(block: *const c_void);
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        /// void (^)(SPUUserUpdateChoice)
        Choice,
        /// void (^)(void)
        Void,
    }

    struct Pending {
        reply: Option<(usize, Kind)>,
        cancel: Option<usize>,
    }
    static PENDING: Mutex<Pending> = Mutex::new(Pending { reply: None, cancel: None });

    fn pending() -> std::sync::MutexGuard<'static, Pending> {
        PENDING.lock().unwrap_or_else(|e| e.into_inner())
    }

    unsafe fn keep(block: *mut c_void) -> usize {
        if block.is_null() {
            0
        } else {
            _Block_copy(block) as usize
        }
    }

    unsafe fn drop_block(b: usize) {
        if b != 0 {
            _Block_release(b as *const c_void);
        }
    }

    fn set_reply(block: *mut c_void, kind: Kind) {
        let new = unsafe { keep(block) };
        if let Some((old, _)) = pending().reply.replace((new, kind)) {
            unsafe { drop_block(old) };
        }
    }

    fn set_cancel(block: *mut c_void) {
        let new = unsafe { keep(block) };
        if let Some(old) = pending().cancel.replace(new) {
            unsafe { drop_block(old) };
        }
    }

    fn clear_blocks() {
        let (r, c) = {
            let mut p = pending();
            (p.reply.take(), p.cancel.take())
        };
        unsafe {
            if let Some((b, _)) = r {
                drop_block(b);
            }
            if let Some(b) = c {
                drop_block(b);
            }
        }
    }

    /// Call (and release) the pending reply. The lock is not held while Sparkle runs, because it may call
    /// straight back into the driver.
    fn answer(choice: i64) {
        let Some((b, kind)) = pending().reply.take() else { return };
        if b == 0 {
            return;
        }
        unsafe {
            let blk = b as *mut BlockLiteral;
            match kind {
                Kind::Choice => {
                    let f: extern "C" fn(*mut BlockLiteral, i64) = std::mem::transmute((*blk).invoke);
                    f(blk, choice);
                }
                Kind::Void => {
                    let f: extern "C" fn(*mut BlockLiteral) = std::mem::transmute((*blk).invoke);
                    f(blk);
                }
            }
            drop_block(b);
        }
    }

    fn cancel_now() {
        let Some(b) = pending().cancel.take() else { return };
        if b == 0 {
            return;
        }
        unsafe {
            let blk = b as *mut BlockLiteral;
            let f: extern "C" fn(*mut BlockLiteral) = std::mem::transmute((*blk).invoke);
            f(blk);
            drop_block(b);
        }
    }

    // SPUUserUpdateChoice
    const SKIP: i64 = 0;
    const INSTALL: i64 = 1;
    const DISMISS: i64 = 2;

    // ── Foundation helpers ──

    unsafe fn nsstring(s: &str) -> *mut Object {
        let c = CString::new(s).unwrap_or_default();
        msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
    }

    unsafe fn rust_string(ns: *mut Object) -> Option<String> {
        if ns.is_null() {
            return None;
        }
        let p: *const std::os::raw::c_char = msg_send![ns, UTF8String];
        if p.is_null() {
            return None;
        }
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }

    unsafe fn error_text(err: *mut Object) -> String {
        if err.is_null() {
            return "Something went wrong.".into();
        }
        let d: *mut Object = msg_send![err, localizedDescription];
        rust_string(d).unwrap_or_else(|| "Something went wrong.".into())
    }

    fn defaults() -> *mut Object {
        unsafe { msg_send![class!(NSUserDefaults), standardUserDefaults] }
    }

    const BETA_KEY: &str = "DustyBetaUpdates";
    const LAST_VERSION_KEY: &str = "DustyLastRunVersion";

    pub fn beta_enabled() -> bool {
        unsafe {
            let d = defaults();
            let v: *mut Object = msg_send![d, objectForKey: nsstring(BETA_KEY)];
            if v.is_null() {
                return is_prerelease();
            }
            let on: BOOL = msg_send![d, boolForKey: nsstring(BETA_KEY)];
            on != NO
        }
    }

    pub fn set_beta(on: bool) {
        unsafe {
            let _: () = msg_send![defaults(), setBool: if on { YES } else { NO } forKey: nsstring(BETA_KEY)];
        }
    }

    /// The version Dusty was updated to, the first time it runs after an update.
    pub fn just_updated() -> Option<String> {
        unsafe {
            let d = defaults();
            let last: *mut Object = msg_send![d, stringForKey: nsstring(LAST_VERSION_KEY)];
            let last = rust_string(last);
            let _: () = msg_send![d, setObject: nsstring(current_version()) forKey: nsstring(LAST_VERSION_KEY)];
            match last {
                Some(v) if v != current_version() => Some(current_version().to_string()),
                _ => None,
            }
        }
    }

    // ── the user driver: Sparkle → STATE ──

    extern "C" fn permission(_: &Object, _: Sel, _req: *mut Object, reply: *mut c_void) {
        // automatic checks are on by default (SUEnableAutomaticChecks), so this is rare: say yes, send nothing
        unsafe {
            let cls = Class::get("SUUpdatePermissionResponse").expect("Sparkle loaded");
            let r: *mut Object = msg_send![cls, alloc];
            let r: *mut Object = msg_send![r, initWithAutomaticUpdateChecks: YES sendSystemProfile: NO];
            let r: *mut Object = msg_send![r, autorelease];
            let blk = reply as *mut BlockLiteral;
            let f: extern "C" fn(*mut BlockLiteral, *mut Object) = std::mem::transmute((*blk).invoke);
            f(blk, r);
        }
    }

    extern "C" fn user_check(_: &Object, _: Sel, cancel: *mut c_void) {
        set_cancel(cancel);
        with_state(|s| {
            s.phase = Phase::Checking;
            s.open = true;
            s.install_chosen = false;
        });
    }

    extern "C" fn found(_: &Object, _: Sel, item: *mut Object, state: *mut Object, reply: *mut c_void) {
        set_reply(reply, Kind::Choice);
        unsafe {
            let version: *mut Object = msg_send![item, displayVersionString];
            let size: u64 = msg_send![item, contentLength];
            let desc: *mut Object = msg_send![item, itemDescription];
            let channel: *mut Object = msg_send![item, channel];
            let critical: BOOL = msg_send![item, isCriticalUpdate];
            let stage: i64 = msg_send![state, stage];
            let user: BOOL = msg_send![state, userInitiated];
            let version = rust_string(version).unwrap_or_default();
            let beta = rust_string(channel).is_some_and(|c| c == "beta") || version.contains('-');
            let f = Found { notes: rust_string(desc).map(|d| note_lines(&d)).unwrap_or_default(), version, size, beta, critical: critical != NO, downloaded: stage != 0 };
            with_state(|s| {
                s.open = s.open || user != NO || stage == 2;
                s.phase = Phase::Found(f);
            });
        }
    }

    extern "C" fn release_notes(_: &Object, _: Sel, _data: *mut Object) {}
    extern "C" fn release_notes_failed(_: &Object, _: Sel, _err: *mut Object) {}

    extern "C" fn not_found(_: &Object, _: Sel, _err: *mut Object, ack: *mut c_void) {
        set_reply(ack, Kind::Void);
        with_state(|s| {
            s.phase = Phase::UpToDate;
            s.open = true;
        });
    }

    extern "C" fn updater_error(_: &Object, _: Sel, err: *mut Object, ack: *mut c_void) {
        set_reply(ack, Kind::Void);
        let msg = unsafe { error_text(err) };
        with_state(|s| {
            s.phase = Phase::Failed(msg);
            s.open = true;
        });
    }

    extern "C" fn download_started(_: &Object, _: Sel, cancel: *mut c_void) {
        set_cancel(cancel);
        with_state(|s| s.phase = Phase::Downloading { received: 0, expected: 0 });
    }

    extern "C" fn download_length(_: &Object, _: Sel, expected: u64) {
        with_state(|s| {
            if let Phase::Downloading { received, .. } = s.phase {
                s.phase = Phase::Downloading { received, expected };
            }
        });
    }

    extern "C" fn download_data(_: &Object, _: Sel, len: u64) {
        with_state(|s| {
            if let Phase::Downloading { received, expected } = s.phase {
                // servers sometimes don't announce a size: let the bar track what arrived
                let received = received + len;
                s.phase = Phase::Downloading { received, expected: expected.max(received) };
            }
        });
    }

    extern "C" fn extracting(_: &Object, _: Sel) {
        clear_cancel_only();
        with_state(|s| s.phase = Phase::Extracting(0.));
    }

    extern "C" fn extract_progress(_: &Object, _: Sel, p: f64) {
        with_state(|s| s.phase = Phase::Extracting(p.clamp(0., 1.)));
    }

    extern "C" fn ready(_: &Object, _: Sel, reply: *mut c_void) {
        set_reply(reply, Kind::Choice);
        let go_on = with_state(|s| {
            if s.install_chosen {
                s.phase = Phase::Installing;
                true
            } else {
                s.phase = Phase::Ready;
                false
            }
        });
        // they already said "Update now": no second question once it's downloaded
        if go_on {
            answer(INSTALL);
        }
    }

    extern "C" fn installing(_: &Object, _: Sel, _terminated: BOOL, _retry: *mut c_void) {
        with_state(|s| s.phase = Phase::Installing);
    }

    extern "C" fn installed(_: &Object, _: Sel, _relaunched: BOOL, ack: *mut c_void) {
        // shown by the new copy at launch instead (see `just_updated`); just let Sparkle carry on
        set_reply(ack, Kind::Void);
        answer(0);
    }

    extern "C" fn in_focus(_: &Object, _: Sel) {
        with_state(|s| s.open = !matches!(s.phase, Phase::Idle));
    }

    extern "C" fn dismiss(_: &Object, _: Sel) {
        clear_blocks();
        with_state(|s| {
            s.phase = Phase::Idle;
            s.open = false;
            s.install_chosen = false;
        });
    }

    fn clear_cancel_only() {
        if let Some(b) = pending().cancel.take() {
            unsafe { drop_block(b) };
        }
    }

    // ── the updater delegate: channels and the test feed ──

    extern "C" fn allowed_channels(_: &Object, _: Sel, _updater: *mut Object) -> *mut Object {
        unsafe {
            if beta_enabled() {
                msg_send![class!(NSSet), setWithObject: nsstring("beta")]
            } else {
                msg_send![class!(NSSet), set]
            }
        }
    }

    extern "C" fn feed_url(_: &Object, _: Sel, _updater: *mut Object) -> *mut Object {
        match std::env::var("DUSTY_UPDATE_FEED") {
            Ok(u) if !u.is_empty() => unsafe { nsstring(&u) },
            _ => std::ptr::null_mut(),
        }
    }

    fn declare_classes() -> Option<(&'static Class, &'static Class)> {
        type Id = *mut Object;
        type Blk = *mut c_void;
        unsafe {
            let mut d = ClassDecl::new("DustyUpdateDriver", class!(NSObject))?;
            if let Some(p) = Protocol::get("SPUUserDriver") {
                d.add_protocol(p);
            }
            d.add_method(sel!(showUpdatePermissionRequest:reply:), permission as extern "C" fn(&Object, Sel, Id, Blk));
            d.add_method(sel!(showUserInitiatedUpdateCheckWithCancellation:), user_check as extern "C" fn(&Object, Sel, Blk));
            d.add_method(sel!(showUpdateFoundWithAppcastItem:state:reply:), found as extern "C" fn(&Object, Sel, Id, Id, Blk));
            d.add_method(sel!(showUpdateReleaseNotesWithDownloadData:), release_notes as extern "C" fn(&Object, Sel, Id));
            d.add_method(sel!(showUpdateReleaseNotesFailedToDownloadWithError:), release_notes_failed as extern "C" fn(&Object, Sel, Id));
            d.add_method(sel!(showUpdateNotFoundWithError:acknowledgement:), not_found as extern "C" fn(&Object, Sel, Id, Blk));
            d.add_method(sel!(showUpdaterError:acknowledgement:), updater_error as extern "C" fn(&Object, Sel, Id, Blk));
            d.add_method(sel!(showDownloadInitiatedWithCancellation:), download_started as extern "C" fn(&Object, Sel, Blk));
            d.add_method(sel!(showDownloadDidReceiveExpectedContentLength:), download_length as extern "C" fn(&Object, Sel, u64));
            d.add_method(sel!(showDownloadDidReceiveDataOfLength:), download_data as extern "C" fn(&Object, Sel, u64));
            d.add_method(sel!(showDownloadDidStartExtractingUpdate), extracting as extern "C" fn(&Object, Sel));
            d.add_method(sel!(showExtractionReceivedProgress:), extract_progress as extern "C" fn(&Object, Sel, f64));
            d.add_method(sel!(showReadyToInstallAndRelaunch:), ready as extern "C" fn(&Object, Sel, Blk));
            d.add_method(sel!(showInstallingUpdateWithApplicationTerminated:retryTerminatingApplication:), installing as extern "C" fn(&Object, Sel, BOOL, Blk));
            d.add_method(sel!(showUpdateInstalledAndRelaunched:acknowledgement:), installed as extern "C" fn(&Object, Sel, BOOL, Blk));
            d.add_method(sel!(showUpdateInFocus), in_focus as extern "C" fn(&Object, Sel));
            d.add_method(sel!(dismissUpdateInstallation), dismiss as extern "C" fn(&Object, Sel));
            let driver = d.register();

            let mut g = ClassDecl::new("DustyUpdaterDelegate", class!(NSObject))?;
            if let Some(p) = Protocol::get("SPUUpdaterDelegate") {
                g.add_protocol(p);
            }
            g.add_method(sel!(allowedChannelsForUpdater:), allowed_channels as extern "C" fn(&Object, Sel, Id) -> Id);
            g.add_method(sel!(feedURLStringForUpdater:), feed_url as extern "C" fn(&Object, Sel, Id) -> Id);
            Some((driver, g.register()))
        }
    }

    /// Load Sparkle and start the updater (it schedules its own daily background checks).
    /// Call once, on the main thread, after the app has launched.
    pub fn start() {
        if UPDATER.load(Ordering::Relaxed) != 0 || DEMO.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(demo) = std::env::var("DUSTY_UPDATE_DEMO") {
            // "<state>-hidden" starts with the sheet put away, to see the pill
            let (name, hidden) = match demo.strip_suffix("-hidden") {
                Some(n) => (n, true),
                None => (demo.as_str(), false),
            };
            if let Some(p) = demo_phase(name) {
                DEMO.store(true, Ordering::Relaxed);
                with_state(|s| {
                    s.phase = p;
                    s.open = !hidden;
                });
                return;
            }
        }
        unsafe {
            let main: *mut Object = msg_send![class!(NSBundle), mainBundle];
            let frameworks: *mut Object = msg_send![main, privateFrameworksPath];
            if frameworks.is_null() {
                return;
            }
            let path: *mut Object = msg_send![frameworks, stringByAppendingPathComponent: nsstring("Sparkle.framework")];
            let bundle: *mut Object = msg_send![class!(NSBundle), bundleWithPath: path];
            if bundle.is_null() {
                return; // not running from a bundle that embeds Sparkle (e.g. `cargo run`)
            }
            let loaded: BOOL = msg_send![bundle, load];
            if loaded == NO {
                eprintln!("dusty: could not load Sparkle.framework; automatic updates are off");
                return;
            }
            let Some(updater_cls) = Class::get("SPUUpdater") else { return };
            let Some((driver_cls, delegate_cls)) = declare_classes() else { return };
            let driver: *mut Object = msg_send![driver_cls, new];
            let delegate: *mut Object = msg_send![delegate_cls, new];
            let u: *mut Object = msg_send![updater_cls, alloc];
            let u: *mut Object = msg_send![u, initWithHostBundle: main applicationBundle: main userDriver: driver delegate: delegate];
            if u.is_null() {
                return;
            }
            let mut err: *mut Object = std::ptr::null_mut();
            let ok: BOOL = msg_send![u, startUpdater: &mut err];
            if ok == NO {
                eprintln!("dusty: updater didn't start: {}", error_text(err));
                return;
            }
            // driver and delegate stay alive with the updater, for the life of the app
            UPDATER.store(u as usize, Ordering::Relaxed);
        }
        if std::env::var("DUSTY_UPDATE_CHECK").as_deref() == Ok("1") {
            check_now();
        }
    }

    pub fn available() -> bool {
        UPDATER.load(Ordering::Relaxed) != 0 || DEMO.load(Ordering::Relaxed)
    }

    /// "Check for Updates…". If an update is already on its way, Sparkle just brings it back into focus.
    pub fn check_now() {
        let u = UPDATER.load(Ordering::Relaxed) as *mut Object;
        if u.is_null() {
            with_state(|s| s.open = !matches!(s.phase, Phase::Idle));
            return;
        }
        unsafe {
            let _: () = msg_send![u, checkForUpdates];
        }
    }

    pub fn act(a: Action) {
        if DEMO.load(Ordering::Relaxed) {
            return demo_act(a);
        }
        let phase = snapshot().phase;
        match a {
            Action::Show => with_state(|s| s.open = true),
            Action::Hide => with_state(|s| s.open = false),
            Action::Install => {
                with_state(|s| {
                    s.install_chosen = true;
                    s.open = true;
                });
                answer(INSTALL);
            }
            Action::Later => {
                with_state(|s| s.open = false);
                answer(DISMISS);
            }
            Action::Skip => {
                with_state(|s| s.open = false);
                answer(SKIP);
            }
            Action::Cancel => {
                with_state(|s| s.open = false);
                cancel_now();
                // a cancelled check ends the session on its own; make sure the sheet doesn't linger
                if matches!(phase, Phase::Checking) {
                    with_state(|s| s.phase = Phase::Idle);
                }
            }
            Action::Acknowledge => {
                with_state(|s| s.open = false);
                answer(0);
            }
        }
    }

    /// In demo mode buttons walk through the states, so the whole flow can be looked at without a feed.
    fn demo_act(a: Action) {
        with_state(|s| {
            s.phase = match (a, &s.phase) {
                (Action::Show, p) => {
                    s.open = true;
                    p.clone()
                }
                (Action::Hide, p) => {
                    s.open = false;
                    p.clone()
                }
                (Action::Install, Phase::Found(_)) => Phase::Downloading { received: 3_900_000, expected: 9_400_000 },
                (Action::Install, Phase::Ready) => Phase::Installing,
                (Action::Cancel | Action::Later | Action::Skip | Action::Acknowledge, _) => {
                    s.open = false;
                    Phase::Idle
                }
                (_, p) => p.clone(),
            };
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Action;
    pub fn start() {}
    pub fn available() -> bool {
        false
    }
    pub fn check_now() {}
    pub fn act(_: Action) {}
    pub fn beta_enabled() -> bool {
        false
    }
    pub fn set_beta(_: bool) {}
    pub fn just_updated() -> Option<String> {
        None
    }
}

pub use imp::{act, available, beta_enabled, check_now, just_updated, set_beta, start};

#[cfg(test)]
mod tests {
    use super::note_lines;

    #[test]
    fn github_generated_notes_become_plain_lines() {
        let md = "## What's Changed\n* Faster scans by @someone in https://github.com/x/y/pull/3\n* Fix [Docker](https://docker.com) sizes\n\n**Full Changelog**: https://github.com/x/y/compare/a...b";
        assert_eq!(note_lines(md), vec!["Faster scans", "Fix Docker sizes"]);
    }

    #[test]
    fn prerelease_versions_read_nicely() {
        assert_eq!(super::pretty("0.2.0-beta.2"), "0.2.0 beta 2");
        assert_eq!(super::pretty("1.0.0"), "1.0.0");
    }

    #[test]
    fn html_notes_lose_their_tags() {
        let html = "<h2>New</h2><ul><li>Dark mode</li><li>Tom &amp; Jerry</li></ul>";
        assert_eq!(note_lines(html), vec!["New", "Dark mode", "Tom & Jerry"]);
    }
}
