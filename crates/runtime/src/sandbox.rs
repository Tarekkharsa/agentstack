//! The backend seam: a synchronous container abstraction. The Docker backend
//! implements this via `bollard` (driving the async API on a runtime it owns
//! internally); tests implement it with an in-memory fake. The orchestration
//! layer knows only this trait.

use crate::spec::SandboxSpec;
use crate::Result;

/// Which stream a chunk of the container's output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// One chunk of streamed container output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamChunk {
    pub stream: Stream,
    pub bytes: Vec<u8>,
}

/// How a sandboxed process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// The process exit code, or `None` if it was killed by a signal (e.g.
    /// teardown).
    pub code: Option<i32>,
}

/// A container backend: creates and starts a sandbox from a spec.
pub trait Sandbox {
    /// Create the container per `spec`, apply its mounts/network/env, and
    /// start it. Returns a handle to stream and reap it.
    fn start(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>>;
}

/// A running (or finished) sandbox container.
pub trait SandboxHandle {
    /// Block until the container exits, delivering each chunk of its
    /// stdout/stderr to `on_output` as it arrives.
    fn wait_streaming(&mut self, on_output: &mut dyn FnMut(StreamChunk)) -> Result<Exit>;

    /// Remove the container and release its resources. Idempotent and
    /// best-effort-safe to call after a failed `wait_streaming` — the
    /// orchestrator always calls it so a container never leaks.
    fn teardown(&mut self) -> Result<()>;
}
