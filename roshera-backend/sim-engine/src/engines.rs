//! The external engines a study runs through: a mesher (Gmsh, an INTERIM
//! engine until Roshera's own mesher passes its bench) and a solver
//! (CalculiX). Each is a separate program, found from configuration -- never
//! from a request -- and run without a shell, with an argument list, in a
//! working directory of its own, under a time limit. A study's report names
//! both and their versions.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Where FreeCAD 1.0 installs both engines on Windows: the default when the
/// environment names neither.
const FREECAD_BIN: &str = r"C:\Program Files\FreeCAD 1.0\bin";

/// The engines found on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Engines {
    pub mesher: EngineInfo,
    pub solver: EngineInfo,
}

/// One engine: its program and the version it reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub program: PathBuf,
    pub version: String,
    /// `true` for an engine Roshera intends to replace with its own (the
    /// report says so: "Mesher: Gmsh (interim)").
    pub interim: bool,
}

/// Why an engine could not be used.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    /// No program at the configured or default place.
    #[error("{engine} was not found (looked at {looked}); set {env} to its program")]
    NotFound {
        engine: &'static str,
        looked: String,
        env: &'static str,
    },
    /// The program could not be started.
    #[error("{engine} could not be started: {reason}")]
    Start { engine: String, reason: String },
    /// It ran past its time limit and was stopped.
    #[error("{engine} ran past {seconds} s and was stopped")]
    Timeout { engine: String, seconds: u64 },
    /// It finished with a failure; the tail of its log.
    #[error("{engine} failed (exit {code}): {tail}")]
    Failed {
        engine: String,
        code: i32,
        tail: String,
    },
}

impl Engines {
    /// The engines from `ROSHERA_GMSH` and `ROSHERA_CCX`, else FreeCAD's
    /// install; refused by name when a program is not there.
    pub fn discover() -> Result<Self, EngineError> {
        let gmsh = locate("ROSHERA_GMSH", "gmsh.exe", "Gmsh")?;
        let ccx = locate("ROSHERA_CCX", "ccx.exe", "CalculiX")?;
        Ok(Self {
            mesher: EngineInfo {
                name: "Gmsh".to_string(),
                version: version_of(&gmsh, &["--version"]),
                program: gmsh,
                interim: true,
            },
            solver: EngineInfo {
                name: "CalculiX".to_string(),
                version: version_of(&ccx, &["-v"]),
                program: ccx,
                interim: false,
            },
        })
    }
}

fn locate(env: &'static str, exe: &str, engine: &'static str) -> Result<PathBuf, EngineError> {
    let path = std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(FREECAD_BIN).join(exe));
    if path.is_file() {
        Ok(path)
    } else {
        Err(EngineError::NotFound {
            engine,
            looked: path.display().to_string(),
            env,
        })
    }
}

/// The first line a program prints for its version flag, trimmed; "unknown"
/// when it prints nothing recognisable (the run still records the program).
fn version_of(program: &Path, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .env("PATH", path_with(program))
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            text.lines()
                .map(str::trim)
                .find(|l| l.chars().any(|c| c.is_ascii_digit()))
                .unwrap_or("unknown")
                .to_string()
        }
        Err(_) => "unknown".to_string(),
    }
}

/// `PATH` with the program's own folder first: FreeCAD's engines load their
/// libraries from beside them.
fn path_with(program: &Path) -> std::ffi::OsString {
    let mut parts: Vec<PathBuf> = program
        .parent()
        .map(|p| vec![p.to_path_buf()])
        .unwrap_or_default();
    if let Some(existing) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(parts).unwrap_or_default()
}

/// Run `program args` in `cwd` with its output to `log`, stopped after
/// `limit`. Returns the wall time.
pub fn run(
    engine: &EngineInfo,
    args: &[&str],
    cwd: &Path,
    log: &Path,
    limit: Duration,
) -> Result<Duration, EngineError> {
    let started = Instant::now();
    let log_file = std::fs::File::create(log).map_err(|e| EngineError::Start {
        engine: engine.name.clone(),
        reason: format!("log {}: {e}", log.display()),
    })?;
    let log_err = log_file.try_clone().map_err(|e| EngineError::Start {
        engine: engine.name.clone(),
        reason: e.to_string(),
    })?;
    let mut child = Command::new(&engine.program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path_with(&engine.program))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| EngineError::Start {
            engine: engine.name.clone(),
            reason: e.to_string(),
        })?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(started.elapsed());
                }
                return Err(EngineError::Failed {
                    engine: engine.name.clone(),
                    code: status.code().unwrap_or(-1),
                    tail: log_tail(log),
                });
            }
            Ok(None) => {
                if started.elapsed() > limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(EngineError::Timeout {
                        engine: engine.name.clone(),
                        seconds: limit.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(EngineError::Start {
                    engine: engine.name.clone(),
                    reason: e.to_string(),
                })
            }
        }
    }
}

/// The last lines of a log, for an error message.
fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join(" | ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// An engine that is not installed is refused by name, saying where it
    /// looked and which variable to set -- never a crash or a silent default.
    #[test]
    fn a_missing_engine_is_refused_by_name() {
        let err = locate(
            "ROSHERA_TEST_NO_SUCH_ENGINE",
            "no-such-engine.exe",
            "Nothing",
        )
        .unwrap_err();
        match err {
            EngineError::NotFound { engine, env, .. } => {
                assert_eq!(engine, "Nothing");
                assert_eq!(env, "ROSHERA_TEST_NO_SUCH_ENGINE");
            }
            other => panic!("{other:?}"),
        }
    }
}
