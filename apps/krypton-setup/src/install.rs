//! Per-user installation (no administrator rights): files, shortcuts, uninstaller entry.

use anyhow::{Context, Result};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));
pub const APP_NAME: &str = "Genoryn Krypton";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\GenorynKrypton";
pub const UNINSTALLER: &str = "Uninstall Genoryn Krypton.exe";

#[derive(Clone)]
pub struct Options {
    pub dir: PathBuf,
    pub desktop: bool,
    pub start_menu: bool,
}

/// Shared progress for the UI: permille done and the current step text.
#[derive(Default)]
pub struct Progress {
    pub permille: AtomicU32,
    pub step: parking::Text,
}

pub mod parking {
    use std::sync::Mutex;
    #[derive(Default)]
    pub struct Text(Mutex<String>);
    impl Text {
        pub fn set(&self, s: &str) {
            *self.0.lock().unwrap() = s.to_string();
        }
        pub fn get(&self) -> String {
            self.0.lock().unwrap().clone()
        }
    }
}

pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Programs").join(APP_NAME)
}

/// Resolves a per-user shell folder the way Explorer does, so redirected folders
/// (OneDrive Desktop, roaming profiles) get the shortcut the user actually sees.
#[cfg(windows)]
fn shell_folder(name: &str) -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders")
        .ok()?;
    let raw: String = key.get_value(name).ok()?;
    let mut out = String::new();
    let mut rest = raw.as_str();
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else {
            break;
        };
        out.push_str(&rest[..start]);
        let var = &rest[start + 1..start + 1 + len];
        match std::env::var(var) {
            Ok(v) => out.push_str(&v),
            Err(_) => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    let p = PathBuf::from(out);
    (p.is_absolute() && p.is_dir()).then_some(p)
}

#[cfg(not(windows))]
fn shell_folder(_: &str) -> Option<PathBuf> {
    None
}

fn desktop_dir() -> Option<PathBuf> {
    shell_folder("Desktop")
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop")))
}

fn start_menu_dir() -> Option<PathBuf> {
    shell_folder("Programs")
        .or_else(|| {
            std::env::var_os("APPDATA")
                .map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"))
        })
        .map(|p| p.join(APP_NAME))
}

/// Unredirected desktop location; a stale shortcut there is removed on upgrade and uninstall.
fn legacy_desktop_lnk() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|p| {
        PathBuf::from(p)
            .join("Desktop")
            .join(format!("{APP_NAME}.lnk"))
    })
}

/// Stops wallet, node and miner processes started from `dir` (upgrade in place).
fn stop_running(dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "Get-Process krypton-wallet,kryptond,krypton-miner -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -like '{}\\*' }} | Stop-Process -Force",
            dir.display().to_string().replace('\'', "''")
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .output();
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    let _ = dir;
}

/// Removes leftovers of interrupted or in-place upgrades.
fn sweep(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".new") || n.contains(".old-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Replaces `dest` with `data`, even if `dest` is a running executable (Windows lets a
/// running .exe be renamed, not overwritten).
fn replace_file(dest: &Path, data: &[u8]) -> Result<()> {
    let tmp = dest.with_extension("new");
    std::fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    if dest.exists() && std::fs::remove_file(dest).is_err() {
        let aside = dest.with_extension(format!("old-{}", std::process::id()));
        std::fs::rename(dest, &aside).with_context(|| {
            format!(
                "{} is in use; close Genoryn Krypton and retry",
                dest.display()
            )
        })?;
    }
    std::fs::rename(&tmp, dest).with_context(|| format!("replace {}", dest.display()))?;
    Ok(())
}

pub fn install(opts: &Options, p: &Arc<Progress>) -> Result<()> {
    let mut z =
        zip::ZipArchive::new(Cursor::new(PAYLOAD)).context("this installer has no payload")?;
    std::fs::create_dir_all(&opts.dir).context("create install directory")?;
    p.step.set("Preparing");
    stop_running(&opts.dir);
    sweep(&opts.dir);
    let total: u64 = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().map(|f| f.size()))
        .sum::<u64>()
        .max(1);
    let mut done = 0u64;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let Some(rel) = f.enclosed_name() else {
            continue;
        };
        let dest = opts.dir.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        p.step.set(&format!(
            "Installing {}",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("")
        ));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)?;
        replace_file(&dest, &buf)?;
        done += buf.len() as u64;
        p.permille
            .store((done * 880 / total) as u32, Ordering::Relaxed);
    }
    p.step.set("Creating the uninstaller");
    let me = std::env::current_exe()?;
    let uninst = opts.dir.join(UNINSTALLER);
    if me != uninst {
        replace_file(&uninst, &std::fs::read(&me)?).context("copy uninstaller")?;
    }
    sweep(&opts.dir);
    p.permille.store(920, Ordering::Relaxed);
    p.step.set("Creating shortcuts");
    let wallet = opts.dir.join("krypton-wallet.exe");
    let icon = opts.dir.join("genoryn.ico");
    if opts.desktop
        && let Some(d) = desktop_dir()
    {
        let lnk = d.join(format!("{APP_NAME}.lnk"));
        shortcut(&wallet, &icon, &opts.dir, &lnk)?;
        if let Some(old) = legacy_desktop_lnk().filter(|o| *o != lnk) {
            let _ = std::fs::remove_file(old);
        }
    }
    if opts.start_menu
        && let Some(d) = start_menu_dir()
    {
        std::fs::create_dir_all(&d)?;
        shortcut(
            &wallet,
            &icon,
            &opts.dir,
            &d.join(format!("{APP_NAME}.lnk")),
        )?;
        shortcut(
            &uninst,
            &icon,
            &opts.dir,
            &d.join(format!("Uninstall {APP_NAME}.lnk")),
        )?;
    }
    p.permille.store(960, Ordering::Relaxed);
    p.step.set("Registering with Windows");
    register(&opts.dir, &uninst, &icon)?;
    p.permille.store(1000, Ordering::Relaxed);
    p.step.set("Ready");
    Ok(())
}

#[cfg(windows)]
fn shortcut(target: &Path, icon: &Path, dir: &Path, lnk: &Path) -> Result<()> {
    let mut sl = mslnk::ShellLink::new(target).map_err(|e| anyhow::anyhow!("shortcut: {e}"))?;
    sl.set_icon_location(Some(icon.to_string_lossy().into_owned()));
    sl.set_working_dir(Some(dir.to_string_lossy().into_owned()));
    sl.set_name(Some(APP_NAME.to_string()));
    sl.create_lnk(lnk)
        .map_err(|e| anyhow::anyhow!("shortcut: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn shortcut(_: &Path, _: &Path, _: &Path, _: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn register(dir: &Path, uninst: &Path, icon: &Path) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
    let size_kb: u32 = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok()?.metadata().ok())
        .map(|m| (m.len() / 1024) as u32)
        .sum();
    k.set_value("DisplayName", &APP_NAME)?;
    k.set_value("DisplayVersion", &VERSION)?;
    k.set_value("Publisher", &"Genoryn Krypton contributors")?;
    k.set_value("DisplayIcon", &icon.to_string_lossy().as_ref())?;
    k.set_value("InstallLocation", &dir.to_string_lossy().as_ref())?;
    k.set_value(
        "UninstallString",
        &format!("\"{}\" --uninstall", uninst.display()),
    )?;
    k.set_value("URLInfoAbout", &"https://genoryn.io")?;
    k.set_value("EstimatedSize", &size_kb)?;
    k.set_value("NoModify", &1u32)?;
    k.set_value("NoRepair", &1u32)?;
    Ok(())
}

#[cfg(not(windows))]
fn register(_: &Path, _: &Path, _: &Path) -> Result<()> {
    Ok(())
}

/// Removes program files, shortcuts and the uninstaller entry. Wallet data in
/// %APPDATA%\GenorynKrypton (wallets, chain) is never touched.
pub fn uninstall(dir: &Path, p: &Arc<Progress>) -> Result<()> {
    p.step.set("Stopping Genoryn Krypton");
    stop_running(dir);
    p.permille.store(200, Ordering::Relaxed);
    p.step.set("Removing shortcuts");
    for lnk in [
        desktop_dir().map(|d| d.join(format!("{APP_NAME}.lnk"))),
        legacy_desktop_lnk(),
    ]
    .into_iter()
    .flatten()
    {
        let _ = std::fs::remove_file(lnk);
    }
    if let Some(d) = start_menu_dir() {
        let _ = std::fs::remove_dir_all(d);
    }
    p.permille.store(450, Ordering::Relaxed);
    p.step.set("Removing program files");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.file_name().and_then(|n| n.to_str()) == Some(UNINSTALLER) {
                continue;
            }
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
    }
    p.permille.store(800, Ordering::Relaxed);
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY);
    }
    // The running uninstaller removes itself and the folder after it exits. The path goes
    // into a cmd line, so one with characters cmd would interpret is left in place.
    #[cfg(windows)]
    if !dir
        .to_string_lossy()
        .contains(['"', '%', '^', '&', '|', '<', '>', '!'])
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "ping 127.0.0.1 -n 3 > nul & rmdir /s /q \"{}\"",
            dir.display()
        );
        let _ = std::process::Command::new("cmd")
            .args(["/C", &script])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    p.permille.store(1000, Ordering::Relaxed);
    p.step
        .set("Removed. Your wallets in %APPDATA%\\GenorynKrypton were kept.");
    Ok(())
}

pub fn launch(dir: &Path) {
    let _ = std::process::Command::new(dir.join("krypton-wallet.exe"))
        .current_dir(dir)
        .spawn();
}
