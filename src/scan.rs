use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{Node, NodeKind};
use crate::util::is_reparse_point;

/// Recursive size of a directory: (total bytes, file count).
///
/// Reparse points (junctions, symlinks, OneDrive placeholders) are never
/// traversed, so space is never counted twice and cycles cannot happen.
pub fn dir_size(path: &Path) -> Option<(u64, u32)> {
    if !path.is_dir() {
        return None;
    }
    let walker = walkdir::WalkDir::new(path)
        .follow_links(false)
        .filter_entry(|entry| {
            !(entry.file_type().is_dir() && is_reparse_point(&entry.path()))
        });

    let mut total = 0u64;
    let mut files = 0u32;
    for entry in walker.flatten() {
        if entry.file_type().is_dir() {
            continue;
        }
        if let Ok(md) = entry.metadata() {
            total += md.len();
            files = files.saturating_add(1);
        }
    }
    Some((total, files))
}

/// Build a size tree for the UI, bounded by `max_depth` directory levels.
///
/// Only directories become child nodes. Loose files directly inside a
/// directory are collapsed into a single synthetic node so a large volume does
/// not produce millions of GUI rows.
pub fn scan_tree(
    root: &Path,
    max_depth: usize,
    mut on_progress: impl FnMut(u32),
) -> Node {
    let mut counted = 0u32;
    build_dir(root, 0, max_depth, &mut counted, &mut on_progress)
}

fn build_dir(
    path: &Path,
    depth: usize,
    max_depth: usize,
    counted: &mut u32,
    on_progress: &mut impl FnMut(u32),
) -> Node {
    let name = display_name(path);

    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            // Unreadable: still give a size so the parent's total stays honest.
            let size = dir_size(path).map(|(bytes, _)| bytes).unwrap_or(0);
            return Node {
                name,
                path: path.to_path_buf(),
                size,
                kind: NodeKind::Inaccessible,
                children: Vec::new(),
            };
        }
    };

    // At the depth limit we stop descending but still report the full subtree size.
    if depth >= max_depth {
        drop(entries);
        let size = dir_size(path).map(|(bytes, _)| bytes).unwrap_or(0);
        return Node {
            name,
            path: path.to_path_buf(),
            size,
            kind: NodeKind::Directory,
            children: Vec::new(),
        };
    }

    let mut subdirs = Vec::new();
    let mut loose_size = 0u64;
    let mut loose_files = 0u32;

    for entry in entries.flatten() {
        counted = counted.saturating_add(1);
        if counted % 4096 == 0 {
            on_progress(counted);
        }

        let child_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            if is_reparse_point(&child_path) {
                continue;
            }
            subdirs.push(build_dir(&child_path, depth + 1, max_depth, counted, on_progress));
        } else if let Ok(md) = entry.metadata() {
            loose_size += md.len();
            loose_files = loose_files.saturating_add(1);
        }
    }

    if loose_files > 0 {
        subdirs.push(Node {
            name: format!("{loose_files} loose files"),
            path: path.join("__loose_files__"),
            size: loose_size,
            kind: NodeKind::LooseFiles(loose_files),
            children: Vec::new(),
        });
    }

    subdirs.sort_by(|a, b| b.size.cmp(&a.size));
    let size = subdirs.iter().map(|child| child.size).sum();

    Node {
        name,
        path: path.to_path_buf(),
        size,
        kind: NodeKind::Directory,
        children: subdirs,
    }
}

fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        // Drive roots ("C:\") have no file name; show the full path.
        None => path.to_string_lossy().into_owned(),
    }
}

/// Total size of every directory in a list, for the rule panel.
pub fn total_size(paths: &[PathBuf]) -> (u64, u32) {
    let mut bytes = 0u64;
    let mut files = 0u32;
    for path in paths {
        if path.is_dir() {
            if let Some((b, f)) = dir_size(path) {
                bytes += b;
                files = files.saturating_add(f);
            }
        } else if let Ok(md) = fs::metadata(path) {
            bytes += md.len();
            files = files.saturating_add(1);
        }
    }
    (bytes, files)
}

/// Total size of files inside `dirs` whose names match `pred`.
pub fn matching_files_size(dirs: &[PathBuf], pred: &crate::rules::FilePred) -> (u64, u32) {
    let mut bytes = 0u64;
    let mut files = 0u32;
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !pred.matches(&name) {
                continue;
            }
            if let Ok(md) = entry.metadata() {
                if md.is_file() {
                    bytes += md.len();
                    files = files.saturating_add(1);
                }
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn touch(path: &Path, bytes: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = File::create(path).unwrap();
        let chunk = vec![b'x'; 1024];
        let mut left = bytes;
        while left > 0 {
            let n = left.min(chunk.len() as u64) as usize;
            file.write_all(&chunk[..n]).unwrap();
            left -= n as u64;
        }
    }

    #[test]
    fn dir_size_sums_nested_files() {
        let base = std::env::temp_dir().join(format!("winpurge-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        touch(&base.join("a").join("f1.bin"), 2048);
        touch(&base.join("f2.bin"), 1024);
        let (bytes, files) = dir_size(&base).unwrap();
        assert_eq!(bytes, 3072);
        assert_eq!(files, 2);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_tree_collapses_loose_files() {
        let base = std::env::temp_dir().join(format!("winpurge-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        touch(&base.join("one.bin"), 100);
        touch(&base.join("two.bin"), 300);
        touch(&base.join("sub").join("three.bin"), 600);

        let tree = scan_tree(&base, 3, |_| {});
        assert_eq!(tree.size, 1000);
        assert_eq!(tree.children.len(), 2);

        let loose = tree
            .children
            .iter()
            .find(|c| matches!(c.kind, NodeKind::LooseFiles(_)))
            .expect("loose files node");
        assert_eq!(loose.size, 400);
        assert_eq!(loose.kind, NodeKind::LooseFiles(2));

        let _ = fs::remove_dir_all(&base);
    }
}
