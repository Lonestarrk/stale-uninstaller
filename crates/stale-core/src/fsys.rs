//! Testable read-only file-system view: [`RealFs`] for production, [`FakeFs`] for tests.

use crate::pathutil::norm;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

pub trait FsReader: Send + Sync {
    /// Direct children only; empty if unreadable.
    fn list_dir(&self, dir: &Path) -> Vec<DirEntryInfo>;
    fn exists(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    fn dir_size(&self, p: &Path) -> Option<u64>;
    /// For task XML.
    fn read_text(&self, p: &Path) -> Option<String>;
    /// Every profile dir except Public, Default, Default User and All Users.
    fn profile_dirs(&self) -> Vec<PathBuf>;
    fn env(&self, var: &str) -> Option<String>;
}

const SKIPPED_PROFILES: [&str; 4] = ["public", "default", "default user", "all users"];

pub struct RealFs;

impl FsReader for RealFs {
    fn list_dir(&self, dir: &Path) -> Vec<DirEntryInfo> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        rd.flatten()
            .filter_map(|e| {
                let md = e.metadata().ok()?;
                let is_dir = md.is_dir();
                Some(DirEntryInfo { path: e.path(), is_dir, size: if is_dir { 0 } else { md.len() } })
            })
            .collect()
    }

    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }

    fn dir_size(&self, p: &Path) -> Option<u64> {
        crate::size::dir_size(p)
    }

    fn read_text(&self, p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }

    fn profile_dirs(&self) -> Vec<PathBuf> {
        self.list_dir(&crate::paths::user_profiles_root())
            .into_iter()
            .filter(|e| e.is_dir)
            .filter(|e| {
                let name = e.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                !SKIPPED_PROFILES.contains(&name.as_str())
            })
            .map(|e| e.path)
            .collect()
    }

    fn env(&self, var: &str) -> Option<String> {
        std::env::var(var).ok()
    }
}

#[derive(Clone, Debug)]
struct Node {
    path: PathBuf,
    is_dir: bool,
    size: Option<u64>,
    text: Option<String>,
}

/// In-memory file system for tests; paths compare case-insensitively via [`norm`].
#[derive(Clone, Debug, Default)]
pub struct FakeFs {
    nodes: BTreeMap<String, Node>,
    profiles: Vec<PathBuf>,
    env: BTreeMap<String, String>,
}

fn parent_of(p: &Path) -> Option<PathBuf> {
    let parent = p.parent()?;
    (!norm(parent).is_empty() && norm(parent) != norm(p)).then(|| parent.to_path_buf())
}

impl FakeFs {
    pub fn new() -> FakeFs {
        FakeFs::default()
    }

    fn add_parents(&mut self, path: &Path) {
        let mut cur = parent_of(path);
        while let Some(p) = cur {
            // A bare drive ("C:") is not worth a node.
            if parent_of(&p).is_none() {
                break;
            }
            self.nodes
                .entry(norm(&p))
                .or_insert_with(|| Node { path: p.clone(), is_dir: true, size: None, text: None });
            cur = parent_of(&p);
        }
    }

    pub fn dir(mut self, path: &str, size: u64) -> Self {
        let p = PathBuf::from(path);
        self.add_parents(&p);
        self.nodes.insert(norm(&p), Node { path: p, is_dir: true, size: Some(size), text: None });
        self
    }

    pub fn file(mut self, path: &str, size: u64) -> Self {
        let p = PathBuf::from(path);
        self.add_parents(&p);
        self.nodes.insert(norm(&p), Node { path: p, is_dir: false, size: Some(size), text: None });
        self
    }

    pub fn text(mut self, path: &str, content: &str) -> Self {
        let p = PathBuf::from(path);
        self.add_parents(&p);
        let size = content.len() as u64;
        self.nodes.insert(norm(&p), Node { path: p, is_dir: false, size: Some(size), text: Some(content.to_string()) });
        self
    }

    pub fn env(mut self, var: &str, value: &str) -> Self {
        self.env.insert(var.to_uppercase(), value.to_string());
        self
    }

    pub fn profile(mut self, path: &str) -> Self {
        self.profiles.push(PathBuf::from(path));
        self
    }

    fn children(&self, dir: &Path) -> Vec<&Node> {
        let key = norm(dir);
        self.nodes.values().filter(|n| parent_of(&n.path).is_some_and(|p| norm(&p) == key)).collect()
    }

    fn size_of(&self, n: &Node) -> u64 {
        if let Some(s) = n.size {
            return s;
        }
        self.children(&n.path).iter().map(|c| self.size_of(c)).sum()
    }
}

impl FsReader for FakeFs {
    fn list_dir(&self, dir: &Path) -> Vec<DirEntryInfo> {
        self.children(dir)
            .into_iter()
            .map(|n| DirEntryInfo { path: n.path.clone(), is_dir: n.is_dir, size: self.size_of(n) })
            .collect()
    }

    fn exists(&self, p: &Path) -> bool {
        self.nodes.contains_key(&norm(p))
    }

    fn is_dir(&self, p: &Path) -> bool {
        self.nodes.get(&norm(p)).is_some_and(|n| n.is_dir)
    }

    fn dir_size(&self, p: &Path) -> Option<u64> {
        self.nodes.get(&norm(p)).filter(|n| n.is_dir).map(|n| self.size_of(n))
    }

    fn read_text(&self, p: &Path) -> Option<String> {
        self.nodes.get(&norm(p)).and_then(|n| n.text.clone())
    }

    fn profile_dirs(&self) -> Vec<PathBuf> {
        self.profiles.clone()
    }

    fn env(&self, var: &str) -> Option<String> {
        self.env.get(&var.to_uppercase()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_lists_children_with_sizes_and_creates_parents() {
        let fs = FakeFs::new().dir(r"C:\A\B", 100).file(r"C:\A\f.txt", 7).file(r"C:\A\C\g.txt", 5);
        assert!(fs.is_dir(Path::new(r"c:/a")));
        let mut kids = fs.list_dir(Path::new(r"C:\A"));
        kids.sort_by_key(|k| norm(&k.path));
        assert_eq!(kids.len(), 3);
        assert_eq!((kids[0].is_dir, kids[0].size), (true, 100));
        assert_eq!((kids[1].is_dir, kids[1].size), (true, 5));
        assert_eq!((kids[2].is_dir, kids[2].size), (false, 7));
        assert_eq!(fs.dir_size(Path::new(r"C:\A\C")), Some(5));
        assert_eq!(fs.dir_size(Path::new(r"C:\A\f.txt")), None);
    }

    #[test]
    fn fake_text_env_profiles() {
        let fs = FakeFs::new().text(r"C:\x\t.xml", "<a/>").env("AppData", r"C:\U").profile(r"C:\Users\A");
        assert_eq!(fs.read_text(Path::new(r"C:\X\T.XML")).as_deref(), Some("<a/>"));
        assert_eq!(FsReader::env(&fs, "APPDATA").as_deref(), Some(r"C:\U"));
        assert_eq!(fs.profile_dirs(), vec![PathBuf::from(r"C:\Users\A")]);
        assert!(fs.exists(Path::new(r"C:\x")));
    }

    #[test]
    #[ignore]
    fn real_fs_lists_program_files() {
        let pf = RealFs.env("ProgramFiles").unwrap();
        assert!(!RealFs.list_dir(Path::new(&pf)).is_empty());
    }
}
