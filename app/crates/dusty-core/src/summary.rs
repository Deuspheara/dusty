//! Last scan summary: `~/Library/Application Support/Dusty/last-scan.json`, written at the end of every
//! finished (not cancelled) `scan_all`, so the home screen can say "Last checkup: 2 days ago, 12.3 GB".
//!
//! The file also carries the short keys `version`, `at`, `found` and `safe` that `src/prefs.rs` reads
//! and writes, and [`load_last_scan`] accepts a file with only those, so both stay compatible.
//! `DUSTY_LAST_SCAN=<path>` points both at another file.

use super::history::{iso_utc, support_dir};
use super::{disk, now_secs, write_atomic, Group, Target};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct LastScan {
    /// Unix seconds.
    pub ts: u64,
    /// ISO 8601, UTC.
    pub date: String,
    /// Everything found (all items, opt-in included).
    pub total_bytes: u64,
    /// What the default selection would free (the "safe picks").
    pub safe_bytes: u64,
    pub items: usize,
    /// Bytes per group, in `Group::ORDER`, groups with nothing found left out.
    pub groups: Vec<(Group, u64)>,
    pub disk_free: u64,
    pub disk_total: u64,
}

impl LastScan {
    pub fn from_targets(targets: &[Target]) -> Self {
        let ts = now_secs();
        let d = disk();
        LastScan {
            ts,
            date: iso_utc(ts),
            total_bytes: targets.iter().map(|t| t.bytes).sum(),
            safe_bytes: targets.iter().filter(|t| t.default_on).map(|t| t.bytes).sum(),
            items: targets.len(),
            groups: Group::ORDER
                .iter()
                .map(|g| (*g, targets.iter().filter(|t| t.group == *g).map(|t| t.bytes).sum::<u64>()))
                .filter(|(_, b)| *b > 0)
                .collect(),
            disk_free: d.free,
            disk_total: d.total,
        }
    }

    /// Seconds since this scan.
    pub fn age_secs(&self) -> u64 {
        now_secs().saturating_sub(self.ts)
    }

    fn to_json(&self) -> Value {
        json!({
            "version": 1,
            "at": self.ts,
            "found": self.total_bytes,
            "safe": self.safe_bytes,
            "ts": self.ts,
            "date": self.date,
            "total_bytes": self.total_bytes,
            "safe_bytes": self.safe_bytes,
            "items": self.items,
            "groups": self.groups.iter().map(|(g, b)| (g.key().to_string(), json!(b))).collect::<serde_json::Map<_, _>>(),
            "disk_free": self.disk_free,
            "disk_total": self.disk_total,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64());
        let obj = v.get("groups").and_then(|g| g.as_object());
        let groups = Group::ORDER
            .iter()
            .filter_map(|g| Some((*g, obj?.get(g.key())?.as_u64()?)))
            .collect();
        let ts = n("ts").or_else(|| n("at"))?;
        Some(LastScan {
            ts,
            date: v.get("date").and_then(|d| d.as_str()).map(String::from).unwrap_or_else(|| iso_utc(ts)),
            total_bytes: n("total_bytes").or_else(|| n("found"))?,
            safe_bytes: n("safe_bytes").or_else(|| n("safe")).unwrap_or(0),
            items: n("items").unwrap_or(0) as usize,
            groups,
            disk_free: n("disk_free").unwrap_or(0),
            disk_total: n("disk_total").unwrap_or(0),
        })
    }
}

pub fn last_scan_file() -> PathBuf {
    if let Some(p) = std::env::var_os("DUSTY_LAST_SCAN") {
        return PathBuf::from(p);
    }
    support_dir().join("last-scan.json")
}

/// Saves the summary of a finished scan (atomically: temp file + rename).
pub fn save_last_scan(targets: &[Target]) -> Result<LastScan, String> {
    let s = LastScan::from_targets(targets);
    write_atomic(&last_scan_file(), serde_json::to_string_pretty(&s.to_json()).unwrap().as_bytes())?;
    Ok(s)
}

/// The last finished scan, if there was one.
pub fn load_last_scan() -> Option<LastScan> {
    let text = std::fs::read_to_string(last_scan_file()).ok()?;
    LastScan::from_json(&serde_json::from_str(&text).ok()?)
}

#[cfg(test)]
mod tests {
    use super::super::tests::FakeHome;
    use super::super::Guard;
    use super::*;

    #[test]
    fn round_trip() {
        let _fake = FakeHome::new("summary");
        assert!(load_last_scan().is_none());
        let t = |g, on, b| Target { id: "x".into(), label: "x".into(), group: g, default_on: on, bytes: b, paths: vec![], commands: vec![], detail: None, guard: Guard::None, wipes_data: false };
        let saved = save_last_scan(&[t(Group::Build, true, 100), t(Group::Build, true, 50), t(Group::Redownload, false, 1000), t(Group::Junk, true, 0)]).unwrap();
        let loaded = load_last_scan().unwrap();
        assert_eq!(saved, loaded);
        assert_eq!(loaded.total_bytes, 1150);
        assert_eq!(loaded.safe_bytes, 150);
        assert_eq!(loaded.items, 4);
        assert_eq!(loaded.groups, vec![(Group::Build, 150), (Group::Redownload, 1000)]);
        assert!(loaded.age_secs() < 5);
        // the short format prefs.rs writes still loads
        std::fs::write(last_scan_file(), r#"{"version":1,"at":1700000000,"found":42,"safe":7}"#).unwrap();
        let short = load_last_scan().unwrap();
        assert_eq!((short.ts, short.total_bytes, short.safe_bytes, short.date.as_str()), (1_700_000_000, 42, 7, "2023-11-14T22:13:20Z"));
    }
}
