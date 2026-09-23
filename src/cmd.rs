//! External command execution behind a trait, so providers, git and herdr
//! calls can be tested against recorded output.

use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(stdout: &str) -> Self {
        Self { success: true, stdout: stdout.to_string(), stderr: String::new() }
    }

    pub fn fail(stderr: &str) -> Self {
        Self { success: false, stdout: String::new(), stderr: stderr.to_string() }
    }

    /// First non-empty stderr line, falling back to stdout; used in error rows.
    pub fn error_line(&self) -> String {
        self.stderr
            .lines()
            .chain(self.stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("command failed with no output")
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The program is not installed (not on PATH).
    NotFound(String),
    Io(String),
}

pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output, RunError>;
}

/// Runs real processes. Stdin is closed and prompts are disabled, because the
/// popup owns the terminal and a credential prompt would hang it.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output, RunError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("GLAB_NO_PROMPT", "1");
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        match command.output() {
            Ok(out) => Ok(Output {
                success: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(RunError::NotFound(program.to_string()))
            }
            Err(err) => Err(RunError::Io(format!("{program}: {err}"))),
        }
    }
}

/// Test double: answers calls from a script of `(argv prefix, result)` pairs
/// and records every call. The first matching entry is consumed, so the same
/// command can be given different answers in sequence.
type Scripted = (Vec<String>, Result<Output, RunError>);

#[derive(Default)]
pub struct ScriptedRunner {
    script: Mutex<VecDeque<Scripted>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl ScriptedRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer the next call whose argv starts with `prefix` (program first).
    pub fn on(self, prefix: &[&str], result: Result<Output, RunError>) -> Self {
        let prefix = prefix.iter().map(|s| s.to_string()).collect();
        self.script.lock().unwrap().push_back((prefix, result));
        self
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl Runner for ScriptedRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: Option<&Path>) -> Result<Output, RunError> {
        let argv: Vec<String> =
            std::iter::once(program).chain(args.iter().copied()).map(String::from).collect();
        self.calls.lock().unwrap().push(argv.clone());
        let mut script = self.script.lock().unwrap();
        let pos = script.iter().position(|(prefix, _)| argv.starts_with(prefix));
        match pos {
            Some(i) => script.remove(i).unwrap().1,
            None => Err(RunError::Io(format!("unscripted call: {}", argv.join(" ")))),
        }
    }
}
