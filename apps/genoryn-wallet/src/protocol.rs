//! One-shot local CLI IPC. Secrets are carried only in stdin, never argv or logs.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub const ATOMIC_PER_GNR: u64 = 1_000_000_000_000;
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;

/// Sensitive editors must not retain discarded text in egui's undo buffer.
pub fn sensitive_edit(
    ui: &mut eframe::egui::Ui,
    enabled: bool,
    editor: eframe::egui::TextEdit<'_>,
) -> eframe::egui::Response {
    let response = ui.add_enabled(enabled, editor);
    if let Some(mut state) = eframe::egui::TextEdit::load_state(ui.ctx(), response.id) {
        state.clear_undoer();
        state.store(ui.ctx(), response.id);
    }
    response
}

pub struct SecretJson(pub Value);
impl Drop for SecretJson {
    fn drop(&mut self) {
        wipe_json(&mut self.0);
    }
}
pub fn wipe_json(value: &mut Value) {
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => {
            for item in a {
                wipe_json(item);
            }
        }
        Value::Object(o) => {
            for (_, item) in o.iter_mut() {
                wipe_json(item);
            }
        }
        // Proof note openings use numeric byte arrays, not only strings.
        _ => *value = Value::Null,
    }
}
pub fn encode_request(mut request: Value) -> Result<Zeroizing<Vec<u8>>> {
    let result = serde_json::to_vec(&request).map(Zeroizing::new);
    wipe_json(&mut request);
    Ok(result?)
}
pub fn sibling_binary(name: &str, configured_directory: &str) -> Result<PathBuf> {
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let directory = if configured_directory.is_empty() {
        std::env::current_exe()?
            .parent()
            .context("binary parent unavailable")?
            .to_path_buf()
    } else {
        PathBuf::from(configured_directory)
    };
    let candidate = directory.join(filename);
    ensure!(
        candidate.is_file(),
        "{} is missing from {}. Build the matching CLI binaries or configure their directory.",
        name,
        directory.display()
    );
    Ok(candidate)
}
pub fn cli_request(
    binary: &Path,
    mut request: Zeroizing<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
) -> Result<SecretJson> {
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "wallet operation cancelled"
    );
    let mut child = gui_child_command(binary)
        .arg("--json-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start wallet CLI")?;
    let input_result = child
        .stdin
        .take()
        .context("CLI stdin unavailable")?
        .write_all(&request);
    request.zeroize();
    if input_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("wallet CLI input failed");
    }
    let stdout = child.stdout.take().context("CLI stdout unavailable")?;
    let reader = thread::spawn(move || {
        let mut bytes = Zeroizing::new(Vec::new());
        stdout
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!(
                "operation cancelled; rescan before retrying a payment because submission may already have occurred"
            );
        }
        if started.elapsed() > Duration::from_secs(600) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!(
                "wallet operation exceeded ten minutes; inspect chain state before retrying a payment"
            );
        }
        thread::sleep(Duration::from_millis(50));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("CLI output reader stopped"))??;
    ensure!(
        bytes.len() <= MAX_RESPONSE as usize,
        "wallet CLI response exceeded limit"
    );
    let value: Value =
        serde_json::from_slice(&bytes).context("wallet CLI returned invalid JSON")?;
    let mut response = SecretJson(value);
    if response.0.get("ok").and_then(Value::as_bool) != Some(true) {
        let error = response
            .0
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("wallet CLI operation failed");
        // The paired CLI is trusted to provide secret-free error messages.
        bail!("{}", error.chars().take(512).collect::<String>());
    }
    ensure!(status.success(), "wallet CLI exited unsuccessfully");
    Ok(SecretJson(
        response
            .0
            .get_mut("result")
            .map(Value::take)
            .unwrap_or(Value::Null),
    ))
}
/// GUI-owned console backends keep their explicit pipes/logs without opening a
/// separate Windows console. This does not affect normal terminal CLI launches.
pub fn gui_child_command(binary: &Path) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut command = Command::new(binary);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new(binary)
    }
}
/// A child is always reaped, including when a startup event arrives after the GUI closes.
pub struct OwnedChild(Option<Child>);
impl OwnedChild {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.0.as_mut().expect("owned child present").try_wait()
    }
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill().context("could not stop owned process")?;
            }
            child.wait()?;
        }
        Ok(())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
pub fn stop_owned_child(mut child: OwnedChild) -> Result<()> {
    child.stop()
}
pub fn parse_amount(input: &str) -> Result<u64> {
    ensure!(
        input == input.trim() && !input.is_empty(),
        "enter an exact GNR amount"
    );
    let pieces: Vec<_> = input.split('.').collect();
    ensure!(
        pieces.len() <= 2 && !pieces[0].is_empty(),
        "invalid GNR amount"
    );
    ensure!(
        pieces.iter().all(|s| s.bytes().all(|b| b.is_ascii_digit())),
        "only decimal digits are allowed"
    );
    let whole: u64 = pieces[0].parse().context("amount too large")?;
    let fraction = pieces.get(1).copied().unwrap_or("");
    ensure!(
        fraction.len() <= 12,
        "GNR supports at most 12 decimal places"
    );
    let fractional: u64 = format!("{fraction:0<12}").parse()?;
    whole
        .checked_mul(ATOMIC_PER_GNR)
        .and_then(|value| value.checked_add(fractional))
        .context("amount too large")
}
pub fn format_atomic(input: &str) -> String {
    let Ok(value) = input.parse::<u128>() else {
        return "Unavailable".into();
    };
    let whole = value / u128::from(ATOMIC_PER_GNR);
    let fractional = value % u128::from(ATOMIC_PER_GNR);
    if fractional == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{:012}", fractional)
            .trim_end_matches('0')
            .to_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    #[test]
    fn gui_child_has_no_console_and_retains_captured_output() {
        let system = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows directory"));
        let powershell = system.join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let script = r#"Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class GenorynConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'; [GenorynConsoleProbe]::GetConsoleWindow().ToInt64()"#;
        let output = gui_child_command(&powershell)
            .args(["-NoProfile", "-Command", script])
            .output()
            .expect("run owned Windows console probe");
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "0");
    }
    #[test]
    fn cleared_sensitive_editor_cannot_restore_dismissed_text_with_undo() {
        use eframe::egui::{CentralPanel, Context, Event, Id, Key, Modifiers, RawInput, TextEdit};
        let context = Context::default();
        let id = Id::new("sensitive_editor_regression");
        let mut text = String::new();
        let frame = |text: &mut String, time: f64, events: Vec<Event>| {
            let _ = context.run(
                RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        sensitive_edit(ui, true, TextEdit::multiline(text).id(id));
                    });
                },
            );
        };
        frame(&mut text, 0.0, vec![]);
        context.memory_mut(|memory| memory.request_focus(id));
        frame(
            &mut text,
            0.1,
            vec![Event::Paste("ordinary discarded draft".into())],
        );
        frame(&mut text, 1.2, vec![]);
        assert_eq!(text, "ordinary discarded draft");
        text.zeroize();
        frame(&mut text, 1.3, vec![]);
        let undo = || Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                command: true,
                ctrl: true,
                ..Default::default()
            },
        };
        frame(&mut text, 1.4, vec![undo()]);
        assert!(text.is_empty());
        text = "replacement import".into();
        frame(&mut text, 2.0, vec![]);
        frame(&mut text, 2.1, vec![undo()]);
        assert_eq!(text, "replacement import");
    }
    #[test]
    fn amounts_never_use_floating_point() {
        assert_eq!(parse_amount("1.000000000001").unwrap(), 1_000_000_000_001);
        assert_eq!(parse_amount("0.000000000001").unwrap(), 1);
        for invalid in [
            "-1",
            "1e3",
            "NaN",
            "0.0000000000001",
            " 1",
            ".1",
            "18446745",
            "1.2.3",
        ] {
            assert!(parse_amount(invalid).is_err(), "{invalid}");
        }
        assert_eq!(format_atomic("1000000000001"), "1.000000000001");
        assert_eq!(format_atomic("0"), "0");
        assert_eq!(format_atomic("21000000000000000000"), "21000000");
        assert_eq!(format_atomic("unknown"), "Unavailable");
    }
    #[test]
    fn secret_json_is_wiped_recursively() {
        let mut value =
            serde_json::json!({"password":"secret","nested":["phrase"],"opening":[1,2,3]});
        wipe_json(&mut value);
        assert_eq!(value["password"], "");
        assert_eq!(value["nested"][0], "");
        assert!(
            value["opening"]
                .as_array()
                .unwrap()
                .iter()
                .all(Value::is_null)
        );
    }
    #[test]
    fn binary_discovery_never_uses_path_search() {
        assert!(sibling_binary("missing-wallet-binary", "/nonexistent").is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn dropped_owned_child_is_stopped_and_reaped() {
        let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        drop(OwnedChild::new(child));
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }
}
