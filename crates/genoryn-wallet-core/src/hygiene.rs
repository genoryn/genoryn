//! Checks GENORYN's own storage for things that should never be there.
//!
//! Only folders GENORYN owns are read (its data folder and the wallet's
//! folder), never the rest of the computer. A finding names the file, never
//! its contents, and nothing leaves this computer. Absence of a finding does
//! not mean a recovery phrase is stored safely elsewhere: that cannot be
//! checked, and is never claimed.
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Consecutive BIP39 words that are treated as a recovery phrase.
pub const PHRASE_WORDS: usize = 12;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FILES: usize = 2_000;
const TEXT_EXTENSIONS: [&str; 7] = ["txt", "log", "json", "md", "csv", "toml", "conf"];

/// Whether `text` holds at least `PHRASE_WORDS` consecutive English BIP39 words.
pub fn contains_recovery_phrase(text: &str) -> bool {
    let mut run = 0;
    for word in text.split(|c: char| !c.is_ascii_alphabetic()) {
        if word.is_empty() {
            continue;
        }
        if word.len() >= 3
            && bip39::Language::English
                .find_word(&word.to_ascii_lowercase())
                .is_some()
        {
            run += 1;
            if run >= PHRASE_WORDS {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub file: PathBuf,
    pub issue: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StorageReport {
    pub folders: Vec<PathBuf>,
    pub files_checked: usize,
    pub findings: Vec<Finding>,
    /// The check stopped at its file limit; more files were not read.
    pub truncated: bool,
}

/// Looks through `folders` (and their direct subfolders) for plain-text
/// files that contain a recovery phrase, and on Unix for wallet files other
/// accounts can read.
pub fn check_storage(folders: &[PathBuf]) -> StorageReport {
    let mut report = StorageReport {
        folders: folders.to_vec(),
        ..Default::default()
    };
    let mut pending: Vec<(PathBuf, u8)> = folders.iter().map(|f| (f.clone(), 0)).collect();
    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if metadata.is_dir() {
                if depth == 0 {
                    pending.push((path, 1));
                }
                continue;
            }
            if report.files_checked >= MAX_FILES {
                report.truncated = true;
                return report;
            }
            report.files_checked += 1;
            check_file(&path, &metadata, &mut report.findings);
        }
    }
    report
}

fn check_file(path: &Path, metadata: &std::fs::Metadata, findings: &mut Vec<Finding>) {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    #[cfg(unix)]
    if extension == "gnr" || extension == "wallet" {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            findings.push(Finding {
                file: path.to_owned(),
                issue: "wallet file readable by other accounts on this computer (chmod 600)",
            });
        }
    }
    if !TEXT_EXTENSIONS.contains(&extension.as_str()) || metadata.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if contains_recovery_phrase(&String::from_utf8_lossy(&bytes)) {
        findings.push(Finding {
            file: path.to_owned(),
            issue: "plain-text file that appears to contain a recovery phrase",
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Wallet;
    use genoryn_core::params::Network;

    #[test]
    fn a_phrase_in_genoryn_storage_is_found_by_name_only() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let phrase = wallet.phrase().unwrap();
        assert!(contains_recovery_phrase(&phrase));
        assert!(contains_recovery_phrase(&format!(
            "note: {}\n",
            phrase.to_uppercase()
        )));
        assert!(!contains_recovery_phrase(
            "abandon ability able about above absent"
        ));
        assert!(!contains_recovery_phrase(
            "the node accepted block 1234 and the wallet synced its notes without any error today"
        ));
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        wallet
            .save_new(&root.join("w.gnr"), "test-password-1")
            .unwrap();
        std::fs::write(root.join("genorynd.log"), "accepted block\n").unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes").join("backup.txt"), phrase.as_bytes()).unwrap();
        let report = check_storage(&[root.to_owned()]);
        assert_eq!(report.findings.len(), 1, "{report:?}");
        assert!(report.findings[0].file.ends_with("backup.txt"));
        let shown = serde_json::to_string(&report).unwrap();
        assert!(
            !shown.contains(phrase.split(' ').next().unwrap()),
            "never the contents"
        );
        assert!(report.files_checked >= 3);
    }
}
