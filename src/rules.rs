use std::path::{Path, PathBuf};

use crate::util::env_dir;

/// A filename filter, expressed without a glob dependency.
#[derive(Clone, Debug)]
pub struct FilePred {
    pub prefix: Option<&'static str>,
    pub suffix: Option<&'static str>,
}

impl FilePred {
    pub const fn any() -> Self {
        Self {
            prefix: None,
            suffix: None,
        }
    }

    pub const fn new(prefix: Option<&'static str>, suffix: Option<&'static str>) -> Self {
        Self { prefix, suffix }
    }

    pub fn matches(&self, name: &str) -> bool {
        if let Some(prefix) = self.prefix {
            if !name.starts_with(prefix) {
                return false;
            }
        }
        if let Some(suffix) = self.suffix {
            if !name.ends_with(suffix) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
pub enum RuleKind {
    /// Whole directory trees, moved as a unit.
    Dirs(Vec<PathBuf>),
    /// Individual files inside the given directories whose names match.
    Files { dirs: Vec<PathBuf>, pred: FilePred },
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// True when the targets live under locations only an elevated process may write to.
    pub needs_admin: bool,
    pub kind: RuleKind,
}

fn system_root() -> PathBuf {
    env_dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

fn system_drive() -> PathBuf {
    env_dir("SystemDrive").unwrap_or_else(|| PathBuf::from("C:"))
}

fn local_app_data() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA")
}

fn program_data() -> Option<PathBuf> {
    env_dir("PROGRAMDATA")
}

/// Every immediate sub-directory of `base` (used to expand browser profiles).
fn sub_dirs(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push(path);
            }
        }
    }
    out
}

fn existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|p| p.exists()).collect()
}

/// Named cache directories that browsers create per profile.
const BROWSER_CACHE_DIRS: [&str; 3] = ["Cache", "Code Cache", "GPUCache"];

fn browser_cache_dirs(user_data: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for profile in sub_dirs(user_data) {
        for name in BROWSER_CACHE_DIRS {
            let candidate = profile.join(name);
            if candidate.is_dir() {
                out.push(candidate);
            }
        }
    }
    out
}

/// The whitelist. This is the *only* set of paths the tool will ever touch.
///
/// Anything not listed here is unreachable by `engine::apply`, no matter what
/// the UI or a corrupted report says.
pub fn all_rules() -> Vec<Rule> {
    let sys = system_root();
    let mut rules = Vec::new();

    rules.push(Rule {
        id: "user_temp",
        name: "User temporary files",
        description: "Your per-user %TEMP% folder. Safe to clear; programs recreate it.",
        needs_admin: false,
        kind: RuleKind::Dirs(existing(env_dir("TEMP").into_iter().collect())),
    });

    rules.push(Rule {
        id: "windows_temp",
        name: "Windows temporary files",
        description: r"%SystemRoot%\Temp. Requires elevation.",
        needs_admin: true,
        kind: RuleKind::Dirs(existing(vec![sys.join("Temp")])),
    });

    rules.push(Rule {
        id: "wu_download",
        name: "Windows Update download cache",
        description: r"%SystemRoot%\SoftwareDistribution\Download. Windows re-downloads updates if needed. Requires elevation.",
        needs_admin: true,
        kind: RuleKind::Dirs(existing(vec![sys.join("SoftwareDistribution").join("Download")])),
    });

    rules.push(Rule {
        id: "wer",
        name: "Windows Error Reporting files",
        description: r"%ProgramData%\Microsoft\Windows\WER. Crash reports queued and already sent. Requires elevation.",
        needs_admin: true,
        kind: RuleKind::Dirs(existing(
            program_data()
                .map(|p| p.join("Microsoft").join("Windows").join("WER"))
                .into_iter()
                .collect(),
        )),
    });

    rules.push(Rule {
        id: "crash_dumps",
        name: "User crash dumps",
        description: r"%LOCALAPPDATA%\CrashDumps. Memory dumps captured when a program crashes.",
        needs_admin: false,
        kind: RuleKind::Dirs(existing(
            local_app_data()
                .map(|p| p.join("CrashDumps"))
                .into_iter()
                .collect(),
        )),
    });

    rules.push(Rule {
        id: "thumbcache",
        name: "Thumbnail and icon cache",
        description: r"thumbcache_*.db and iconcache_*.db under %LOCALAPPDATA%\Microsoft\Windows\Explorer. Windows rebuilds these on demand.",
        needs_admin: false,
        kind: RuleKind::Files {
            dirs: existing(
                local_app_data()
                    .map(|p| p.join("Microsoft").join("Windows").join("Explorer"))
                    .into_iter()
                    .collect(),
            ),
            pred: FilePred::new(Some("thumbcache_"), Some(".db")),
        },
    });

    if let Some(local) = local_app_data() {
        rules.push(Rule {
            id: "edge_cache",
            name: "Microsoft Edge cache",
            description: "Per-profile Cache / Code Cache / GPUCache for Edge. Pages reload from the network.",
            needs_admin: false,
            kind: RuleKind::Dirs(existing(browser_cache_dirs(
                &local.join("Microsoft").join("Edge").join("User Data"),
            ))),
        });

        rules.push(Rule {
            id: "chrome_cache",
            name: "Google Chrome cache",
            description: "Per-profile Cache / Code Cache / GPUCache for Chrome. Pages reload from the network.",
            needs_admin: false,
            kind: RuleKind::Dirs(existing(browser_cache_dirs(
                &local.join("Google").join("Chrome").join("User Data"),
            ))),
        });

        rules.push(Rule {
            id: "firefox_cache",
            name: "Mozilla Firefox cache",
            description: r"cache2 folders under %LOCALAPPDATA%\Mozilla\Firefox\Profiles. Pages reload from the network.",
            needs_admin: false,
            kind: RuleKind::Dirs(existing(
                sub_dirs(&local.join("Mozilla").join("Firefox").join("Profiles"))
                    .into_iter()
                    .map(|p| p.join("cache2"))
                    .filter(|p| p.is_dir())
                    .collect(),
            )),
        });
    }

    // Per-drive recycle bins. These are normal directories on each volume.
    let mut recycle_dirs = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        let candidate = root.join("$Recycle.Bin");
        if candidate.is_dir() {
            recycle_dirs.push(candidate);
        }
    }
    rules.push(Rule {
        id: "recycle_bin",
        name: "Recycle Bin (all drives)",
        description: r"The contents of $Recycle.Bin on every volume. Not recoverable from the Recycle Bin afterwards, but restorable from WinPurge's quarantine.",
        needs_admin: false,
        kind: RuleKind::Dirs(existing(recycle_dirs)),
    });

    rules.push(Rule {
        id: "windows_old",
        name: "Windows.old",
        description: r"The previous Windows installation kept after an upgrade. Removes your ability to roll back to the prior version. Requires elevation.",
        needs_admin: true,
        kind: RuleKind::Dirs(existing(vec![system_drive().join("Windows.old")])),
    });

    rules
}

/// Paths that must never be deleted, regardless of what a rule resolves to.
/// This is a second line of defence behind the whitelist: even if a rule were
/// misconfigured, these prefixes make the engine refuse to act.
pub fn is_critical(path: &Path) -> bool {
    let text = path.to_string_lossy().to_uppercase();

    // Volume roots and the top-level system trees. Sub-directories below these
    // are governed by the whitelist, not by this function.
    const CRITICAL_ROOTS: [&str; 6] = [
        "C:\\",
        "C:\\WINDOWS",
        "C:\\PROGRAM FILES",
        "C:\\PROGRAM FILES (X86)",
        "C:\\USERS",
        "C:\\SYSTEM VOLUME INFORMATION",
    ];
    if CRITICAL_ROOTS.contains(&text.as_str()) {
        return true;
    }

    // Core system files that sit at a volume root.
    if let Some(name) = path.file_name() {
        if matches!(
            name.to_string_lossy().to_uppercase().as_str(),
            "PAGEFILE.SYS" | "HIBERFIL.SYS" | "SWAPFILE.SYS" | "BOOTMGR" | "BOOTSECT.BAK"
        ) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_pred_combines_prefix_and_suffix() {
        let pred = FilePred::new(Some("thumbcache_"), Some(".db"));
        assert!(pred.matches("thumbcache_32.db"));
        assert!(!pred.matches("thumbcache_"));
        assert!(!pred.matches("iconcache_32.db"));
        assert!(FilePred::any().matches("anything"));
    }

    #[test]
    fn critical_paths_are_recognised() {
        // Only the exact roots are blocked here; sub-directories are allowed
        // because the whitelist in `all_rules` is what bounds them.
        assert!(is_critical(Path::new(r"C:\Windows")));
        assert!(is_critical(Path::new(r"C:\Users")));
        assert!(is_critical(Path::new(r"C:\pagefile.sys")));
        assert!(!is_critical(Path::new(r"C:\Windows\Temp")));
        assert!(!is_critical(Path::new(r"C:\Users\me\AppData\Local\Temp")));
    }
}
