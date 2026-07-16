//! A bounded writer-thread spool that keeps event-sink I/O off the async
//! proxy workers.
//!
//! [`ServerProxy`](crate::proxy::ServerProxy) emits every egress decision
//! through its [`EventSink`](crate::proxy::EventSink) from inside a tokio
//! task, and the lockdown log-follower does the same with its sink. The
//! production sinks do real blocking I/O — the CLI appends to the run's
//! `events.jsonl`, the sidecar binary writes stdout — while the proxy
//! runtimes run with only 1–2 worker threads
//! ([`BlockingBridge`](crate::blocking::BlockingBridge) uses one). A single
//! slow write inside a sink therefore used to park a worker and stall every
//! in-flight tunnel on that listener.
//!
//! The spool decouples them: the sink closure just enqueues onto a bounded
//! `std::sync::mpsc::sync_channel` and one dedicated OS thread performs the
//! writes, preserving arrival order (single consumer, FIFO channel).
//!
//! At the bound the spool **sheds** (drops the event) rather than blocks:
//! blocking would reintroduce the exact stall this module exists to remove,
//! and every sink routed through here is best-effort by contract —
//! `RunLog::append` already swallows failures, and the recorder's
//! must-succeed `append_checked` path (locked-run material events) is never
//! called through an `EventSink`, so its guarantee is untouched. A shed is
//! reported once to stderr so an incomplete audit log is visible, not silent.
//!
//! Dropping the spool flushes it: the writer thread drains the queue and is
//! joined once every [`SpoolSender`] is gone. Callers must therefore drop the
//! spool **after** whatever holds the sink (the bridge / lockdown handle), or
//! the join would wait on sender clones that are still alive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Queue depth before shedding. Events are small (one serialized decision
/// line), so this is ~1 MiB of buffer at worst — deep enough to absorb a
/// multi-second filesystem stall at any realistic decision rate, small enough
/// that a wedged disk can't grow memory without bound.
const CAPACITY: usize = 1024;

/// A dedicated writer thread fed by a bounded channel. Create one per
/// production sink, hand [`sender`](WriterSpool::spawn) handles to the sink
/// closures, and keep the spool alive for the run.
pub struct WriterSpool<T> {
    // `Option` so Drop can `take()` them: the sender must drop BEFORE the
    // join (the thread only exits once every sender is gone), and JoinHandle's
    // `join` consumes it — a plain field couldn't be moved out of `&mut self`.
    tx: Option<SyncSender<T>>,
    writer: Option<JoinHandle<()>>,
    shed: Arc<AtomicBool>,
    name: &'static str,
}

impl<T: Send + 'static> WriterSpool<T> {
    /// Spawn the writer thread. `write` runs there for every enqueued item,
    /// in arrival order; it may block freely without affecting enqueuers.
    /// Errs only if the OS refuses the thread — callers should fail the run
    /// setup rather than fall back to writing on the async workers.
    pub fn spawn(
        name: &'static str,
        mut write: impl FnMut(T) + Send + 'static,
    ) -> std::io::Result<WriterSpool<T>> {
        let (tx, rx) = sync_channel::<T>(CAPACITY);
        let writer = std::thread::Builder::new()
            .name(format!("agentstack-spool-{name}"))
            .spawn(move || {
                // Iterating a Receiver yields until every sender has dropped,
                // then ends — so the join in Drop is also the flush.
                for item in rx {
                    write(item);
                }
            })?;
        Ok(WriterSpool {
            tx: Some(tx),
            writer: Some(writer),
            shed: Arc::new(AtomicBool::new(false)),
            name,
        })
    }

    /// A cheap handle for a sink closure to enqueue with. Never blocks.
    pub fn sender(&self) -> SpoolSender<T> {
        SpoolSender {
            tx: self
                .tx
                .clone()
                .expect("spool senders are taken only in Drop"),
            shed: Arc::clone(&self.shed),
            name: self.name,
        }
    }
}

impl<T> Drop for WriterSpool<T> {
    fn drop(&mut self) {
        // Disconnect our sender first; once the sink holders have dropped
        // theirs too, the writer drains what's queued and exits — the join
        // guarantees every accepted event is written before the run returns.
        self.tx.take();
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

/// The enqueue side handed to `EventSink` / `LockdownSink` closures.
pub struct SpoolSender<T> {
    tx: SyncSender<T>,
    shed: Arc<AtomicBool>,
    name: &'static str,
}

impl<T> SpoolSender<T> {
    /// Enqueue without ever blocking the caller. At the bound the item is
    /// shed (see the module docs for why shedding beats blocking here), with
    /// a one-time stderr notice; after the spool is gone it is a no-op.
    pub fn send(&self, item: T) {
        match self.tx.try_send(item) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                // `swap` returns the PREVIOUS value, so exactly one enqueuer
                // wins the right to print the notice.
                if !self.shed.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "agentstack: the '{}' event spool is full — shedding events \
                         (this run's audit log may be incomplete)",
                        self.name
                    );
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Drop flushes: everything enqueued before the spool drops is written,
    /// in order, even though the writes happen on another thread.
    #[test]
    fn drop_joins_the_writer_and_flushes_queued_items() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let spool = WriterSpool::spawn("test-flush", move |n: u32| {
            sink.lock().unwrap().push(n);
        })
        .unwrap();
        let tx = spool.sender();
        for n in 0..100 {
            tx.send(n);
        }
        drop(tx); // the sink's handle goes first, as in real teardown
        drop(spool);
        assert_eq!(*seen.lock().unwrap(), (0..100).collect::<Vec<_>>());
    }
}
