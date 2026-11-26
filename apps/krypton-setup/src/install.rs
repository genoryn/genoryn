//! Per-user installation (no administrator rights): files, shortcuts, uninstaller entry.

use anyhow::{Context, Result};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));
pub const APP_NAME: &str = "Genoryn Krypton";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(windows)]
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\GenorynKrypton";
pub const UNINSTALLER: &str = "Uninstall Genoryn Krypton.exe";
/// Every file an install put into the folder (one relative path per line): uninstalling
/// removes these and nothing else.
const MANIFEST: &str = "installed-files.txt";

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

/// Whether `dir` holds an install of ours (or a copy of the app).
fn is_ours(dir: &Path) -> bool {
    dir.join(MANIFEST).is_file()
        || dir.join(UNINSTALLER).is_file()
        || dir.join("krypton-wallet.exe").is_file()
}

/// The folder an install goes into: the chosen one when it is ours or empty, otherwise
/// a "Genoryn Krypton" folder inside it, so a drive, the profile or any folder with other
/// files in it is never treated as the app's own.
pub fn install_dir(chosen: &Path) -> PathBuf {
    let empty = std::fs::read_dir(chosen).map_or(true, |mut rd| rd.next().is_none());
    if chosen.parent().is_some() && (is_ours(chosen) || empty) {
        chosen.to_path_buf()
    } else {
        chosen.join(APP_NAME)
    }
}

/// The files of this installer's payload, as relative paths.
fn payload_files() -> Vec<PathBuf> {
    let Ok(mut z) = zip::ZipArchive::new(Cursor::new(PAYLOAD)) else {
        return Vec::new();
    };
    (0..z.len())
        .filter_map(|i| {
            let f = z.by_index(i).ok()?;
            (!f.is_dir()).then(|| f.enclosed_name()).flatten()
        })
        .collect()
}

/// A relative path that stays inside the install folder.
fn inside(rel: &Path) -> bool {
    !rel.as_os_str().is_empty()
        && rel
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn read_manifest(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(dir.join(MANIFEST))
        .unwrap_or_default()
        .lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| inside(p))
        .collect()
}

/// The program's own Windows tools, from the system folder only (a `powershell.exe` or
/// `cmd.exe` next to the installer, say in Downloads, is never the one run).
#[cfg(windows)]
fn system_tool(rel: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join(rel)
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
        let _ = std::process::Command::new(system_tool(r"WindowsPowerShell\v1.0\powershell.exe"))
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .output();
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    let _ = dir;
}

/// Removes leftovers of interrupted or in-place upgrades: the `.new` and `.old-*` copies
/// [`replace_file`] makes of the program's own files, and no other file.
fn sweep(dir: &Path, files: &[PathBuf]) {
    for rel in files {
        let dest = dir.join(rel);
        let _ = std::fs::remove_file(dest.with_extension("new"));
        let (Some(parent), Some(stem)) = (dest.parent(), dest.file_stem()) else {
            continue;
        };
        let aside = format!("{}.old-", stem.to_string_lossy());
        if let Ok(rd) = std::fs::read_dir(parent) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.strip_prefix(&aside)
                    .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
                {
                    let _ = std::fs::remove_file(e.path());
                }
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
    let mut files = read_manifest(&opts.dir);
    for f in payload_files()
        .into_iter()
        .chain([PathBuf::from(UNINSTALLER)])
    {
        if !files.contains(&f) {
            files.push(f);
        }
    }
    p.step.set("Preparing");
    stop_running(&opts.dir);
    sweep(&opts.dir, &files);
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
    sweep(&opts.dir, &files);
    let list: String = files.iter().map(|f| format!("{}\n", f.display())).collect();
    std::fs::write(opts.dir.join(MANIFEST), list).context("write the list of installed files")?;
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
    // Only what an install put there: other files in the folder stay, and so does the
    // folder then.
    let mut files = read_manifest(dir);
    files.extend(payload_files());
    remove_installed(dir, &files);
    p.permille.store(800, Ordering::Relaxed);
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY);
    }
    // The running uninstaller removes itself after it exits, then the folder if nothing
    // else is left in it. The path goes into a cmd line, so one with characters cmd would
    // interpret is left in place.
    #[cfg(windows)]
    if !dir
        .to_string_lossy()
        .contains(['"', '%', '^', '&', '|', '<', '>', '!'])
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "ping 127.0.0.1 -n 3 > nul & del /f /q \"{}\" & rmdir \"{}\"",
            dir.join(UNINSTALLER).display(),
            dir.display()
        );
        let _ = std::process::Command::new(system_tool("cmd.exe"))
            .args(["/C", &script])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    p.permille.store(1000, Ordering::Relaxed);
    p.step
        .set("Removed. Your wallets in %APPDATA%\\GenorynKrypton were kept.");
    Ok(())
}

/// Deletes the listed files (and the folders they leave empty) and the manifest; the
/// uninstaller itself goes after it exits.
fn remove_installed(dir: &Path, files: &[PathBuf]) {
    let files: Vec<&PathBuf> = files
        .iter()
        .filter(|f| inside(f) && f.as_os_str() != UNINSTALLER)
        .collect();
    let owned: Vec<PathBuf> = files.iter().map(|f| (*f).clone()).collect();
    sweep(dir, &owned);
    for rel in &files {
        let _ = std::fs::remove_file(dir.join(rel));
    }
    for rel in &files {
        let mut d = dir.join(rel);
        while d.pop() && d != dir && d.starts_with(dir) {
            if std::fs::remove_dir(&d).is_err() {
                break;
            }
        }
    }
    let _ = std::fs::remove_file(dir.join(MANIFEST));
}

pub fn launch(dir: &Path) {
    let _ = std::process::Command::new(dir.join("krypton-wallet.exe"))
        .current_dir(dir)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gk-setup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_folder_with_other_files_gets_its_own_subfolder() {
        let d = temp("dir");
        assert_eq!(install_dir(&d), d, "an empty folder is used as it is");
        assert_eq!(install_dir(&d.join("new")), d.join("new"));
        std::fs::write(d.join("notes.txt"), "mine").unwrap();
        assert_eq!(install_dir(&d), d.join(APP_NAME));
        std::fs::write(d.join(UNINSTALLER), "").unwrap();
        assert_eq!(install_dir(&d), d, "an install of ours is updated in place");
        let root = d.ancestors().last().unwrap().to_path_buf();
        assert_eq!(install_dir(&root), root.join(APP_NAME));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn uninstalling_removes_only_what_was_installed() {
        let d = temp("rm");
        std::fs::create_dir_all(d.join("lib")).unwrap();
        for f in [
            "krypton-wallet.exe",
            "lib/a.dll",
            "photo.jpg",
            "keep.new",
            "krypton-wallet.old-123",
            "krypton-wallet.new",
        ] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        std::fs::write(
            d.join(MANIFEST),
            "krypton-wallet.exe\nlib/a.dll\n../outside.txt\n",
        )
        .unwrap();
        let files = read_manifest(&d);
        assert_eq!(files.len(), 2, "a path out of the folder is never listed");
        remove_installed(&d, &files);
        assert!(!d.join("krypton-wallet.exe").exists());
        assert!(
            !d.join("lib").exists(),
            "an emptied folder of ours goes too"
        );
        assert!(!d.join("krypton-wallet.old-123").exists());
        assert!(!d.join("krypton-wallet.new").exists());
        assert!(!d.join(MANIFEST).exists());
        assert!(d.join("photo.jpg").exists(), "other files stay");
        assert!(
            d.join("keep.new").exists(),
            "only leftovers of our own files are swept"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
