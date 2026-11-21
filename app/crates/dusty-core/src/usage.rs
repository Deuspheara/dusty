//! Disk usage tree (for a treemap / sunburst) and a squarified treemap layout.
//!
//! Built on the same native sizer as the scan: allocated blocks, each hard-linked file counted once,
//! symlinks measured as links (never followed), one volume only, iCloud placeholders never opened,
//! a time budget and a cancel flag. Read-only: nothing here deletes anything.

use super::{home, is_dataless, name_of, size_of_with};
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// The "everything else" bucket: `count` smaller entries merged into one.
    Other { count: usize },
}

#[derive(Clone, Debug)]
pub struct UsageNode {
    pub name: String,
    /// `None` for the "other" bucket.
    pub path: Option<PathBuf>,
    pub bytes: u64,
    pub kind: NodeKind,
    /// Biggest first; at most `top_n` real entries plus one `Other`. Empty past the depth limit
    /// (call [`usage_tree`] again on that folder to drill down).
    pub children: Vec<UsageNode>,
    /// True when the size may be low: the time budget ran out or the walk was cancelled.
    pub partial: bool,
    /// An iCloud placeholder (content not on this Mac, so it takes no space here).
    pub in_cloud: bool,
}

impl UsageNode {
    /// Squarified treemap rects for this node's children, in `children` order.
    pub fn layout(&self, bounds: Rect) -> Vec<Rect> {
        squarify(&self.children.iter().map(|c| c.bytes).collect::<Vec<_>>(), bounds)
    }
}

#[derive(Clone, Debug)]
pub struct UsageOptions {
    pub root: PathBuf,
    /// Levels of children to return (1 = just the root's children).
    pub depth: usize,
    /// Children kept per node; the rest are merged into an "other" bucket.
    pub top_n: usize,
    /// Wall-clock budget for the whole walk; past it, sizes stop growing (`partial`).
    pub budget: Duration,
}

impl Default for UsageOptions {
    fn default() -> Self {
        UsageOptions { root: home(), depth: 3, top_n: 12, budget: Duration::from_secs(60) }
    }
}

/// Live progress for the UI.
#[derive(Default, Clone, Debug)]
pub struct UsageProgress {
    pub message: String,
    /// Folders listed so far (phase 1), then entries measured.
    pub entries: usize,
    pub done: bool,
}

/// One entry found while listing: either expanded (children listed) or a leaf to measure.
struct Raw {
    name: String,
    path: PathBuf,
    is_dir: bool,
    in_cloud: bool,
    /// Blocks of the entry itself (a folder's own blocks; leaves are measured whole later).
    own: u64,
    children: Option<Vec<Raw>>,
}

/// Size tree of `opts.root`. Blocking and parallel: call it from a background thread. Returns
/// `None` if the root can't be read.
pub fn usage_tree(opts: &UsageOptions, progress: &Mutex<UsageProgress>, cancel: &AtomicBool) -> Option<UsageNode> {
    let start = Instant::now();
    let deadline = start + opts.budget;
    let meta = std::fs::symlink_metadata(&opts.root).ok()?;
    let dev = meta.dev();
    progress.lock().unwrap().message = "Listing folders…".into();
    // phase 1: list folders down to `depth`, collecting the leaves to measure
    let mut leaves: Vec<PathBuf> = vec![];
    let mut listed = 0usize;
    let mut root = Raw { name: name_of(&opts.root), path: opts.root.clone(), is_dir: meta.is_dir(), in_cloud: is_dataless(&meta), own: meta.blocks() * 512, children: None };
    if root.is_dir && !root.in_cloud {
        root.children = Some(list(&opts.root, 1, opts.depth, dev, &mut leaves, &mut listed, progress, cancel, deadline));
    } else {
        leaves.push(opts.root.clone());
    }
    // phase 2: measure every leaf in parallel, sharing one hard-link set
    {
        let mut p = progress.lock().unwrap();
        p.message = format!("Measuring {} items…", leaves.len());
        p.entries = leaves.len();
    }
    let seen = Mutex::new(HashSet::new());
    let sizes = size_of_with(&leaves, deadline, cancel, &seen);
    let partial = Instant::now() > deadline || cancel.load(Ordering::Relaxed);
    let node = build(root, &sizes, opts.top_n, partial);
    let mut p = progress.lock().unwrap();
    p.done = true;
    p.message = "Done".into();
    Some(node)
}

#[allow(clippy::too_many_arguments)]
fn list(
    dir: &std::path::Path,
    level: usize,
    depth: usize,
    dev: u64,
    leaves: &mut Vec<PathBuf>,
    listed: &mut usize,
    progress: &Mutex<UsageProgress>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Vec<Raw> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    *listed += 1;
    if (*listed).is_multiple_of(64) {
        progress.lock().unwrap().entries = *listed;
    }
    let mut out = vec![];
    for e in rd.flatten() {
        // DirEntry::metadata is lstat: a symlink is a (tiny) leaf, never followed
        let Ok(m) = e.metadata() else { continue };
        if m.dev() != dev {
            continue; // another volume mounted inside: not part of this disk's usage
        }
        let path = e.path();
        let in_cloud = is_dataless(&m);
        let is_dir = m.is_dir();
        let expand = is_dir && !in_cloud && level < depth && !cancel.load(Ordering::Relaxed) && Instant::now() < deadline;
        let children = if expand { Some(list(&path, level + 1, depth, dev, leaves, listed, progress, cancel, deadline)) } else { None };
        if children.is_none() {
            leaves.push(path.clone());
        }
        out.push(Raw { name: e.file_name().to_string_lossy().into_owned(), path, is_dir, in_cloud, own: m.blocks() * 512, children });
    }
    out
}

fn build(raw: Raw, sizes: &std::collections::HashMap<PathBuf, u64>, top_n: usize, partial: bool) -> UsageNode {
    let kind = if raw.is_dir { NodeKind::Dir } else { NodeKind::File };
    match raw.children {
        None => UsageNode {
            bytes: sizes.get(&raw.path).copied().unwrap_or(0),
            name: raw.name,
            path: Some(raw.path),
            kind,
            children: vec![],
            partial,
            in_cloud: raw.in_cloud,
        },
        Some(kids) => {
            let mut children: Vec<UsageNode> = kids.into_iter().map(|k| build(k, sizes, top_n, partial)).collect();
            let bytes = raw.own + children.iter().map(|c| c.bytes).sum::<u64>();
            children.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
            if children.len() > top_n {
                let rest: Vec<UsageNode> = children.split_off(top_n);
                children.push(UsageNode {
                    name: format!("{} other items", rest.len()),
                    path: None,
                    bytes: rest.iter().map(|c| c.bytes).sum(),
                    kind: NodeKind::Other { count: rest.len() },
                    children: vec![],
                    partial,
                    in_cloud: false,
                });
            }
            UsageNode { name: raw.name, path: Some(raw.path), bytes, kind, children, partial, in_cloud: raw.in_cloud }
        }
    }
}

// ───────────────────────── treemap layout ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    #[cfg(test)]
    pub fn area(&self) -> f32 {
        self.w * self.h
    }
}

/// Squarified treemap (Bruls, Huizing & van Wijk): splits `bounds` into one rect per value, area
/// proportional to the value, keeping rects close to square. Pure. Returns rects in the order of
/// `values` (they don't need to be sorted); zero values get an empty rect at the origin of `bounds`.
pub fn squarify(values: &[u64], bounds: Rect) -> Vec<Rect> {
    let mut out = vec![Rect { x: bounds.x, y: bounds.y, w: 0.0, h: 0.0 }; values.len()];
    let total: f64 = values.iter().map(|v| *v as f64).sum();
    if total <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }
    let mut order: Vec<usize> = (0..values.len()).filter(|i| values[*i] > 0).collect();
    order.sort_by(|a, b| values[*b].cmp(&values[*a]).then(a.cmp(b)));
    let scale = (bounds.w as f64 * bounds.h as f64) / total;
    let areas: Vec<(usize, f64)> = order.iter().map(|i| (*i, values[*i] as f64 * scale)).collect();
    let (mut x, mut y, mut w, mut h) = (bounds.x as f64, bounds.y as f64, bounds.w as f64, bounds.h as f64);
    let worst = |row: &[(usize, f64)], side: f64| -> f64 {
        let s: f64 = row.iter().map(|r| r.1).sum();
        let (mx, mn) = row.iter().fold((0f64, f64::MAX), |(mx, mn), r| (mx.max(r.1), mn.min(r.1)));
        ((side * side * mx) / (s * s)).max((s * s) / (side * side * mn))
    };
    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        // grow the row while it makes the worst aspect ratio better
        let mut end = i + 1;
        while end < areas.len() && worst(&areas[i..=end], side) <= worst(&areas[i..end], side) {
            end += 1;
        }
        let row = &areas[i..end];
        let s: f64 = row.iter().map(|r| r.1).sum();
        let last = end == areas.len();
        if w >= h {
            // a column on the left
            let cw = if last { w } else { (s / h).min(w) };
            let mut cy = y;
            for (k, (idx, a)) in row.iter().enumerate() {
                let rh = if k + 1 == row.len() { y + h - cy } else { a / cw };
                out[*idx] = Rect { x: x as f32, y: cy as f32, w: cw as f32, h: rh.max(0.0) as f32 };
                cy += rh;
            }
            x += cw;
            w -= cw;
        } else {
            // a row on top
            let rh = if last { h } else { (s / w).min(h) };
            let mut cx = x;
            for (k, (idx, a)) in row.iter().enumerate() {
                let rw = if k + 1 == row.len() { x + w - cx } else { a / rh };
                out[*idx] = Rect { x: cx as f32, y: y as f32, w: rw.max(0.0) as f32, h: rh as f32 };
                cx += rw;
            }
            y += rh;
            h -= rh;
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::{tmp_dir, touch};
    use super::*;

    fn check_layout(values: &[u64], b: Rect) -> Vec<Rect> {
        let rects = squarify(values, b);
        assert_eq!(rects.len(), values.len());
        let total: f64 = values.iter().map(|v| *v as f64).sum();
        let eps = 1e-2;
        for (r, v) in rects.iter().zip(values) {
            assert!(r.x >= b.x - eps as f32 && r.y >= b.y - eps as f32, "{r:?} starts outside {b:?}");
            assert!(r.x + r.w <= b.x + b.w + eps as f32 && r.y + r.h <= b.y + b.h + eps as f32, "{r:?} ends outside {b:?}");
            let want = *v as f64 / total * b.area() as f64;
            assert!((r.area() as f64 - want).abs() <= want * 0.01 + 0.5, "area {} vs {want}", r.area());
        }
        for (i, a) in rects.iter().enumerate() {
            for c in &rects[i + 1..] {
                let ox = (a.x + a.w).min(c.x + c.w) - a.x.max(c.x);
                let oy = (a.y + a.h).min(c.y + c.h) - a.y.max(c.y);
                assert!(ox <= 0.01 || oy <= 0.01, "{a:?} overlaps {c:?}");
            }
        }
        let sum: f32 = rects.iter().map(Rect::area).sum();
        assert!((sum - b.area()).abs() < b.area() * 0.001 + 1.0, "covers {sum} of {}", b.area());
        rects
    }

    #[test]
    fn squarify_basics() {
        // four equal values in a square: four squares
        let r = check_layout(&[1, 1, 1, 1], Rect::new(0.0, 0.0, 100.0, 100.0));
        for q in &r {
            assert!((q.w - 50.0).abs() < 0.01 && (q.h - 50.0).abs() < 0.01, "{q:?}");
        }
        // the classic paper example, unsorted input, offset bounds
        let vals = [1, 6, 2, 4, 3, 6, 2];
        let r = check_layout(&vals, Rect::new(10.0, 20.0, 600.0, 400.0));
        // ratios stay reasonable
        for q in r.iter() {
            let ratio = (q.w / q.h).max(q.h / q.w);
            assert!(ratio < 4.0, "{q:?}");
        }
        check_layout(&[1000, 1, 1, 1, 500, 3, 250, 7], Rect::new(0.0, 0.0, 320.0, 90.0));
        check_layout(&[5], Rect::new(0.0, 0.0, 30.0, 300.0));
        // zeros and empties
        let r = squarify(&[0, 3, 0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(r[0].area(), 0.0);
        assert!((r[1].area() - 100.0).abs() < 0.01);
        assert!(squarify(&[], Rect::new(0.0, 0.0, 1.0, 1.0)).is_empty());
        assert!(squarify(&[0, 0], Rect::new(0.0, 0.0, 1.0, 1.0)).iter().all(|r| r.area() == 0.0));
    }

    #[test]
    fn tree_of_a_folder() {
        let d = std::fs::canonicalize(tmp_dir("usage")).unwrap();
        touch(&d.join("big/a.bin"), 400_000);
        touch(&d.join("big/deep/b.bin"), 200_000);
        for i in 0..5 {
            touch(&d.join(format!("small/f{i}.bin")), 10_000 * (i + 1));
        }
        touch(&d.join("top.bin"), 50_000);
        // both links under big/: which sibling gets credited for a shared file depends on thread timing
        std::fs::hard_link(d.join("big/a.bin"), d.join("big/deep/link.bin")).unwrap();
        std::os::unix::fs::symlink(d.join("big"), d.join("to-big")).unwrap();
        let opts = UsageOptions { root: d.clone(), depth: 2, top_n: 3, budget: Duration::from_secs(30) };
        let progress = Mutex::new(UsageProgress::default());
        let t = usage_tree(&opts, &progress, &AtomicBool::new(false)).unwrap();
        assert!(progress.lock().unwrap().done);
        assert!(!t.partial);
        assert_eq!(t.kind, NodeKind::Dir);
        // biggest first, top_n then "other"
        assert_eq!(t.children[0].name, "big");
        assert_eq!(t.children.len(), 4);
        assert!(matches!(t.children[3].kind, NodeKind::Other { count: 1 }));
        let sum: u64 = t.children.iter().map(|c| c.bytes).sum();
        assert!(t.bytes >= sum);
        // the hard link is counted once in the whole tree, the symlink isn't followed
        assert!(t.bytes < 900_000, "{}", t.bytes);
        assert!(t.bytes >= 750_000, "{}", t.bytes);
        let big = &t.children[0];
        assert!(big.bytes >= 600_000);
        assert_eq!(big.children.len(), 2, "depth 2 lists big/'s children");
        assert!(big.children.iter().all(|c| c.children.is_empty()), "but no deeper");
        assert_eq!(t.layout(Rect::new(0.0, 0.0, 100.0, 50.0)).len(), t.children.len());
        // cancelled: still returns, marked partial
        let c = AtomicBool::new(true);
        assert!(usage_tree(&opts, &Mutex::new(UsageProgress::default()), &c).unwrap().partial);
        let _ = std::fs::remove_dir_all(&d);
    }
}
