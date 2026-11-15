use std::path::{Path, PathBuf};

/// `FILE_ATTRIBUTE_REPARSE_POINT` from the Windows SDK.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Junctions, symlinks and OneDrive placeholders all carry the reparse-point
/// attribute. Traversing them would double-count space and can loop forever,
/// so the scanner skips them.
pub fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            Ok(md) => md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

pub fn percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0) as f32
    }
}

pub fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Every drive letter that currently has a readable volume attached.
pub fn drives() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let path = PathBuf::from(format!("{}:\\", letter as char));
        if path.is_dir() {
            out.push(path);
        }
    }
    out
}

pub fn quarantine_root() -> PathBuf {
    let base = env_dir("PROGRAMDATA").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("WinPurge").join("Quarantine")
}

/// Filesystem-safe token used to build quarantine sub-directories.
pub fn sanitize(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.len() > 80 {
        out.truncate(80);
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_is_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn percent_handles_zero() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(1, 4), 25.0);
    }

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize(r"C:\Windows\Temp"), "C__Windows_Temp");
        assert_eq!(sanitize("edge-cache"), "edge-cache");
        assert_eq!(sanitize(""), "_");
    }
}
