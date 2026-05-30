//! Tiny pub-sub. Subscribers register a closure; every successful
//! `Project::apply` / `Project::undo` / `Project::redo` fans out the
//! resulting `AppliedOperation`. Single-threaded (matches the wasm
//! main-thread inspector model); native callers needing thread-safety
//! wrap in their own channel.
//!
//! Fires exactly once per top-level Op — a `Batch` produces one event,
//! not N. Consumers that care about per-child detail can walk the
//! `Operation::Batch(...)` payload themselves.

use crate::operation::AppliedOperation;

#[derive(Default)]
pub struct Notifier {
    listeners: Vec<Box<dyn FnMut(&AppliedOperation)>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe<F: FnMut(&AppliedOperation) + 'static>(&mut self, f: F) {
        self.listeners.push(Box::new(f));
    }

    pub fn notify(&mut self, applied: &AppliedOperation) {
        for listener in &mut self.listeners {
            listener(applied);
        }
    }

    pub fn listener_count(&self) -> usize {
        self.listeners.len()
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("listener_count", &self.listeners.len())
            .finish()
    }
}
