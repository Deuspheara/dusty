//! Cleaning in two steps. [`CleanPlan::new`] works out what will happen to each picked item, and
//! touches nothing, so a confirm sheet or `dusty clean` can show it first. [`execute`] then does it:
//! every path is checked again right before it goes (things change between planning and confirming),
//! progress comes back as [`CleanEvent`]s, and the clean is recorded in the history.

use super::history::{HistoryEntry, HistoryItem, HistoryPath, HistorySource, PathAction};
use super::*;

/// What a clean will do.
#[derive(Clone, Debug)]
pub struct CleanPlan {
    pub items: Vec<PlanItem>,
    /// Practice run: everything is checked and recorded, nothing is removed.
    pub practice: bool,
    pub policy: CleanPolicy,
    pub source: HistorySource,
}

#[derive(Clone, Debug)]
pub struct PlanItem {
    pub target: Target,
    pub removal: Removal,
}

impl CleanPlan {
    /// The plan for cleaning `targets` under `policy`. Pure: nothing is touched.
    pub fn new(targets: &[Target], policy: CleanPolicy, source: HistorySource, practice: bool) -> Self {
        let items = targets.iter().map(|t| PlanItem { removal: policy.removal_for(t), target: t.clone() }).collect();
        CleanPlan { items, practice: force_dry(practice), policy, source }
    }

    /// Estimated bytes, in all or for one kind of removal.
    pub fn bytes(&self, removal: Option<Removal>) -> u64 {
        self.items.iter().filter(|i| removal.is_none_or(|r| i.removal == r)).map(|i| i.target.bytes).sum()
    }
}

/// Progress of [`execute`], one event per item.
#[derive(Clone, Debug)]
pub enum CleanEvent {
    Started { index: usize, total: usize, label: String },
    Finished { index: usize, bytes: u64, errors: Vec<String> },
}

/// Live progress built from [`CleanEvent`]s, for a UI to show.
#[derive(Default, Clone, Debug)]
pub struct CleanProgress {
    /// Label of the item being cleaned.
    pub current: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Estimated bytes of finished items.
    pub bytes_done: u64,
    pub errors: Vec<String>,
    /// Errors of each finished item, in order (empty when it went fine): lets a UI tell which one was busy.
    pub item_errors: Vec<Vec<String>>,
}

impl CleanProgress {
    pub fn apply(&mut self, e: &CleanEvent) {
        match e {
            CleanEvent::Started { total, label, .. } => {
                self.total = *total;
                self.current = Some(label.clone());
            }
            CleanEvent::Finished { bytes, errors, .. } => {
                self.done += 1;
                self.bytes_done += bytes;
                self.errors.extend(errors.iter().cloned());
                self.item_errors.push(errors.clone());
                self.current = None;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CleanReport {
    /// What was recorded in the history.
    pub entry: HistoryEntry,
    pub errors: Vec<String>,
    /// Set if the history line couldn't be written (the clean itself still happened).
    pub history_error: Option<String>,
    /// Set if the cancel flag stopped the clean before every item was done.
    pub cancelled: bool,
}

/// Carries out `plan` item by item and records it in the history. Blocking: call it from a
/// background thread. Checks `cancel` between items.
pub fn execute(plan: &CleanPlan, on_event: &mut dyn FnMut(CleanEvent), cancel: &AtomicBool) -> CleanReport {
    let mut entry = HistoryEntry::new(plan.source, plan.practice, Some(plan.policy));
    let mut errors = vec![];
    let mut cancelled = false;
    let total = plan.items.len();
    for (index, PlanItem { target: t, removal }) in plan.items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        on_event(CleanEvent::Started { index, total, label: t.label.clone() });
        let mut paths = vec![];
        let errs = clean_target_with(t, plan.practice, *removal, &mut |o| paths.push(history_path(o)));
        // bytes: the target's estimate, minus whatever was left behind
        let left: Vec<PathBuf> = paths.iter().filter(|p| p.action == PathAction::Skipped).map(|p| p.path.clone()).collect();
        let left_bytes: u64 = if left.is_empty() { 0 } else { size_of(&left).values().sum() };
        let bytes = if !t.paths.is_empty() && left.len() == t.paths.len() { 0 } else { t.bytes.saturating_sub(left_bytes) };
        entry.items.push(HistoryItem { id: t.id.clone(), label: t.label.clone(), group: Some(t.group), bytes, removal: *removal, paths });
        on_event(CleanEvent::Finished { index, bytes, errors: errs.clone() });
        errors.extend(errs);
    }
    entry.finish(errors, cancelled)
}

fn history_path(o: PathOutcome) -> HistoryPath {
    let hp = |path: PathBuf, action| HistoryPath { path, action, trashed_to: None, bytes: None, error: None, restored: false };
    match o {
        PathOutcome::Deleted(p) => hp(p, PathAction::Deleted),
        PathOutcome::Trashed { from, to } => HistoryPath { trashed_to: Some(to), ..hp(from, PathAction::Trashed) },
        PathOutcome::Practice(p, _) => hp(p, PathAction::Practice),
        PathOutcome::Skipped(p, e) => HistoryPath { error: Some(e), ..hp(p, PathAction::Skipped) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, group: Group, bytes: u64) -> Target {
        Target { id: id.into(), label: id.into(), group, default_on: true, bytes, paths: vec![], commands: vec![], detail: None, guard: Guard::None, wipes_data: false }
    }

    #[test]
    fn plan_is_pure_and_splits_by_removal() {
        let plan = CleanPlan::new(&[t("cache", Group::Cache, 10), t("logs", Group::Junk, 5)], CleanPolicy::TrashUnlessCache, HistorySource::Cli, true);
        assert!(plan.practice);
        assert_eq!(plan.items.iter().map(|i| i.removal).collect::<Vec<_>>(), [Removal::Delete, Removal::Trash]);
        assert_eq!((plan.bytes(None), plan.bytes(Some(Removal::Delete)), plan.bytes(Some(Removal::Trash))), (15, 10, 5));
    }

    #[test]
    fn progress_follows_events() {
        let mut p = CleanProgress::default();
        p.apply(&CleanEvent::Started { index: 0, total: 2, label: "a".into() });
        assert_eq!((p.total, p.current.as_deref()), (2, Some("a")));
        p.apply(&CleanEvent::Finished { index: 0, bytes: 7, errors: vec!["busy".into()] });
        assert_eq!((p.done, p.bytes_done, p.current.clone(), p.item_errors.len()), (1, 7, None, 1));
    }
}
