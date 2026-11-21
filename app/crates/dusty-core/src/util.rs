//! Small shared helpers: running commands, sizing folders, paths, dates and version sorting.

use super::*;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
pub(crate) fn h(p: &str) -> PathBuf {
    home().join(p)
}

/// Apps launched from Finder get a bare PATH; add the usual tool locations so brew/docker/xcrun resolve.
pub fn fix_path() {
    let cur = std::env::var("PATH").unwrap_or_default();
    let extra = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"];
    let mut parts: Vec<String> = cur.split(':').filter(|s| !s.is_empty()).map(String::from).collect();
    for e in extra.iter().rev() {
        if !parts.iter().any(|p| p == e) {
            parts.insert(0, e.to_string());
        }
    }
    let cargo = h(".cargo/bin");
    parts.push(cargo.to_string_lossy().into());
    // SAFETY: called once at startup before any threads are spawned.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

pub struct RunResult {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run a command with a timeout. Never panics; a missing binary is just `ok: false`.
pub fn run(program: &str, args: &[String], timeout: Duration, stdin: Option<&str>) -> RunResult {
    let mut c = Command::new(program);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return RunResult { ok: false, stdout: String::new(), stderr: e.to_string() },
    };
    if let (Some(input), Some(mut sin)) = (stdin, child.stdin.take()) {
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = sin.write_all(input.as_bytes());
        });
    }
    // drain pipes on threads so a chatty child can't deadlock on a full pipe
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let to = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = std::io::Read::read_to_string(o, &mut s);
        }
        s
    });
    let te = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let _ = std::io::Read::read_to_string(e, &mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    let stdout = to.join().unwrap_or_default();
    let stderr = te.join().unwrap_or_default();
    RunResult { ok: status.map(|s| s.success()).unwrap_or(false), stdout, stderr }
}

pub(crate) fn run_s(program: &str, args: &[&str], secs: u64) -> RunResult {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    run(program, &a, Duration::from_secs(secs), None)
}

/// Writes `bytes` to `path` atomically (temp file + rename, creating the folder), so a crash never
/// leaves a half-written file behind.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or_else(|| format!("{} has no folder", tildify(path)))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {} ({e})", tildify(dir)))?;
    let tmp = dir.join(format!(".{}.{}.tmp", name_of(path), std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("couldn't write {} ({e})", tildify(&tmp)))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("couldn't save {} ({e})", tildify(path))
    })
}

/// A property list as JSON (`plutil -convert json`). `None` if it can't be read or converted
/// (plists holding dates or data have no JSON form).
pub(crate) fn read_plist_json(p: &Path) -> Option<serde_json::Value> {
    let r = run_s("/usr/bin/plutil", &["-convert", "json", "-o", "-", &p.to_string_lossy()], 10);
    r.ok.then(|| serde_json::from_str(&r.stdout).ok()).flatten()
}

/// Threads for a parallel folder walk: one per core, between 2 and 8.
pub(crate) fn walk_workers() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8)
}

/// Test switch: `DUSTY_FORCE_DRY=1` makes every clean a practice run, whatever the UI asked for.
pub(crate) fn force_dry(dry_run: bool) -> bool {
    dry_run || std::env::var_os("DUSTY_FORCE_DRY").is_some()
}

pub(crate) fn which(bin: &str) -> bool {
    run_s("/usr/bin/which", &[bin], 10).ok
}

pub(crate) fn is_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}
pub(crate) fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

pub(crate) fn list_dir(p: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(p) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => vec![],
    }
}

pub fn name_of(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Real on-disk sizes (allocated blocks, so sparse files count right), measured natively and in parallel.
/// Like `du -skx`, it never follows symlinks and never crosses into another volume. Unlike separate `du` runs,
/// a hard-linked file is counted once per call, and iCloud placeholders (dataless folders) are never opened,
/// which would make macOS download them. Paths that don't exist are absent from the map.
pub fn size_of(paths: &[PathBuf]) -> HashMap<PathBuf, u64> {
    size_of_with(paths, Instant::now() + SIZE_BUDGET, &AtomicBool::new(false), &Mutex::new(HashSet::new()))
}

/// [`size_of`] with a caller-chosen deadline, a cancel flag (sizes stop growing once it is set) and a
/// shared hard-link set, so several calls can still count each hard-linked file once.
pub(crate) fn size_of_with(paths: &[PathBuf], deadline: Instant, cancel: &AtomicBool, seen: &Mutex<HashSet<(u64, u64)>>) -> HashMap<PathBuf, u64> {
    let out = Mutex::new(HashMap::new());
    let next = AtomicUsize::new(0);
    let workers = walk_workers().min(paths.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(p) = paths.get(i) else { break };
                if let Some(b) = tree_size(p, seen, deadline, cancel) {
                    out.lock().unwrap().insert(p.clone(), b);
                }
            });
        }
    });
    out.into_inner().unwrap()
}

/// A pathological folder (millions of files, a hung network mount) can't stall a scan past this:
/// past the deadline, sizes stop growing and the scan moves on (the delete itself is unaffected).
pub(crate) const SIZE_BUDGET: Duration = Duration::from_secs(240);

pub(crate) fn tree_size(root: &Path, seen: &Mutex<HashSet<(u64, u64)>>, deadline: Instant, cancel: &AtomicBool) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(root).ok()?;
    let dev = meta.dev();
    let mut total = 0u64;
    let mut stack = vec![(root.to_path_buf(), meta)];
    while let Some((p, m)) = stack.pop() {
        if !m.is_dir() && m.nlink() > 1 && !seen.lock().unwrap().insert((m.dev(), m.ino())) {
            continue; // another link to a file already counted
        }
        total += m.blocks() * 512;
        if !m.is_dir() || is_dataless(&m) || Instant::now() > deadline || cancel.load(Ordering::Relaxed) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            // DirEntry::metadata is lstat: symlinks are measured as links, never followed
            if let Ok(em) = e.metadata() {
                if em.dev() == dev {
                    stack.push((e.path(), em));
                }
            }
        }
    }
    Some(total)
}

/// An iCloud / File Provider placeholder whose contents live in the cloud. Reading it downloads it,
/// and deleting it deletes the cloud copy while freeing nothing here.
#[cfg(target_os = "macos")]
pub(crate) fn is_dataless(m: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    m.st_flags() & SF_DATALESS != 0
}
#[cfg(not(target_os = "macos"))]
pub(crate) fn is_dataless(_: &std::fs::Metadata) -> bool {
    false
}

pub(crate) fn total_size(paths: &[PathBuf]) -> u64 {
    size_of(paths).values().sum()
}

pub fn format_bytes(b: u64) -> String {
    if b < 1024 {
        return format!("{b} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = b as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 { format!("{:.0} {}", v, units[u]) } else { format!("{:.1} {}", v, units[u]) }
}

/// "6.745GB", "512MB", "0B" → bytes (docker uses SI units)
pub(crate) fn parse_docker_size(s: &str) -> u64 {
    let s = s.trim();
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let unit = s[num.len()..].trim().to_uppercase();
    let n: f64 = num.parse().unwrap_or(0.0);
    let m = if unit.starts_with("TB") { 1e12 } else if unit.starts_with("GB") { 1e9 } else if unit.starts_with("MB") { 1e6 } else if unit.starts_with("KB") { 1e3 } else { 1.0 };
    (n * m) as u64
}

pub fn disk() -> Disk {
    use std::ffi::CString;
    let path = CString::new("/System/Volumes/Data").unwrap();
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut st) } == 0 {
        let bs = st.f_bsize as u64;
        Disk { total: st.f_blocks * bs, free: st.f_bavail * bs }
    } else {
        Disk { total: 1, free: 0 }
    }
}

pub fn tildify(p: &Path) -> String {
    let hs = home().to_string_lossy().into_owned();
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(&hs) { format!("~{rest}") } else { s.into_owned() }
}

pub(crate) fn mtime(p: &Path) -> SystemTime {
    std::fs::symlink_metadata(p).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH)
}

pub(crate) fn older_than(p: &Path, days: u64) -> bool {
    SystemTime::now().duration_since(mtime(p)).map(|d| d > Duration::from_secs(days * 86400)).unwrap_or(false)
}

/// Days since 1970-01-01 for an ISO date "YYYY-MM-DD…" (civil-from-days inverse).
pub(crate) fn iso_days(s: &str) -> Option<i64> {
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// Seconds since 1970 (0 if the clock is before it).
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub(crate) fn now_days() -> i64 {
    (now_secs() / 86400) as i64
}

/// Version-like descending sort on the file name ("1.10" > "1.9").
pub(crate) fn by_version_desc(a: &PathBuf, b: &PathBuf) -> std::cmp::Ordering {
    natural_cmp(&name_of(b), &name_of(a))
}
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = vec![];
        for c in s.chars() {
            let dig = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, buf)) if *d == dig => buf.push(c),
                _ => out.push((dig, c.to_string())),
            }
        }
        out
    };
    let (x, y) = (split(a), split(b));
    for (p, q) in x.iter().zip(y.iter()) {
        let o = if p.0 && q.0 {
            p.1.trim_start_matches('0').len().cmp(&q.1.trim_start_matches('0').len()).then(p.1.cmp(&q.1))
        } else {
            p.1.to_lowercase().cmp(&q.1.to_lowercase())
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    x.len().cmp(&y.len())
}

pub(crate) fn all_but_newest(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = list_dir(dir).into_iter().filter(|p| is_dir(p)).collect();
    dirs.sort_by(by_version_desc);
    dirs.into_iter().skip(keep).collect()
}
