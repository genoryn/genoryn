//! Explains why the miner stopped and chooses the next automatic attempt.
//!
//! The miner ends a failed run with `genoryn-miner stopped [kind]: reason` on
//! stderr, which the wallet sends to `genoryn-miner.log`. A run with no such line
//! crashed: on some CPUs or under some security software the RandomX JIT is
//! refused, which portable mode avoids; allocation failures need fewer threads.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Written by the wallet before each miner start; the diagnosis reads only the
/// lines after the last marker.
pub(super) const LOG_MARKER: &str = "=== GENORYN Wallet started genoryn-miner";
const MAX_RETRIES: u8 = 3;
const MAX_LOG_BYTES: u64 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Attempt {
    pub threads: u32,
    pub portable: bool,
    pub retries: u8,
}
impl Attempt {
    pub fn fresh(threads: u32) -> Self {
        Self {
            threads: threads.max(1),
            portable: false,
            retries: 0,
        }
    }
    pub fn header(&self, unix_seconds: u64) -> String {
        format!(
            "{LOG_MARKER} at unix time {unix_seconds}: {} thread(s), RandomX {} mode",
            self.threads,
            if self.portable {
                "portable"
            } else {
                "optimized"
            }
        )
    }
    fn portable(self) -> Option<Self> {
        (!self.portable && self.retries < MAX_RETRIES).then_some(Self {
            portable: true,
            retries: self.retries + 1,
            ..self
        })
    }
    fn fewer_threads(self) -> Option<Self> {
        (self.threads > 1 && self.retries < MAX_RETRIES).then_some(Self {
            threads: self.threads / 2,
            retries: self.retries + 1,
            ..self
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Diagnosis {
    pub message: String,
    pub retry: Option<Attempt>,
}

/// The last run's lines, after the final start marker.
fn last_run(log: &str) -> &str {
    log.rfind(LOG_MARKER).map_or(log, |start| &log[start..])
}

/// `(kind, reason)` from the miner's own report of why it stopped.
fn reported(run: &str) -> Option<(&str, &str)> {
    let line = run
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("genoryn-miner stopped ["))?;
    let (kind, reason) = line.split_once("]: ")?;
    Some((kind, reason.trim()))
}

fn last_line(run: &str) -> Option<&str> {
    run.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty() && !line.starts_with(LOG_MARKER))
}

/// Windows NTSTATUS codes a crashing process exits with.
fn crash_cause(code: Option<i32>) -> Option<(&'static str, bool)> {
    let (cause, memory) = match code? as u32 {
        0xC000_001D => ("an instruction this CPU does not support", false),
        0xC000_0005 => ("a memory access violation", false),
        0xC000_0409 => ("a security check failure", false),
        0xC000_0017 | 0xC000_012D | 0xC000_00FD => ("insufficient memory", true),
        _ => return None,
    };
    Some((cause, memory))
}

fn with_action(
    problem: String,
    previous: Attempt,
    retry: Option<Attempt>,
    give_up: &str,
) -> Diagnosis {
    let action = match retry {
        Some(next) if next.portable && !previous.portable => format!(
            "Restarting automatically in portable RandomX mode with {} thread(s); it is slower but works on every CPU.",
            next.threads
        ),
        Some(next) => format!(
            "Restarting automatically with {} thread(s) to use less memory.",
            next.threads
        ),
        None => give_up.to_owned(),
    };
    Diagnosis {
        message: format!("{problem} {action}"),
        retry,
    }
}

/// Why the run ended and what to try next. `exit` is the exit status as text;
/// `code` its raw value when the platform reports one.
pub(super) fn diagnose(
    log: &str,
    exit: &str,
    code: Option<i32>,
    attempt: Attempt,
    rpc_url: &str,
) -> Diagnosis {
    let run = last_run(log);
    const GIVE_UP: &str = "Automatic fixes did not help; see genoryn-miner.log in the data folder.";
    if let Some((kind, reason)) = reported(run) {
        let no_retry = |message: String| Diagnosis {
            message,
            retry: None,
        };
        return match kind {
            "unauthorized" => no_retry(format!(
                "Mining stopped: the node at {rpc_url} refused this wallet's admin token. A different GENORYN node is answering on that port, often an older version or one started from another data folder. Close it or use Node > Restart node, then start mining again."
            )),
            "node_unreachable" => no_retry(format!(
                "Mining stopped: the node at {rpc_url} did not answer. Start it on the Node page, then start mining again."
            )),
            "network_mismatch" => no_retry(format!(
                "Mining stopped: {reason}. Point Settings > Node RPC at a GENORYN mainnet node."
            )),
            "randomx" => {
                let memory = ["alloc", "memory", "cache"]
                    .iter()
                    .any(|word| reason.to_ascii_lowercase().contains(word));
                let retry = if memory {
                    attempt.fewer_threads().or_else(|| attempt.portable())
                } else {
                    attempt.portable().or_else(|| attempt.fewer_threads())
                };
                with_action(
                    format!("Mining stopped: {reason}."),
                    attempt,
                    retry,
                    GIVE_UP,
                )
            }
            _ => no_retry(format!("Mining stopped: {reason}.")),
        };
    }
    if code == Some(0) {
        return Diagnosis {
            message: "The miner finished.".into(),
            retry: None,
        };
    }
    let (cause, memory) = crash_cause(code).unwrap_or(("an unexpected crash", false));
    let retry = if memory {
        attempt.fewer_threads().or_else(|| attempt.portable())
    } else {
        attempt.portable().or_else(|| attempt.fewer_threads())
    };
    let last = last_line(run)
        .map(|line| format!(" Last log line: {line}"))
        .unwrap_or_default();
    with_action(
        format!("The miner stopped because of {cause} ({exit}).{last}"),
        attempt,
        retry,
        "Automatic fixes did not help. Security software often quarantines CPU miners: check its history for genoryn-miner.exe and allow it. Administrator rights are not needed for mining.",
    )
}

/// A plain explanation for a miner or node executable that could not be started.
pub(super) fn spawn_failure(binary: &Path, error: &std::io::Error) -> String {
    let name = binary.file_name().map_or_else(
        || binary.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    match (error.kind(), error.raw_os_error()) {
        (_, Some(4551 | 1260)) => format!(
            "Windows Smart App Control or an application control policy blocked {name} because this GENORYN build is not code-signed. Turn Smart App Control off (Windows Security > App & browser control > Smart App Control settings), or use a signed GENORYN release."
        ),
        (_, Some(225 | 226)) => format!(
            "Windows security blocked {name} as potentially unwanted software; CPU miners are often flagged. Restore it from the protection history, allow it, and start again."
        ),
        (std::io::ErrorKind::NotFound, _) => format!(
            "{name} is missing from {}. Reinstall GENORYN, or restore the file if security software removed it.",
            binary
                .parent()
                .map_or_else(String::new, |dir| dir.display().to_string())
        ),
        (std::io::ErrorKind::PermissionDenied, _) => format!(
            "The system refused to start {name} (access denied). Security software, Smart App Control or a device policy blocked it; allow it and start again. Administrator rights are not needed."
        ),
        _ => format!("Could not start {name}: {error}"),
    }
}

/// The last 64 KiB of the miner log, or nothing when it cannot be read.
pub(super) fn read_log_tail(path: &Path) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let offset = file.metadata()?.len().saturating_sub(MAX_LOG_BYTES);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    read().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RPC: &str = "http://127.0.0.1:29611";

    fn log(body: &str) -> String {
        format!(
            "genoryn-miner stopped [unauthorized]: from an older run\n{}\n{body}",
            Attempt::fresh(8).header(1)
        )
    }

    #[test]
    fn only_the_last_run_is_read() {
        let diagnosis = diagnose(
            &log(""),
            "exit code: 0xc000001d",
            Some(0xC000_001Du32 as i32),
            Attempt::fresh(8),
            RPC,
        );
        assert!(
            diagnosis.message.contains("does not support"),
            "{}",
            diagnosis.message
        );
        assert_eq!(
            diagnosis.retry,
            Some(Attempt {
                threads: 8,
                portable: true,
                retries: 1
            })
        );
        assert!(
            diagnosis
                .message
                .contains("portable RandomX mode with 8 thread(s)")
        );
    }

    #[test]
    fn a_refused_token_names_the_port_and_does_not_retry() {
        let text = log(
            "genoryn-miner stopped [unauthorized]: the node refused this admin token: node rejected request: invalid authentication",
        );
        let diagnosis = diagnose(&text, "exit code: 1", Some(1), Attempt::fresh(4), RPC);
        assert!(diagnosis.message.contains(RPC));
        assert_eq!(diagnosis.retry, None);
    }

    #[test]
    fn randomx_memory_failures_halve_threads_then_go_portable() {
        let text = log(
            "genoryn-miner stopped [randomx]: RandomX could not start on this computer: Failed to allocate cache",
        );
        let first = diagnose(&text, "exit code: 1", Some(1), Attempt::fresh(8), RPC);
        assert_eq!(
            first.retry,
            Some(Attempt {
                threads: 4,
                portable: false,
                retries: 1
            })
        );
        assert!(
            first
                .message
                .contains("with 4 thread(s) to use less memory")
        );
        let last = diagnose(&text, "exit code: 1", Some(1), Attempt::fresh(1), RPC);
        assert_eq!(
            last.retry,
            Some(Attempt {
                threads: 1,
                portable: true,
                retries: 1
            })
        );
    }

    #[test]
    fn retries_are_bounded() {
        let exhausted = Attempt {
            threads: 8,
            portable: false,
            retries: MAX_RETRIES,
        };
        let diagnosis = diagnose(
            &log(""),
            "exit code: 0xc0000005",
            Some(0xC000_0005u32 as i32),
            exhausted,
            RPC,
        );
        assert_eq!(diagnosis.retry, None);
        assert!(
            diagnosis
                .message
                .contains("Administrator rights are not needed")
        );
        let mut attempt = Attempt::fresh(16);
        let mut starts = 1;
        while let Some(next) = diagnose(&log(""), "crash", None, attempt, RPC).retry {
            attempt = next;
            starts += 1;
        }
        assert_eq!(starts, 1 + MAX_RETRIES as usize);
        assert!(attempt.portable && attempt.threads == 4);
    }

    #[test]
    fn a_clean_exit_and_the_last_line_are_reported() {
        assert_eq!(
            diagnose(&log(""), "exit code: 0", Some(0), Attempt::fresh(2), RPC).retry,
            None
        );
        let text = log("block template unavailable, retrying in 1s: connection refused");
        let crash = diagnose(
            &text,
            "exit code: 0xc0000409",
            Some(0xC000_0409u32 as i32),
            Attempt::fresh(2),
            RPC,
        );
        assert!(
            crash
                .message
                .contains("Last log line: block template unavailable")
        );
    }

    #[test]
    fn spawn_failures_explain_the_blocker() {
        let binary = Path::new("C:/GENORYN/genoryn-miner.exe");
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(spawn_failure(binary, &denied).contains("access denied"));
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(spawn_failure(binary, &missing).contains("is missing from C:/GENORYN"));
        let virus = std::io::Error::from_raw_os_error(225);
        assert!(spawn_failure(binary, &virus).contains("protection history"));
        let policy = std::io::Error::from_raw_os_error(4551);
        assert!(spawn_failure(binary, &policy).contains("Smart App Control"));
    }
}
