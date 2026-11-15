use std::path::{Path, PathBuf};

use crate::util::env_dir;

/// A filename filter, expressed without a glob dependency.
#[derive(Clone, Debug)]
pub struct FilePred {
    pub prefix: Option<&'static str>,
    /// A name matches when it ends with any of these (case-sensitive on Windows).
    pub suffixes: &'static [&'static str],
}

impl FilePred {
    pub const fn any() -> Self {
        Self {
            prefix: None,
            suffixes: &[],
        }
    }

    pub const fn new(prefix: Option<&'static str>, suffixes: &'static [&'static str]) -> Self {
        Self { prefix, suffixes }
    }

    pub fn matches(&self, name: &str) -> bool {
        if let Some(prefix) = self.prefix {
            if !name.starts_with(prefix) {
                return false;
            }
        }
        if !self.suffixes.is_empty() && !self.suffixes.iter().any(|s| name.ends_with(s)) {
            return false;
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

/// Immediate sub-directories of `roots` whose name is in `names`
/// (case-insensitive). Used to pick log/crash containers out of a vendor tree
/// without touching the sibling folders that hold personal data.
fn named_sub_dirs(roots: &[PathBuf], names: &[&str]) -> Vec<PathBuf> {
    let wanted: Vec<String> = names.iter().map(|n| n.to_uppercase()).collect();
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_uppercase();
            if wanted.contains(&name) {
                out.push(path);
            }
        }
    }
    out
}

fn app_data() -> Option<PathBuf> {
    env_dir("APPDATA")
}

/// Per-user Sogou trees. `%ProgramData%\SogouInput` is handled separately
/// because it needs elevation.
fn sogou_user_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = app_data() {
        roots.push(p.join("SogouInput"));
        roots.push(p.join("SogouPY"));
    }
    if let Some(p) = local_app_data() {
        roots.push(p.join("SogouInput"));
        roots.push(p.join("SogouPY"));
    }
    roots
}

/// Directories whose names unambiguously mean "logs or crash dumps".
const LOG_DIR_NAMES: &[&str] = &[
    "SogouLog",
    "Log",
    "Logs",
    "Crash",
    "CrashReport",
    "CrashReports",
    "CrashDumps",
    "Dumps",
];

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
            pred: FilePred::new(Some("thumbcache_"), &[".db"]),
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

    // --- Browser error logs / crash dumps ---------------------------------
    if let Some(local) = local_app_data() {
        let chrome = local.join("Google").join("Chrome").join("User Data");
        let edge = local.join("Microsoft").join("Edge").join("User Data");
        let firefox_profiles = local.join("Mozilla").join("Firefox").join("Profiles");

        let mut crash_dirs: Vec<PathBuf> = vec![
            chrome.join("Crashpad"),
            edge.join("Crashpad"),
            local.join("Mozilla").join("Firefox").join("Crash Reports"),
        ];
        crash_dirs.extend(
            sub_dirs(&firefox_profiles)
                .into_iter()
                .map(|p| p.join("minidumps")),
        );

        rules.push(Rule {
            id: "browser_crash",
            name: "Browser crash reports",
            description: r"Crashpad reports for Chrome/Edge plus Firefox Crash Reports and minidumps. Browsers recreate these containers.",
            needs_admin: false,
            kind: RuleKind::Dirs(existing(crash_dirs)),
        });
    }

    // --- Sogou input method ----------------------------------------------
    // Sogou keeps lexicons, skins and personal settings in the same trees as
    // its caches. Only log/crash containers are targeted; the user's lexicon
    // is deliberately left alone.
    let sogou_user = sogou_user_roots();
    rules.push(Rule {
        id: "sogou_log_dirs",
        name: "Sogou IME log directories",
        description: r"Log and crash directories under %APPDATA% / %LOCALAPPDATA% SogouInput and SogouPY. Lexicons and skins are not touched.",
        needs_admin: false,
        kind: RuleKind::Dirs(existing(named_sub_dirs(&sogou_user, LOG_DIR_NAMES))),
    });

    rules.push(Rule {
        id: "sogou_log_files",
        name: "Sogou IME log and dump files",
        description: r"*.log and crash-dump files sitting directly inside the per-user Sogou trees.",
        needs_admin: false,
        kind: RuleKind::Files {
            dirs: existing(sogou_user.clone()),
            pred: FilePred::new(None, &[".log", ".dmp", ".hdmp", ".mdmp"]),
        },
    });

    rules.push(Rule {
        id: "sogou_machine_logs",
        name: "Sogou IME machine logs",
        description: r"Log and crash containers under %PROGRAMDATA%\SogouInput. Requires elevation.",
        needs_admin: true,
        kind: RuleKind::Dirs(existing(named_sub_dirs(
            &program_data()
                .map(|p| vec![p.join("SogouInput")])
                .unwrap_or_default(),
            LOG_DIR_NAMES,
        ))),
    });

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
        let pred = FilePred::new(Some("thumbcache_"), &[".db"]);
        assert!(pred.matches("thumbcache_32.db"));
        assert!(!pred.matches("thumbcache_"));
        assert!(!pred.matches("iconcache_32.db"));
        assert!(FilePred::any().matches("anything"));

        let dump = FilePred::new(None, &[".log", ".dmp"]);
        assert!(dump.matches("crash.log"));
        assert!(dump.matches("heap.dmp"));
        assert!(!dump.matches("UserPhrase.db"));
        assert!(!dump.matches("settings.ini"));
    }

    #[test]
    fn named_sub_dirs_picks_only_log_and_crash_containers() {
        let base = std::env::temp_dir().join(format!("winpurge-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("SogouLog")).unwrap();
        std::fs::create_dir_all(base.join("logs")).unwrap();
        std::fs::create_dir_all(base.join("UserData")).unwrap();

        let found = named_sub_dirs(&[base.clone()], LOG_DIR_NAMES);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"SogouLog".to_string()));
        assert!(
            names.contains(&"logs".to_string()),
            "match is case-insensitive"
        );
        assert!(
            !names.iter().any(|n| n == "UserData"),
            "personal-data folders must never be selected"
        );

        let _ = std::fs::remove_dir_all(&base);
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
