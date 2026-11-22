//! `--json` output. Field names are part of the CLI's contract: scripts read them.

use dusty_core::history::HistoryEntry;
use dusty_core::large::LargeFile;
use dusty_core::{CleanPlan, CleanReport, Target};
use serde_json::{json, Value};
use std::time::UNIX_EPOCH;

pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

pub fn targets(ts: &[Target]) -> Value {
    Value::Array(
        ts.iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "label": t.label,
                    "group": t.group.key(),
                    "defaultOn": t.default_on,
                    "bytes": t.bytes,
                    "paths": t.paths,
                    "commands": t.commands.iter().map(|c| json!({ "cmd": c.cmd, "args": c.args })).collect::<Vec<_>>(),
                    "detail": t.detail,
                })
            })
            .collect(),
    )
}

/// The plan, and the report once it was carried out (`null` for a dry run).
pub fn clean(plan: &CleanPlan, report: Option<&CleanReport>) -> Value {
    json!({
        "practice": plan.practice,
        "policy": plan.policy.key(),
        "bytes": plan.bytes(None),
        "items": plan.items.iter().map(|i| json!({
            "id": i.target.id,
            "label": i.target.label,
            "group": i.target.group.key(),
            "bytes": i.target.bytes,
            "removal": i.removal.key(),
            "wipesData": i.target.wipes_data,
            "paths": i.target.paths,
        })).collect::<Vec<_>>(),
        "report": report.map(|r| json!({ "entry": r.entry.to_json(), "errors": r.errors, "historyError": r.history_error })),
    })
}

/// Entries exactly as they are stored in the history file.
pub fn history(entries: &[HistoryEntry]) -> Value {
    Value::Array(entries.iter().map(HistoryEntry::to_json).collect())
}

pub fn large_files(files: &[LargeFile]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|f| {
                json!({
                    "path": f.path,
                    "bytes": f.bytes,
                    "lastUsed": f.last_used.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                    "isLarge": f.is_large,
                    "isOld": f.is_old,
                    "hardLinked": f.hard_linked,
                })
            })
            .collect(),
    )
}

pub fn explore(r: &dusty_ai::ExploreReport, agent: &dusty_ai::Agent) -> Value {
    json!({
        "agent": agent.id,
        "account": r.account,
        "tokens": r.tokens,
        "own_looks": r.own_looks,
        "suggestions": serde_json::to_value(&r.suggestions).unwrap_or_default(),
        "looked": r.looked,
        "unchanged": r.unchanged,
        "refused": r.refused,
        "note": r.note,
        "asked_ai": r.asked,
        "error": r.error.as_ref().map(|e| e.message(agent)),
    })
}
