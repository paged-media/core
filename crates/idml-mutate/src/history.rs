//! Undo / redo stacks. The single commitment: one `Project::apply` /
//! `undo` / `redo` call moves exactly one entry between the two
//! stacks. A `Batch` is one entry — that's what users mean by "undo
//! the last change" when the change happened to set 50 properties at
//! once.

use crate::operation::AppliedOperation;

/// Default cap on each stack. Long sessions stay bounded; the oldest
/// entry is dropped FIFO when the cap is hit. Configurable per-Project
/// for callers that have different needs.
pub const DEFAULT_HISTORY_CAPACITY: usize = 1_000;

#[derive(Debug)]
pub struct History {
    undo: Vec<AppliedOperation>,
    redo: Vec<AppliedOperation>,
    capacity: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_HISTORY_CAPACITY)
    }
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Record a freshly-applied op. Clears the redo stack — once the
    /// user takes a new action, the previously-undone redo branch is
    /// no longer reachable.
    pub fn record(&mut self, applied: AppliedOperation) {
        self.redo.clear();
        self.push_with_cap(applied);
    }

    /// Pop the most recent undo entry. Caller is responsible for
    /// applying its inverse and pushing the *resulting*
    /// `AppliedOperation` onto the redo stack via [`record_redo`].
    pub fn pop_for_undo(&mut self) -> Option<AppliedOperation> {
        self.undo.pop()
    }

    /// Pop the most recent redo entry. Caller applies its inverse
    /// (which equals the original op) and pushes that onto the undo
    /// stack via [`record_after_redo`].
    pub fn pop_for_redo(&mut self) -> Option<AppliedOperation> {
        self.redo.pop()
    }

    /// Push after an undo (so it can be redone).
    pub fn record_redo(&mut self, applied: AppliedOperation) {
        // Redo stack does not need cap pruning beyond the same cap
        // applied to undo — redo never grows beyond the matching
        // undo entries that produced it.
        self.redo.push(applied);
        while self.redo.len() > self.capacity {
            self.redo.remove(0);
        }
    }

    /// Push after a redo (so it can be undone again). Does NOT clear
    /// the redo stack — the redo branch is still meaningful (the user
    /// just walked along it, but could undo back).
    pub fn record_after_redo(&mut self, applied: AppliedOperation) {
        self.push_with_cap(applied);
    }

    fn push_with_cap(&mut self, applied: AppliedOperation) {
        self.undo.push(applied);
        while self.undo.len() > self.capacity {
            self.undo.remove(0);
        }
    }
}
