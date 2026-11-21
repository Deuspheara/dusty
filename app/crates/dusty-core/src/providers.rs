//! Finders written in Rust, for targets a list of paths can't describe. The catalog names them
//! (`provider = "chromium"`, with `args`); each one only finds things. Labels, groups, guards and
//! safety stay in the catalog and in `clean`.
//!
//! Adding one: write a function below, add it to [`provider`], and use its name in `catalog/system.toml`.

use super::catalog::{Found, ScanCtx};
use super::*;

/// How a provider answers.
#[derive(Clone, Copy)]
pub(crate) enum Provider {
    /// Paths to measure and offer, like a `paths` list worked out at scan time.
    Paths(fn(&ScanCtx, &[String]) -> Vec<PathBuf>),
    /// A finished finding: usually commands (`docker … prune`, `simctl delete`) with a size estimate.
    Found(fn(&ScanCtx, &[String]) -> Option<Found>),
}

/// Every provider the catalog can name.
pub(crate) fn provider(name: &str) -> Option<Provider> {
    use Provider::{Found as F, Paths as P};
    Some(match name {
        // paths
        "android-old-sdk" => P(android_old_sdk),
        "android-unused-images" => P(android_unused_images),
        "apple-media" => P(apple_media),
        "avd-snapshots" => P(avd_snapshots),
        "avd-userdata" => P(avd_userdata),
        "chromium" => P(chromium_caches),
        "codex-install-leftovers" => P(codex_install_leftovers),
        "editor" => P(editor),
        "electron-app" => P(electron_app_caches),
        "electron-caches" => P(electron_caches),
        "esp-old-toolchains" => P(esp_old_toolchains),
        "fvm-unused" => P(fvm_unused),
        "jetbrains-current" => P(jetbrains_current),
        "library-caches" => P(library_caches),
        "old-device-logs" => P(old_device_logs),
        "old-ide-versions" => P(old_ide_versions),
        "xcode-old-archives" => P(xcode_old_archives),
        // commands
        "docker" => F(docker),
        "homebrew" => F(homebrew),
        "rust-old-toolchains" => F(rust_old_toolchains),
        "sim-erase" => F(sim_erase),
        "sim-runtimes-old" => F(sim_runtimes_old),
        "sim-unavailable" => F(sim_unavailable),
        _ => return None,
    })
}

// ───────────────────────── paths ─────────────────────────

fn android_old_sdk(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let mut v = all_but_newest(&h("Library/Android/sdk/ndk"), 1);
    v.extend(all_but_newest(&h("Library/Android/sdk/build-tools"), 2));
    v
}

fn android_unused_images(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let mut used = HashSet::new();
    for avd in list_dir(&h(".android/avd")) {
        if let Ok(cfg) = std::fs::read_to_string(avd.join("config.ini")) {
            for line in cfg.lines() {
                if let Some(rest) = line.split_once('=').filter(|(k, _)| k.trim() == "image.sysdir.1").map(|(_, v)| v.trim()) {
                    if let Some(img) = rest.strip_prefix("system-images/").and_then(|r| r.split('/').next()) {
                        used.insert(img.to_string());
                    }
                }
            }
        }
    }
    list_dir(&h("Library/Android/sdk/system-images")).into_iter().filter(|p| !used.contains(&name_of(p))).collect()
}

/// Music and TV caches' contents, plus Podcasts' streamed episodes.
fn apple_media(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let mut v = vec![h("Library/Caches/com.apple.Music"), h("Library/Caches/com.apple.podcasts"), h("Library/Caches/com.apple.TV")];
    v.extend(list_dir(&h("Library/Containers/com.apple.podcasts/Data/tmp/StreamedMedia")));
    v.into_iter().flat_map(|p| if p.ends_with("StreamedMedia") { vec![p] } else { list_dir(&p) }).collect()
}

fn avd_snapshots(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h(".android/avd")).into_iter().filter(|p| p.to_string_lossy().ends_with(".avd")).map(|a| a.join("snapshots")).collect()
}

fn avd_userdata(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h(".android/avd")).into_iter().map(|a| a.join("userdata-qemu.img.qcow2")).filter(|p| exists(p)).collect()
}

/// args: the browser's Application Support folder, then its ~/Library/Caches folder.
fn chromium_caches(_: &ScanCtx, args: &[String]) -> Vec<PathBuf> {
    match args {
        [support, caches] => chromium(support, caches),
        _ => vec![],
    }
}

fn codex_install_leftovers(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h(".cache/codex-runtimes")).into_iter().filter(|d| name_of(d).starts_with("codex-runtime-install-") && older_than(d, 1)).collect()
}

/// args: the editor's folders in ~/Library/Application Support.
fn editor(_: &ScanCtx, folders: &[String]) -> Vec<PathBuf> {
    folders.iter().flat_map(|f| EDITOR_CACHE_DIRS.iter().flat_map(move |c| list_dir(&h("Library/Application Support").join(f).join(c)))).collect()
}

/// args: the app's support folders (home-relative).
fn electron_app_caches(_: &ScanCtx, roots: &[String]) -> Vec<PathBuf> {
    roots.iter().flat_map(|r| ["Cache", "Code Cache", "GPUCache", "Service Worker/CacheStorage"].iter().flat_map(move |c| list_dir(&h(r).join(c)))).collect()
}

/// Cache folders of every Electron-style app in Application Support, except protected apps and
/// apps with a target of their own.
fn electron_caches(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let mut out = vec![];
    for app in list_dir(&h("Library/Application Support")) {
        let n = name_of(&app);
        if is_protected_name(&n) || OWN_TARGET_SUPPORT.contains(&n.as_str()) {
            continue;
        }
        out.extend(ELECTRON_CACHE_DIRS.iter().map(|c| app.join(c)).filter(|p| is_dir(p)));
    }
    out
}

fn esp_old_toolchains(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h(".espressif/tools")).iter().flat_map(|t| all_but_newest(t, 1)).collect()
}

fn fvm_unused(ctx: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let used = fvm_versions_in_use(ctx.roots);
    list_dir(&h("fvm/versions")).into_iter().filter(|p| !used.contains(&name_of(p))).collect()
}

/// The newest folder of each JetBrains IDE (older versions are `old-ide-versions`).
fn jetbrains_current(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    let parent = h("Library/Caches/JetBrains");
    let old: HashSet<PathBuf> = old_ide_dirs(&parent).into_iter().collect();
    list_dir(&parent).into_iter().filter(|d| is_dir(d) && !old.contains(d)).flat_map(|d| list_dir(&d)).collect()
}

/// ~/Library/Caches, minus Apple's own, download caches, folders with their own target and protected names.
fn library_caches(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h("Library/Caches"))
        .into_iter()
        .filter(|p| {
            let n = name_of(p);
            !(n.starts_with("com.apple.") || LIB_CACHE_KEEP.contains(&n.as_str()) || OWN_TARGET_CACHES.contains(&n.as_str()) || is_protected_name(&n))
        })
        .collect()
}

fn old_device_logs(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h("Library/Developer/Xcode/iOS Device Logs")).into_iter().filter(|p| older_than(p, 30)).collect()
}

/// args: folders holding versioned IDE folders (`AndroidStudio2024.1`…); all but the newest of each.
fn old_ide_versions(_: &ScanCtx, parents: &[String]) -> Vec<PathBuf> {
    parents.iter().flat_map(|p| old_ide_dirs(&h(p))).collect()
}

/// Archives/<date>/<name>.xcarchive older than 6 months.
fn xcode_old_archives(_: &ScanCtx, _: &[String]) -> Vec<PathBuf> {
    list_dir(&h("Library/Developer/Xcode/Archives"))
        .into_iter()
        .filter(|d| is_dir(d))
        .flat_map(|d| list_dir(&d))
        .filter(|a| name_of(a).ends_with(".xcarchive") && older_than(a, 180))
        .collect()
}

// ───────────────────────── commands ─────────────────────────

/// args: the `docker system df` type, then the prune command's arguments.
fn docker(ctx: &ScanCtx, args: &[String]) -> Option<Found> {
    let (ty, prune) = args.split_first()?;
    let bytes = *ctx.docker_df()?.get(ty)?;
    Some(Found { bytes, paths: vec![], commands: vec![Cmd { cmd: "docker".into(), args: prune.to_vec() }], detail: None })
}

fn homebrew(_: &ScanCtx, _: &[String]) -> Option<Found> {
    if !which("brew") {
        return None;
    }
    let c = run_s("brew", &["--cache"], 60).stdout.trim().to_string();
    let bytes = if c.is_empty() { 0 } else { total_size(&[PathBuf::from(c)]) };
    Some(Found { bytes, paths: vec![], commands: vec![cmd("brew", &["cleanup", "--prune=all", "-s"])], detail: None })
}

fn rust_old_toolchains(_: &ScanCtx, _: &[String]) -> Option<Found> {
    if !which("rustup") {
        return None;
    }
    let r = run_s("rustup", &["toolchain", "list"], 30);
    let extra: Vec<String> = r.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.contains('(') && l.chars().next().is_some_and(|c| c.is_ascii_digit())).collect();
    if extra.is_empty() {
        return None;
    }
    let bytes = total_size(&extra.iter().map(|t| h(".rustup/toolchains").join(t)).collect::<Vec<_>>());
    let commands = extra.iter().map(|t| cmd("rustup", &["toolchain", "uninstall", t])).collect();
    let detail = extra.iter().map(|t| t.split('-').next().unwrap_or(t).to_string()).collect::<Vec<_>>().join(", ");
    Some(Found { bytes, paths: vec![], commands, detail: Some(detail) })
}

fn sim_erase(_: &ScanCtx, _: &[String]) -> Option<Found> {
    if !which("xcrun") {
        return None;
    }
    let bytes = total_size(&[h("Library/Developer/CoreSimulator/Devices")]);
    if bytes < 200 * 1024 * 1024 {
        return None;
    }
    let commands = vec![cmd("xcrun", &["simctl", "shutdown", "all"]), cmd("xcrun", &["simctl", "erase", "all"])];
    Some(Found { bytes, paths: vec![], commands, detail: None })
}

fn sim_runtimes_old(_: &ScanCtx, _: &[String]) -> Option<Found> {
    if !which("xcrun") {
        return None;
    }
    let r = run_s("xcrun", &["simctl", "runtime", "list", "-j"], 60);
    let j: serde_json::Value = serde_json::from_str(&r.stdout).ok()?;
    let cutoff = now_days() - 30;
    let old: Vec<&serde_json::Value> = j
        .as_object()?
        .values()
        .filter(|rt| {
            rt.get("deletable").and_then(|v| v.as_bool()).unwrap_or(false)
                && rt.get("lastUsedAt").and_then(|v| v.as_str()).and_then(iso_days).map(|d| d < cutoff).unwrap_or(true)
        })
        .collect();
    if old.is_empty() {
        return None;
    }
    let bytes = old.iter().filter_map(|r| r.get("sizeBytes")?.as_u64()).sum();
    let commands = old.iter().filter_map(|r| Some(cmd("xcrun", &["simctl", "runtime", "delete", r.get("identifier")?.as_str()?]))).collect();
    let detail = old
        .iter()
        .filter_map(|r| {
            let plat = r.get("platformIdentifier")?.as_str()?.rsplit('.').next()?.replace("simulator", "");
            Some(format!("{} {}", plat, r.get("version")?.as_str()?))
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(Found { bytes, paths: vec![], commands, detail: Some(detail) })
}

fn sim_unavailable(_: &ScanCtx, _: &[String]) -> Option<Found> {
    if !which("xcrun") {
        return None;
    }
    let r = run_s("xcrun", &["simctl", "list", "devices", "unavailable", "-j"], 60);
    let j: serde_json::Value = serde_json::from_str(&r.stdout).ok()?;
    let udids: Vec<String> = j.get("devices")?.as_object()?.values().filter_map(|v| v.as_array()).flatten().filter_map(|d| d.get("udid")?.as_str().map(String::from)).collect();
    if udids.is_empty() {
        return None;
    }
    let bytes = total_size(&udids.iter().map(|u| h("Library/Developer/CoreSimulator/Devices").join(u)).collect::<Vec<_>>());
    let commands = vec![cmd("xcrun", &["simctl", "delete", "unavailable"])];
    Some(Found { bytes, paths: vec![], commands, detail: Some(format!("{} devices", udids.len())) })
}

// ───────────────────────── helpers ─────────────────────────

/// Download caches in ~/Library/Caches: never swept as "app caches" (deleting them costs a download),
/// offered instead by their own opt-in targets.
pub(crate) const LIB_CACHE_KEEP: &[&str] = &[
    "Homebrew", "Yarn", "pnpm", "CocoaPods", "ms-playwright", "ms-playwright-go", "pip", "pypoetry", "composer", "deno", "electron",
    "node-gyp", "org.swift.swiftpm", "org.carthage.CarthageKit", "Coursier", "bazel", "mise",
];
pub(crate) const ELECTRON_CACHE_DIRS: &[&str] = &[
    "Cache", "Code Cache", "GPUCache", "CachedData", "CachedExtensionVSIXs", "DawnGraphiteCache", "DawnWebGPUCache", "ShaderCache",
    "GrShaderCache", "Service Worker/CacheStorage", "Service Worker/ScriptCache", "Crashpad/completed", "logs",
];

pub(crate) fn old_ide_dirs(parent: &Path) -> Vec<PathBuf> {
    let mut by_product: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for p in list_dir(parent) {
        let n = name_of(&p);
        let split = n.find(|c: char| c.is_ascii_digit());
        let Some(i) = split else { continue };
        let (prod, ver) = n.split_at(i);
        let ver_ok = ver.len() >= 6 && ver[..4].chars().all(|c| c.is_ascii_digit()) && ver[4..5] == *"." && ver[5..].chars().all(|c| c.is_ascii_digit() || c == '.');
        if prod.is_empty() || !prod.chars().all(|c| c.is_ascii_alphabetic()) || !ver_ok || !is_dir(&p) {
            continue;
        }
        by_product.entry(prod.to_string()).or_default().push(p);
    }
    by_product
        .into_values()
        .flat_map(|mut v| {
            v.sort_by(by_version_desc);
            v.into_iter().skip(1)
        })
        .collect()
}

pub(crate) fn fvm_versions_in_use(roots: &[PathBuf]) -> HashSet<String> {
    let mut used = HashSet::new();
    for root in roots {
        let r = run_s(
            "/usr/bin/find",
            &[&root.to_string_lossy(), "-maxdepth", "6", "(", "-name", ".fvmrc", "-o", "-name", "fvm_config.json", ")", "-not", "-path", "*/node_modules/*"],
            120,
        );
        for f in r.stdout.lines().filter(|l| !l.is_empty()) {
            if let Ok(s) = std::fs::read_to_string(f) {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(v) = j.get("flutter").or_else(|| j.get("flutterSdkVersion")).and_then(|v| v.as_str()) {
                        used.insert(v.to_string());
                    }
                }
            }
        }
    }
    used
}

pub(crate) fn docker_df() -> Option<HashMap<String, u64>> {
    if !which("docker") {
        return None;
    }
    if !run_s("docker", &["info", "--format", "{{.ServerVersion}}"], 15).ok {
        return None; // daemon not running
    }
    let r = run_s("docker", &["system", "df", "--format", "{{json .}}"], 60);
    if !r.ok {
        return None;
    }
    let mut out = HashMap::new();
    for line in r.stdout.lines() {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(line) {
            if let (Some(t), Some(rec)) = (j.get("Type").and_then(|v| v.as_str()), j.get("Reclaimable").and_then(|v| v.as_str())) {
                out.insert(t.to_string(), parse_docker_size(rec));
            }
        }
    }
    Some(out)
}

pub(crate) const CHROMIUM_PROFILE_CACHES: &[&str] = &["Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "GrShaderCache", "GraphiteDawnCache", "Application Cache", "Service Worker/CacheStorage", "Service Worker/ScriptCache"];

/// A Chromium browser: its ~/Library/Caches folder plus per-profile cache folders (never profile data).
pub(crate) fn chromium(support: &str, caches: &str) -> Vec<PathBuf> {
    let mut out = list_dir(&h(&format!("Library/Caches/{caches}")));
    let base = h(&format!("Library/Application Support/{support}"));
    for top in ["ShaderCache", "GrShaderCache", "GraphiteDawnCache", "component_crx_cache"] {
        out.extend(list_dir(&base.join(top)));
    }
    for profile in list_dir(&base).into_iter().filter(|p| is_dir(p)) {
        for c in CHROMIUM_PROFILE_CACHES {
            out.extend(list_dir(&profile.join(c)));
        }
    }
    out
}

/// Cache folders of a VS Code-family editor. Its Application Support folder is otherwise protected
/// (settings, workspace state, extension storage), so only these folders are ever touched.
pub(crate) const EDITOR_CACHE_DIRS: &[&str] = &["Cache", "Code Cache", "GPUCache", "CachedData", "CachedExtensionVSIXs", "DawnGraphiteCache", "DawnWebGPUCache", "logs", "Crashpad/completed"];

/// ~/Library/Caches folders that have their own (guarded) target above, so the sweep skips them.
pub(crate) const OWN_TARGET_CACHES: &[&str] = &[
    "Google", "Firefox", "com.microsoft.edgemac", "BraveSoftware", "company.thebrowser.Browser", "com.vivaldi.Vivaldi", "us.zoom.xos",
    "com.microsoft.teams2", "net.whatsapp.WhatsApp", "ru.keepcoder.Telegram", "com.figma.Desktop", "com.openai.chat",
    "com.anthropic.claudefordesktop", "org.videolan.vlc", "com.colliderli.iina", "tv.plex.player.desktop", "JetBrains", "go-build",
];
/// ~/Library/Application Support folders with their own target (or browser profiles) the Electron sweep skips.
pub(crate) const OWN_TARGET_SUPPORT: &[&str] = &["discord", "Slack", "Arc", "Google", "Microsoft Edge", "BraveSoftware", "Vivaldi", "Adobe"];
