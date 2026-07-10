//! The lifecycle driver: take a sandbox through start → stream → teardown,
//! emitting flight-recorder events, with teardown guaranteed on every path so
//! a container never leaks. Backend-agnostic — it knows only the [`Sandbox`]
//! trait — which is what lets the whole flow be tested against a fake.

use agentstack_recorder::{now_epoch, RunEvent};

use crate::sandbox::{Exit, Sandbox, StreamChunk};
use crate::spec::SandboxSpec;
use crate::Result;

/// Run one sandboxed bundle to completion.
///
/// `on_output` receives each chunk of the container's stdout/stderr as it
/// streams; `on_event` receives each flight-recorder event (the caller
/// persists them, e.g. via `RunLog::append`). Teardown runs on every exit
/// path — success, a streaming error, or a panic-free early return — so the
/// container is always reaped.
pub fn run(
    sandbox: &dyn Sandbox,
    spec: &SandboxSpec,
    on_output: &mut dyn FnMut(StreamChunk),
    on_event: &mut dyn FnMut(RunEvent),
) -> Result<Exit> {
    on_event(RunEvent::SandboxStarted {
        ts: now_epoch(),
        image: spec.image.clone(),
        workspace: spec.workspace().to_string(),
    });

    let mut handle = sandbox.start(spec)?;

    // Stream to exit, then ALWAYS tear down — even if streaming errored, so a
    // failed run can't leak a container. The streaming error (the primary
    // failure) wins over a teardown error when both occur.
    let wait_result = handle.wait_streaming(on_output);
    let teardown_result = handle.teardown();

    let exit = wait_result?;
    teardown_result?;

    on_event(RunEvent::SandboxExited {
        ts: now_epoch(),
        code: exit.code,
    });
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{SandboxHandle, Stream};
    use crate::spec::{NetworkPolicy, SandboxSpec};
    use crate::RuntimeError;
    use std::cell::Cell;
    use std::rc::Rc;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            image: "agentstack/sandbox".into(),
            command: vec!["claude".into()],
            mounts: vec![crate::spec::Mount {
                host: "/proj".into(),
                container: "/workspace".into(),
                read_only: false,
            }],
            workdir: "/workspace".into(),
            env: vec![],
            network: NetworkPolicy::None,
            ruleset: agentstack_policy::CompiledRuleset::default(),
        }
    }

    /// A fake backend recording what it was driven through.
    struct FakeSandbox {
        chunks: Vec<StreamChunk>,
        exit: Exit,
        wait_err: bool,
        torn_down: Rc<Cell<bool>>,
    }

    struct FakeHandle {
        chunks: Vec<StreamChunk>,
        exit: Exit,
        wait_err: bool,
        torn_down: Rc<Cell<bool>>,
    }

    impl Sandbox for FakeSandbox {
        fn start(&self, _spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
            Ok(Box::new(FakeHandle {
                chunks: self.chunks.clone(),
                exit: self.exit.clone(),
                wait_err: self.wait_err,
                torn_down: Rc::clone(&self.torn_down),
            }))
        }
    }

    impl SandboxHandle for FakeHandle {
        fn wait_streaming(&mut self, on_output: &mut dyn FnMut(StreamChunk)) -> Result<Exit> {
            for c in &self.chunks {
                on_output(c.clone());
            }
            if self.wait_err {
                return Err(RuntimeError::Backend("stream broke".into()));
            }
            Ok(self.exit.clone())
        }
        fn teardown(&mut self) -> Result<()> {
            self.torn_down.set(true);
            Ok(())
        }
    }

    struct FailingStart;
    impl Sandbox for FailingStart {
        fn start(&self, _spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
            Err(RuntimeError::Backend("no daemon".into()))
        }
    }

    #[test]
    fn happy_path_streams_output_and_emits_start_then_exit() {
        let torn = Rc::new(Cell::new(false));
        let sandbox = FakeSandbox {
            chunks: vec![
                StreamChunk {
                    stream: Stream::Stdout,
                    bytes: b"hello".to_vec(),
                },
                StreamChunk {
                    stream: Stream::Stderr,
                    bytes: b"warn".to_vec(),
                },
            ],
            exit: Exit { code: Some(0) },
            wait_err: false,
            torn_down: Rc::clone(&torn),
        };

        let mut out = Vec::new();
        let mut events = Vec::new();
        let exit = run(
            &sandbox,
            &spec(),
            &mut |c| out.push(c),
            &mut |e| events.push(e),
        )
        .unwrap();

        assert_eq!(exit, Exit { code: Some(0) });
        assert_eq!(out.len(), 2, "both chunks streamed through");
        assert!(torn.get(), "container was torn down");
        assert!(matches!(events[0], RunEvent::SandboxStarted { .. }));
        match &events[1] {
            RunEvent::SandboxExited { code, .. } => assert_eq!(*code, Some(0)),
            other => panic!("expected SandboxExited, got {other:?}"),
        }
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn start_failure_propagates_and_emits_no_exit() {
        let mut events = Vec::new();
        let err = run(
            &FailingStart,
            &spec(),
            &mut |_| {},
            &mut |e| events.push(e),
        )
        .unwrap_err();
        assert!(matches!(err, RuntimeError::Backend(_)));
        // Started was emitted before the failed start; Exited never is.
        assert!(matches!(events.as_slice(), [RunEvent::SandboxStarted { .. }]));
    }

    #[test]
    fn streaming_error_still_tears_down_and_reports_the_error() {
        let torn = Rc::new(Cell::new(false));
        let sandbox = FakeSandbox {
            chunks: vec![],
            exit: Exit { code: None },
            wait_err: true,
            torn_down: Rc::clone(&torn),
        };
        let mut events = Vec::new();
        let err = run(&sandbox, &spec(), &mut |_| {}, &mut |e| events.push(e)).unwrap_err();
        assert!(matches!(err, RuntimeError::Backend(_)));
        assert!(torn.get(), "teardown must run even when streaming failed");
        // No SandboxExited on a failed run.
        assert!(matches!(events.as_slice(), [RunEvent::SandboxStarted { .. }]));
    }
}
