//! The pods being streamed: which are Running, which streams run, and
//! which pods had their first stream already (`--tail-lines` only once).

use std::collections::{HashMap, HashSet};

use tokio::task::JoinHandle;
use tracing::info;

use super::stream::{StreamCtx, try_start};

pub(super) struct Pods {
    ctx: StreamCtx,
    tail_lines: i64,
    since_seconds: i64,
    /// Source of truth: pods known to be in Running phase, mapped to the
    /// container keys to stream for each pod.
    running: HashMap<String, Vec<String>>,
    /// (pod, container_key) pairs with a running stream task;
    /// container_key "" means the K8s default container.
    active: HashMap<(String, String), JoinHandle<()>>,
    /// Pods that have already had their first stream started. The first
    /// stream uses --tail-lines; reconnects use --since-seconds instead.
    initialized: HashSet<String>,
}

impl Pods {
    pub(super) fn new(ctx: StreamCtx, tail_lines: i64, since_seconds: i64) -> Self {
        Self {
            ctx,
            tail_lines,
            since_seconds,
            running: HashMap::new(),
            active: HashMap::new(),
            initialized: HashSet::new(),
        }
    }

    pub(super) fn running_count(&self) -> usize {
        self.running.len()
    }

    /// The pod `name` is Running: remember its containers and start the
    /// streams that do not run yet.
    pub(super) fn running(&mut self, name: String, container_keys: Vec<String>) {
        self.running.insert(name.clone(), container_keys);
        self.start(&name);
    }

    /// The pod `name` left the Running phase. `None` if it was not known
    /// as Running, else how many streams were aborted.
    pub(super) fn not_running(&mut self, name: &str) -> Option<usize> {
        self.running.remove(name)?;
        Some(self.abort(name))
    }

    /// The pod `name` was deleted.
    pub(super) fn deleted(&mut self, name: &str) {
        self.running.remove(name);
        self.initialized.remove(name);
        self.abort(name);
    }

    /// Forget finished streams (the API server closes them, typically every
    /// ~5 minutes) and start them again for pods that still run.
    pub(super) fn reconcile(&mut self) {
        self.active.retain(|(pod, container), handle| {
            if handle.is_finished() {
                info!(pod = %pod, container = %container, "stream finished — scheduling restart");
                false
            } else {
                true
            }
        });
        let names: Vec<String> = self.running.keys().cloned().collect();
        for name in names {
            self.start(&name);
        }
    }

    /// Start the streams of `name` that do not run yet.
    fn start(&mut self, name: &str) {
        let Some(container_keys) = self.running.get(name).cloned() else {
            return;
        };
        // First connect uses --tail-lines; reconnects --since-seconds only.
        let tail_lines = if self.initialized.contains(name) {
            0
        } else {
            self.tail_lines
        };
        let mut any_started = false;
        for ck in &container_keys {
            if try_start(
                &self.ctx,
                &mut self.active,
                name,
                ck,
                tail_lines,
                self.since_seconds,
            ) {
                any_started = true;
            }
        }
        if any_started {
            self.initialized.insert(name.to_owned());
        }
    }

    /// Abort every stream of `name`; returns how many.
    fn abort(&mut self, name: &str) -> usize {
        let mut aborted = 0usize;
        self.active.retain(|(pod, _), handle| {
            if pod == name {
                handle.abort();
                aborted += 1;
                false
            } else {
                true
            }
        });
        aborted
    }
}
