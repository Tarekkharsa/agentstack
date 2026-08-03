//! Container **image** construction — the build-time sibling of [`spec`].
//!
//! [`spec::SandboxSpec`] describes one *run*: what to execute and how to
//! confine it. [`ImageSpec`] describes one *artifact*: what a toolset's pinned
//! capabilities compose into, so it can be run later
//! (`docs/design/packaging.md`). Both are backend-agnostic descriptions built
//! by the caller (cli) and handed to a backend here, for the same reason:
//! container mechanics belong to this crate, and the CLI stays out of them.
//!
//! **Why this backend shells out to the `docker` CLI while [`docker`] uses
//! bollard.** Two reasons, both structural rather than preference. (1) The
//! daemon's build endpoint takes a *tar stream* of the build context, and this
//! workspace has no tar writer and may not add a dependency for one. The
//! `docker` client already knows how to pack a directory. (2) bollard is
//! behind the opt-in `docker` feature — off in default builds and in CI — and
//! packaging is a headline capability that must work in a default build. So:
//! runs keep bollard, builds use argv. Nothing is duplicated between them,
//! because they do different things.
//!
//! Every string that reaches the generated Dockerfile is charset-validated
//! first and emitted in JSON form (exec-form argv, quoted label values), so no
//! repository-derived byte is ever interpolated into a shell (`CLAUDE.md`
//! invariant 7). The `docker` invocation itself is argv — there is no shell in
//! this module at all.
//!
//! [`spec`]: crate::spec
//! [`docker`]: crate::docker

use std::path::Path;
use std::process::Command;

/// Environment variable to point at a different `docker` client — a full path,
/// or a name resolved on `PATH`. Exists for the same reason
/// `AGENTSTACK_SANDBOX_IMAGE` does: a test needs to drive the
/// daemon-unavailable branch deterministically on a machine that happens to
/// have Docker.
pub const DOCKER_PROGRAM_ENV: &str = "AGENTSTACK_DOCKER";

/// What went wrong building an image. Kept separate from
/// [`RuntimeError`](crate::RuntimeError), which describes a *run*: an unsafe
/// value has no meaning there, and a caller that mixes the two loses the
/// distinction between "this artifact cannot be described" and "this container
/// misbehaved".
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// A value could not be proven safe to place in a generated file. Carries
    /// the field name and the escaped value, never the raw bytes.
    #[error("refusing to build: {field} is not a safe value for a generated Dockerfile ({value})")]
    UnsafeValue { field: String, value: String },
    /// `docker` could not be executed at all.
    #[error("docker client '{program}' could not be run: {detail}")]
    ClientMissing { program: String, detail: String },
    /// `docker` ran and refused.
    #[error("docker build failed: {detail}")]
    BuildFailed { detail: String },
}

type Result<T> = std::result::Result<T, ImageError>;

/// The backend-agnostic description of one image to build.
///
/// Deliberately holds no file *bytes*: the caller stages the payload tree into
/// the build context itself (it is the side that knows the content store and
/// the digests), and this describes the image wrapped around it. That split is
/// what keeps the store, the lock, and consent entirely out of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSpec {
    /// The `FROM` base. Defaults, at the call site, to the same image
    /// `agentstack run --sandbox` would have launched — so a packaged image is
    /// that runner plus one toolset, never a second notion of "the runner".
    pub base: String,
    /// The local tag to apply. Never pushed.
    pub tag: String,
    /// Directory inside the build context that is `COPY`d into the image.
    pub payload_dir: String,
    /// Absolute destination of that directory inside the image.
    pub payload_dest: String,
    /// `LABEL` pairs. Values are emitted JSON-quoted.
    pub labels: Vec<(String, String)>,
    /// `ENV` pairs. Names must be `[A-Z_][A-Z0-9_]*`.
    pub env: Vec<(String, String)>,
    /// `WORKDIR` — the same mount point a sandbox run uses, so a packaged
    /// image behaves the same whether or not a workspace is mounted over it.
    pub workdir: String,
    /// `ENTRYPOINT`, exec form.
    pub entrypoint: Vec<String>,
    /// `CMD`, exec form — the default argv the entrypoint execs.
    pub cmd: Vec<String>,
}

impl ImageSpec {
    /// Render the Dockerfile, or refuse.
    ///
    /// Validation is up front and total: a value that cannot be proven safe
    /// fails the whole render rather than being escaped, quoted, or dropped.
    /// That is the fail-closed shape the rest of the codebase uses for hostile
    /// input — an unrepresentable value is a signal, not something to work
    /// around.
    pub fn dockerfile(&self) -> Result<String> {
        check(&self.base, "base image")?;
        check_tag(&self.tag)?;
        check(&self.payload_dir, "payload directory")?;
        check(&self.payload_dest, "payload destination")?;
        check(&self.workdir, "workdir")?;
        for (k, v) in &self.labels {
            check(k, "label name")?;
            check(v, "label value")?;
        }
        for (k, v) in &self.env {
            check_env_name(k)?;
            check(v, "env value")?;
        }
        for a in self.entrypoint.iter().chain(self.cmd.iter()) {
            check(a, "argv entry")?;
        }
        if self.entrypoint.is_empty() {
            return Err(ImageError::UnsafeValue {
                field: "entrypoint".into(),
                value: "empty".into(),
            });
        }

        let mut out = String::new();
        out.push_str(
            "# Generated by `agentstack image` — do not edit, and do not commit.\n\
             # What this is, and what its posture label does and does not promise:\n\
             # docs/design/packaging.md\n",
        );
        out.push_str(&format!("FROM {}\n", self.base));
        for (k, v) in &self.labels {
            out.push_str(&format!("LABEL {k}={}\n", json_string(v)));
        }
        // The payload is one COPY of one directory: a single, auditable layer
        // whose contents `image.json` describes member by member.
        out.push_str(&format!(
            "COPY {} {}\n",
            self.payload_dir, self.payload_dest
        ));
        for (k, v) in &self.env {
            out.push_str(&format!("ENV {k}={}\n", json_string(v)));
        }
        out.push_str(&format!("WORKDIR {}\n", self.workdir));
        out.push_str(&format!("ENTRYPOINT {}\n", json_argv(&self.entrypoint)));
        if !self.cmd.is_empty() {
            out.push_str(&format!("CMD {}\n", json_argv(&self.cmd)));
        }
        Ok(out)
    }
}

/// Whether a `docker` client is usable here — and, when it is not, which of the
/// two distinct reasons applies. The distinction matters to the user: "install
/// Docker" and "start Docker" are different next steps, and collapsing them
/// into "Docker is unavailable" makes the message useless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerStatus {
    Available,
    /// The client binary itself could not be executed.
    ClientNotFound {
        program: String,
        detail: String,
    },
    /// The client ran; the daemon did not answer.
    DaemonUnreachable {
        program: String,
        detail: String,
    },
}

impl DockerStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, DockerStatus::Available)
    }

    /// One plain sentence naming what is missing. Kept here so every surface
    /// that reports the outage says the same thing.
    pub fn sentence(&self) -> String {
        match self {
            DockerStatus::Available => "docker is available".to_string(),
            DockerStatus::ClientNotFound { program, .. } => format!(
                "no docker client: '{program}' could not be run — install Docker, \
                 or set {DOCKER_PROGRAM_ENV} to the client you use"
            ),
            DockerStatus::DaemonUnreachable { program, .. } => format!(
                "the docker client '{program}' ran but no daemon answered — start Docker \
                 (Docker Desktop, `colima start`, or `systemctl start docker`)"
            ),
        }
    }
}

/// Which `docker` client this build would use.
pub fn docker_program() -> String {
    std::env::var(DOCKER_PROGRAM_ENV).unwrap_or_else(|_| "docker".to_string())
}

/// Probe the client and the daemon, without building anything.
///
/// `docker info` is the same liveness check the Docker-gated integration tests
/// use, for the same reason: a socket path existing is not a daemon answering.
pub fn probe() -> DockerStatus {
    let program = docker_program();
    match Command::new(&program).arg("info").output() {
        Err(e) => DockerStatus::ClientNotFound {
            program,
            detail: e.to_string(),
        },
        Ok(out) if !out.status.success() => DockerStatus::DaemonUnreachable {
            program,
            // Bounded and control-stripped: this is daemon output, and it is
            // about to be printed to a terminal.
            detail: first_clean_line(&String::from_utf8_lossy(&out.stderr)),
        },
        Ok(_) => DockerStatus::Available,
    }
}

/// The exact argv a user can run by hand to finish a staged build. Returned as
/// a vector so the caller can both display it and (in the happy path) run it —
/// the displayed line and the executed one can never disagree.
pub fn build_argv(context: &Path, tag: &str) -> Vec<String> {
    vec![
        docker_program(),
        "build".to_string(),
        "--tag".to_string(),
        tag.to_string(),
        context.display().to_string(),
    ]
}

/// Build the staged context. Streams the client's own output to this process's
/// stdout/stderr — there is no reason to buffer a build log, and a user
/// watching a slow build needs to see it move.
pub fn build(context: &Path, tag: &str) -> Result<()> {
    check_tag(tag)?;
    let program = docker_program();
    let status = Command::new(&program)
        .arg("build")
        .arg("--tag")
        .arg(tag)
        .arg(context)
        .status()
        .map_err(|e| ImageError::ClientMissing {
            program: program.clone(),
            detail: e.to_string(),
        })?;
    if !status.success() {
        return Err(ImageError::BuildFailed {
            detail: match status.code() {
                Some(c) => format!("`{program} build` exited {c}"),
                None => format!("`{program} build` was terminated by a signal"),
            },
        });
    }
    Ok(())
}

// ── validation ─────────────────────────────────────────────────────────────

/// Bound on any single generated value. Generous for a path or an image
/// reference, far below anything that could be a payload.
const VALUE_MAX: usize = 512;

/// A value safe to place in a generated Dockerfile: non-empty, bounded, and
/// printable ASCII with no quote, no backslash, and no `$`.
///
/// `$` is excluded even though the fields here are not shell lines, because
/// Dockerfile instructions do their own variable expansion — a `$` in a label
/// or an `ENV` value would be substituted by the builder, which is exactly the
/// class of surprise invariant 7 exists to prevent.
fn is_safe(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= VALUE_MAX
        && s.bytes()
            .all(|b| (0x20..=0x7e).contains(&b) && b != b'"' && b != b'\\' && b != b'$')
}

fn check(s: &str, field: &str) -> Result<()> {
    if is_safe(s) {
        return Ok(());
    }
    Err(ImageError::UnsafeValue {
        field: field.to_string(),
        value: s.escape_debug().to_string(),
    })
}

/// A tag additionally may not start with `-`, or `docker build` would read it
/// as a flag.
fn check_tag(tag: &str) -> Result<()> {
    check(tag, "tag")?;
    if tag.starts_with('-') {
        return Err(ImageError::UnsafeValue {
            field: "tag".into(),
            value: tag.escape_debug().to_string(),
        });
    }
    Ok(())
}

fn check_env_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        return Ok(());
    }
    Err(ImageError::UnsafeValue {
        field: "env name".into(),
        value: name.escape_debug().to_string(),
    })
}

/// A JSON string literal. Values are already validated to hold no quote,
/// backslash, or control byte, so this is a plain wrap — but it goes through
/// `serde_json` rather than `format!("\"{s}\"")` so the escaping rule lives in
/// one library rather than in an assumption.
fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

fn json_argv(argv: &[String]) -> String {
    serde_json::Value::Array(argv.iter().map(|a| a.as_str().into()).collect()).to_string()
}

/// First non-empty line, control characters dropped, bounded. Daemon output is
/// untrusted text headed for a terminal.
fn first_clean_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            l.chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect::<String>()
        })
        .unwrap_or_else(|| "no detail".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ImageSpec {
        ImageSpec {
            base: "agentstack/sandbox:latest".into(),
            tag: "agentstack-toolset/backend:latest".into(),
            payload_dir: "agentstack".into(),
            payload_dest: "/agentstack".into(),
            labels: vec![("org.agentstack.toolset".into(), "backend".into())],
            env: vec![("HOME".into(), "/agentstack/home".into())],
            workdir: "/workspace".into(),
            entrypoint: vec!["/agentstack/entrypoint.sh".into()],
            cmd: vec!["claude".into()],
        }
    }

    #[test]
    fn renders_exec_form_and_one_payload_layer() {
        let text = spec().dockerfile().expect("safe spec renders");
        assert!(text.contains("FROM agentstack/sandbox:latest\n"));
        assert!(text.contains("COPY agentstack /agentstack\n"));
        assert!(text.contains("ENV HOME=\"/agentstack/home\"\n"));
        assert!(text.contains("WORKDIR /workspace\n"));
        // Exec form, not shell form: no `/bin/sh -c` between docker and the
        // harness, so nothing gets a shell to interpret.
        assert!(text.contains("ENTRYPOINT [\"/agentstack/entrypoint.sh\"]\n"));
        assert!(text.contains("CMD [\"claude\"]\n"));
    }

    /// Invariant 7 at this seam: a value carrying shell or Dockerfile
    /// metacharacters is refused, never escaped into the file.
    #[test]
    fn hostile_values_are_refused_rather_than_escaped() {
        for hostile in [
            "backend\"; RUN curl evil",
            "back\\end",
            "back$end",
            "back\nend",
            "back\u{1b}[31mend",
            "",
        ] {
            let mut s = spec();
            s.labels = vec![("org.agentstack.toolset".into(), hostile.to_string())];
            assert!(
                s.dockerfile().is_err(),
                "hostile label value {hostile:?} must refuse"
            );
        }
        let mut s = spec();
        s.tag = "-rm".into();
        assert!(s.dockerfile().is_err(), "a flag-shaped tag must refuse");
    }

    #[test]
    fn env_names_are_constrained() {
        let mut s = spec();
        s.env = vec![("home".into(), "/x".into())];
        assert!(s.dockerfile().is_err(), "lowercase env name must refuse");
    }

    #[test]
    fn build_argv_matches_what_a_user_would_type() {
        let argv = build_argv(Path::new("/tmp/ctx"), "t:1");
        assert_eq!(&argv[1..], ["build", "--tag", "t:1", "/tmp/ctx"]);
    }
}
