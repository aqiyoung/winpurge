use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rules::{is_critical, Rule, RuleKind};
use crate::scan;
use crate::util::sanitize;

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    from: String,
    to: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    id: String,
    created: String,
    entries: Vec<ManifestEntry>,
}

#[derive(Clone, Debug)]
pub struct PlannedItem {
    pub rule_id: String,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub items: Vec<PlannedItem>,
    pub total: u64,
}

#[derive(Debug, Default)]
pub struct ApplyOutcome {
    pub moved: u32,
    pub freed: u64,
    pub failed: Vec<(PathBuf, String)>,
    pub manifest_id: Option<String>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// Build the deletion plan for the selected rule ids.
///
/// Only paths produced by the whitelist in `rules` can appear here, and every
/// path is re-checked against `is_critical` before it is added.
pub fn build_plan(rules: &[Rule], selected: &[String]) -> Plan {
    let mut items = Vec::new();

    for rule in rules {
        if !selected.iter().any(|id| id.as_str() == rule.id) {
            continue;
        }
        let candidates: Vec<(PathBuf, bool)> = match &rule.kind {
            RuleKind::Dirs(dirs) => dirs.iter().map(|d| (d.clone(), true)).collect(),
            RuleKind::Files { dirs, pred } => dirs
                .iter()
                .flat_map(|dir| {
                    let Ok(entries) = fs::read_dir(dir) else {
                        return Vec::new();
                    };
                    entries
                        .flatten()
                        .filter(|entry| pred.matches(&entry.file_name().to_string_lossy()))
                        .map(|entry| (entry.path(), false))
                        .collect::<Vec<_>>()
                })
                .collect(),
        };

        for (path, force_dir) in candidates {
            if !path.exists() || is_critical(&path) {
                continue;
            }
            let is_dir = force_dir || path.is_dir();
            let size = if is_dir {
                scan::dir_size(&path).map(|(bytes, _)| bytes).unwrap_or(0)
            } else {
                fs::metadata(&path).map(|md| md.len()).unwrap_or(0)
            };
            if size == 0 && !is_dir {
                continue;
            }
            items.push(PlannedItem {
                rule_id: rule.id.to_string(),
                path,
                size,
                is_dir,
            });
        }
    }

    let total = items.iter().map(|item| item.size).sum();
    Plan { items, total }
}

/// Move every planned item into a timestamped quarantine directory.
pub fn apply(
    plan: &Plan,
    quarantine_root: &Path,
    mut on_progress: impl FnMut(usize, usize, String),
) -> ApplyOutcome {
    let mut outcome = ApplyOutcome::default();
    if plan.items.is_empty() {
        return outcome;
    }

    let id = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let batch_dir = quarantine_root.join(&id);
    if let Err(error) = fs::create_dir_all(&batch_dir) {
        outcome.failed.push((
            batch_dir.clone(),
            format!("cannot create quarantine: {error}"),
        ));
        return outcome;
    }

    let mut entries = Vec::new();
    let total = plan.items.len();

    for (index, item) in plan.items.iter().enumerate() {
        on_progress(index + 1, total, item.path.display().to_string());

        if is_critical(&item.path) {
            outcome
                .skipped
                .push((item.path.clone(), "refused: critical path".to_string()));
            continue;
        }
        if !item.path.exists() {
            outcome
                .skipped
                .push((item.path.clone(), "already gone".to_string()));
            continue;
        }

        let key = format!(
            "{}-{}",
            item.rule_id,
            sanitize(&item.path.to_string_lossy())
        );
        let destination = unique_destination(&batch_dir, &key);

        match move_path(&item.path, &destination) {
            Ok(()) => {
                entries.push(ManifestEntry {
                    from: item.path.display().to_string(),
                    to: destination.display().to_string(),
                });
                outcome.moved += 1;
                outcome.freed += item.size;
            }
            Err(error) => outcome.failed.push((item.path.clone(), error.to_string())),
        }
    }

    let manifest = Manifest {
        id: id.clone(),
        created: chrono::Local::now().to_rfc3339(),
        entries,
    };
    let manifest_path = batch_dir.join("manifest.json");
    match serde_json::to_string_pretty(&manifest) {
        Ok(json) => {
            let _ = fs::write(&manifest_path, json);
        }
        Err(error) => outcome
            .failed
            .push((manifest_path, format!("cannot serialise manifest: {error}"))),
    }

    outcome.manifest_id = Some(id);
    outcome
}

/// Restore everything a batch moved, putting files back where they came from.
pub fn rollback(quarantine_root: &Path, id: &str) -> (u32, Vec<(PathBuf, String)>) {
    let batch_dir = quarantine_root.join(id);
    let manifest_path = batch_dir.join("manifest.json");
    let text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => return (0, vec![(manifest_path, error.to_string())]),
    };
    let manifest: Manifest = match serde_json::from_str(&text) {
        Ok(manifest) => manifest,
        Err(error) => return (0, vec![(manifest_path, error.to_string())]),
    };

    let mut restored = 0u32;
    let mut failed = Vec::new();

    // Restore in reverse order so nested paths come back outermost-first.
    for entry in manifest.entries.iter().rev() {
        let from = PathBuf::from(&entry.from);
        let to = PathBuf::from(&entry.to);
        if !to.exists() {
            continue;
        }
        match move_path(&to, &from) {
            Ok(()) => restored += 1,
            Err(error) => failed.push((to, error.to_string())),
        }
    }

    let _ = fs::remove_file(&manifest_path);
    let _ = fs::remove_dir(&batch_dir);

    (restored, failed)
}

/// Quarantine batches, newest first.
pub fn list_batches(quarantine_root: &Path) -> Vec<(String, u64)> {
    let mut batches = Vec::new();
    let Ok(entries) = fs::read_dir(quarantine_root) else {
        return batches;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let size = scan::dir_size(&path).map(|(bytes, _)| bytes).unwrap_or(0);
        batches.push((name, size));
    }
    batches.sort_by(|a, b| b.0.cmp(&a.0));
    batches
}

fn unique_destination(batch_dir: &Path, key: &str) -> PathBuf {
    let mut candidate = batch_dir.join(key);
    let mut counter = 1;
    while candidate.exists() {
        candidate = batch_dir.join(format!("{key}-{counter}"));
        counter += 1;
    }
    candidate
}

/// Move a file or directory tree, falling back to copy+delete when the source
/// and destination are on different volumes.
fn move_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            let is_dir = source.is_dir();
            copy_recursive(source, destination).map_err(|copy_error| {
                std::io::Error::new(
                    copy_error.kind(),
                    format!("rename failed ({rename_error}); copy failed ({copy_error})"),
                )
            })?;
            if is_dir {
                fs::remove_dir_all(source)
            } else {
                fs::remove_file(source)
            }
        }
    }
}

fn copy_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, destination)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::FilePred;
    use std::fs::File;
    use std::io::Write;

    fn fixture(tag: &str) -> PathBuf {
        // Each test gets its own directory: `cargo test` runs tests in
        // parallel, and a shared path would let one test delete another's
        // fixture mid-run.
        let base =
            std::env::temp_dir().join(format!("winpurge-engine-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("temp")).unwrap();
        fs::create_dir_all(base.join("thumbdir")).unwrap();
        let mut file = File::create(base.join("temp").join("junk.tmp")).unwrap();
        file.write_all(&[b'j'; 4096]).unwrap();
        let mut file = File::create(base.join("thumbdir").join("thumbcache_96.db")).unwrap();
        file.write_all(&[b't'; 2048]).unwrap();
        File::create(base.join("thumbdir").join("keep.txt")).unwrap();
        base
    }

    #[test]
    fn plan_only_contains_whitelisted_targets() {
        let base = fixture("plan");
        let rules = vec![
            Rule {
                id: "temp",
                name: "temp",
                description: "",
                needs_admin: false,
                kind: RuleKind::Dirs(vec![base.join("temp")]),
            },
            Rule {
                id: "thumbs",
                name: "thumbs",
                description: "",
                needs_admin: false,
                kind: RuleKind::Files {
                    dirs: vec![base.join("thumbdir")],
                    pred: FilePred::new(Some("thumbcache_"), Some(".db")),
                },
            },
        ];

        let plan = build_plan(&rules, &["thumbs".to_string()]);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(
            plan.items[0].path,
            base.join("thumbdir").join("thumbcache_96.db")
        );
        assert!(!plan.items.iter().any(|i| i.path.ends_with("keep.txt")));
        assert!(!plan.items.iter().any(|i| i.path.ends_with("temp")));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_then_rollback_round_trips() {
        let base = fixture("roundtrip");
        let quarantine = base.join("quarantine");
        let rules = vec![Rule {
            id: "temp",
            name: "temp",
            description: "",
            needs_admin: false,
            kind: RuleKind::Dirs(vec![base.join("temp")]),
        }];

        let plan = build_plan(&rules, &["temp".to_string()]);
        assert_eq!(plan.total, 4096);

        let outcome = apply(&plan, &quarantine, |_, _, _| {});
        assert_eq!(
            outcome.moved, 1,
            "skipped={:?} failed={:?}",
            outcome.skipped, outcome.failed
        );
        assert_eq!(outcome.freed, 4096);
        assert!(outcome.failed.is_empty());
        assert!(!base.join("temp").exists(), "source moved away");

        let id = outcome.manifest_id.expect("manifest id");
        let (restored, failed) = rollback(&quarantine, &id);
        assert!(failed.is_empty());
        assert_eq!(restored, 1);
        assert!(base.join("temp").join("junk.tmp").exists(), "file restored");
        assert_eq!(
            fs::read(base.join("temp").join("junk.tmp")).unwrap().len(),
            4096
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn critical_paths_are_never_planned() {
        let rules = vec![Rule {
            id: "bad",
            name: "bad",
            description: "",
            needs_admin: true,
            kind: RuleKind::Dirs(vec![PathBuf::from(r"C:\Windows")]),
        }];
        let plan = build_plan(&rules, &["bad".to_string()]);
        assert!(plan.items.is_empty(), "C:\\Windows must never be a target");
    }
}
