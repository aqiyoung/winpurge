use std::path::PathBuf;

/// What a tree node actually represents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A real directory on disk.
    Directory,
    /// Files that sit directly inside the parent directory (not in a subdirectory).
    /// The payload is the number of such files.
    LooseFiles(u32),
    /// A directory we could not read (permissions, locked, ...).
    Inaccessible,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub kind: NodeKind,
    pub children: Vec<Node>,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Directory)
    }
}

#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub letter: char,
    pub path: PathBuf,
    pub total: u64,
    pub free: u64,
}
