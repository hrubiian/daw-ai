//! Snapshot-based undo/redo. `Project` is a small cloneable value, so each
//! undo step stores a full labelled snapshot; same-label records arriving
//! within the coalescing window collapse into one step (a drag gesture is a
//! single undo). The clock is injected (`record_at`) for deterministic tests.

use crate::project::Project;

/// Same-label records closer together than this collapse into one undo step.
pub const COALESCE_MS: u64 = 800;
/// Maximum retained undo steps; the oldest is dropped on overflow.
pub const MAX_DEPTH: usize = 100;

#[derive(Default)]
pub struct UndoStack {
    undo: Vec<(String, Project)>,
    redo: Vec<(String, Project)>,
    /// Timestamp (ms) of the last accepted or coalesced `record`.
    last_ms: Option<u64>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pre-edit snapshot with a real clock. See `record_at`.
    pub fn record(&mut self, label: &str, current: &Project) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.record_at(label, current, now_ms);
    }

    /// Record a pre-edit snapshot. Clears the redo stack. Skips the push when
    /// the top entry has the same label and the previous record was less than
    /// `COALESCE_MS` ago (the gesture continues; its pre-state is already
    /// captured).
    pub fn record_at(&mut self, label: &str, current: &Project, now_ms: u64) {
        self.redo.clear();
        let coalesce = match (self.undo.last(), self.last_ms) {
            (Some((l, _)), Some(t)) => l == label && now_ms.saturating_sub(t) < COALESCE_MS,
            _ => false,
        };
        self.last_ms = Some(now_ms);
        if coalesce {
            return;
        }
        self.undo.push((label.to_string(), current.clone()));
        if self.undo.len() > MAX_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Pop the last snapshot; the displaced `current` goes onto redo.
    pub fn undo(&mut self, current: Project) -> Option<(String, Project)> {
        let (label, snap) = self.undo.pop()?;
        self.redo.push((label.clone(), current));
        // A subsequent record must not coalesce with a pre-undo gesture.
        self.last_ms = None;
        Some((label, snap))
    }

    pub fn redo(&mut self, current: Project) -> Option<(String, Project)> {
        let (label, snap) = self.redo.pop()?;
        self.undo.push((label.clone(), current));
        self.last_ms = None;
        Some((label, snap))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj_with_bpm(bpm: f64) -> Project {
        let mut p = Project::default_project();
        p.project.bpm = bpm;
        p
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut s = UndoStack::new();
        let a = proj_with_bpm(100.0);
        let b = proj_with_bpm(120.0);
        s.record_at("bpm", &a, 0);
        // current state is now b; undo restores a.
        let (label, restored) = s.undo(b.clone()).unwrap();
        assert_eq!(label, "bpm");
        assert_eq!(restored.project.bpm, 100.0);
        // redo brings b back.
        let (_, redone) = s.redo(restored).unwrap();
        assert_eq!(redone.project.bpm, 120.0);
    }

    #[test]
    fn new_record_clears_redo() {
        let mut s = UndoStack::new();
        s.record_at("a", &proj_with_bpm(100.0), 0);
        let _ = s.undo(proj_with_bpm(110.0)).unwrap();
        assert!(s.can_redo());
        s.record_at("b", &proj_with_bpm(100.0), 5_000);
        assert!(!s.can_redo());
    }

    #[test]
    fn same_label_coalesces_within_window() {
        let mut s = UndoStack::new();
        s.record_at("volume", &proj_with_bpm(100.0), 0);
        s.record_at("volume", &proj_with_bpm(101.0), 200);
        s.record_at("volume", &proj_with_bpm(102.0), 400);
        // one step: undo restores the pre-gesture state (bpm 100).
        let (_, restored) = s.undo(proj_with_bpm(103.0)).unwrap();
        assert_eq!(restored.project.bpm, 100.0);
        assert!(!s.can_undo());
    }

    #[test]
    fn different_label_or_elapsed_window_does_not_coalesce() {
        let mut s = UndoStack::new();
        s.record_at("volume", &proj_with_bpm(100.0), 0);
        s.record_at("pan", &proj_with_bpm(101.0), 100); // different label
        s.record_at("pan", &proj_with_bpm(102.0), 2_000); // window elapsed
        assert_eq!(s.undo.len(), 3);
    }

    #[test]
    fn coalescing_window_chains_across_a_held_gesture() {
        // Records every 200 ms for 2 s: still one step (each record renews
        // the window).
        let mut s = UndoStack::new();
        for i in 0..10 {
            s.record_at("drag", &proj_with_bpm(100.0 + i as f64), i * 200);
        }
        assert_eq!(s.undo.len(), 1);
    }

    #[test]
    fn depth_cap_drops_oldest() {
        let mut s = UndoStack::new();
        for i in 0..(MAX_DEPTH as u64 + 10) {
            s.record_at(&format!("e{i}"), &proj_with_bpm(i as f64), i * 10_000);
        }
        assert_eq!(s.undo.len(), MAX_DEPTH);
        assert_eq!(s.undo.first().unwrap().0, "e10");
    }

    #[test]
    fn empty_stacks_return_none() {
        let mut s = UndoStack::new();
        assert!(s.undo(proj_with_bpm(1.0)).is_none());
        assert!(s.redo(proj_with_bpm(1.0)).is_none());
        assert!(s.undo_label().is_none());
    }

    #[test]
    fn no_coalesce_across_undo() {
        // gesture → undo → same-label record again quickly: must NOT coalesce
        // into the (now-popped) old entry.
        let mut s = UndoStack::new();
        s.record_at("volume", &proj_with_bpm(100.0), 0);
        let _ = s.undo(proj_with_bpm(101.0)).unwrap();
        s.record_at("volume", &proj_with_bpm(100.0), 100);
        assert_eq!(s.undo.len(), 1);
        assert!(s.can_undo());
    }
}
