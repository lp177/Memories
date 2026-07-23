//! Markdown file store + permission model.
//!
//! Layout on disk: `{data_dir}/memories/<project>/<user>/<path...>`.
//! The first path segment is the *project*, the second the *owner space*
//! (a username). A user always has full access to their own space in any
//! project; access to anything else comes from shares (user or group) or
//! from being an admin.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::Db;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Access {
    None,
    Read,
    ReadWrite,
}

#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: Option<String>,
    /// Effective permission of the requesting user on this entry.
    pub access: String,
    pub shared: bool,
}

pub struct Store {
    root: PathBuf,
}

/// Validate one path segment: no traversal, no separators, no hidden files.
fn valid_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg.len() <= 255
        && seg != "."
        && seg != ".."
        && !seg.starts_with('.')
        && !seg.contains(['/', '\\', '\0'])
}

/// Normalize and validate a relative memory path ("a/b/c"). Empty = root.
pub fn clean_path(path: &str) -> Option<Vec<String>> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let segs: Vec<String> = trimmed.split('/').map(str::to_string).collect();
    if segs.iter().all(|s| valid_segment(s)) {
        Some(segs)
    } else {
        None
    }
}

impl Store {
    pub fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("memories");
        let _ = fs::create_dir_all(&root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn abs(&self, segs: &[String]) -> PathBuf {
        let mut p = self.root.clone();
        for s in segs {
            p.push(s);
        }
        p
    }

    // ---- permissions ------------------------------------------------------

    /// Effective access of `user` on `segs`, given precomputed grants
    /// (path prefix -> permission) and admin flag.
    pub fn access_for(
        username: &str,
        is_admin: bool,
        grants: &[(String, String)],
        segs: &[String],
    ) -> Access {
        if is_admin {
            return Access::ReadWrite;
        }
        // Own space: memories/<project>/<username>/...
        if segs.len() >= 2 && segs[1] == username {
            return Access::ReadWrite;
        }
        // A user may always list the root and any project container that they
        // can reach *into* — visibility filtering happens at list time.
        let joined = segs.join("/");
        let mut best = Access::None;
        for (path, perm) in grants {
            let hit = joined == *path || joined.starts_with(&format!("{path}/"));
            if hit {
                let level = if perm == "readwrite" { Access::ReadWrite } else { Access::Read };
                if level == Access::ReadWrite {
                    return Access::ReadWrite;
                }
                best = Access::Read;
            }
        }
        best
    }

    /// Whether `segs` (or something under it) is visible to the user: own
    /// space, admin, a project the user has a space in, or a grant whose
    /// path is under OR above this path.
    pub fn visible(
        &self,
        username: &str,
        is_admin: bool,
        grants: &[(String, String)],
        segs: &[String],
    ) -> bool {
        if is_admin {
            return true;
        }
        if segs.is_empty() {
            return true;
        }
        if segs.len() >= 2 && segs[1] == username {
            return true;
        }
        // Project container: visible when the user owns a space inside it.
        if segs.len() == 1 && self.user_has_space(&segs[0], username) {
            return true;
        }
        let joined = segs.join("/");
        grants.iter().any(|(path, _)| {
            joined == *path
                || joined.starts_with(&format!("{path}/"))
                || path.starts_with(&format!("{joined}/"))
        })
    }

    // ---- listing ----------------------------------------------------------

    pub fn list(
        &self,
        segs: &[String],
        username: &str,
        is_admin: bool,
        grants: &[(String, String)],
        db: &Db,
    ) -> std::io::Result<Vec<Entry>> {
        let dir = self.abs(segs);
        let mut out = Vec::new();
        if !dir.exists() {
            return Ok(out);
        }
        for item in fs::read_dir(&dir)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().to_string();
            if !valid_segment(&name) {
                continue;
            }
            let mut child = segs.to_vec();
            child.push(name.clone());
            if !self.visible(username, is_admin, grants, &child) {
                continue;
            }
            let meta = item.metadata()?;
            let access = Self::access_for(username, is_admin, grants, &child);
            // Skip entries the user cannot even read (visible ancestors of a
            // grant are kept so the user can navigate down to the share).
            let joined = child.join("/");
            let reachable = access != Access::None
                || grants.iter().any(|(p, _)| p.starts_with(&format!("{joined}/")) || *p == joined)
                || (child.len() == 1 && self.user_has_space(&child[0], username));
            if !reachable {
                continue;
            }
            let shared = !db.shares_for_path(&joined).is_empty();
            out.push(Entry {
                name,
                path: joined,
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified_at: meta
                    .modified()
                    .ok()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                access: match access {
                    Access::ReadWrite => "readwrite",
                    Access::Read => "read",
                    Access::None => "traverse",
                }
                .to_string(),
                shared,
            });
        }
        out.sort_by(|a, b| (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase())));
        Ok(out)
    }

    fn user_has_space(&self, project: &str, username: &str) -> bool {
        self.root.join(project).join(username).is_dir()
    }

    // ---- content ----------------------------------------------------------

    pub fn read_file(&self, segs: &[String]) -> std::io::Result<String> {
        fs::read_to_string(self.abs(segs))
    }

    pub fn write_file(&self, segs: &[String], content: &str) -> std::io::Result<bool> {
        let path = self.abs(segs);
        let created = !path.exists();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(created)
    }

    pub fn mkdir(&self, segs: &[String]) -> std::io::Result<()> {
        fs::create_dir_all(self.abs(segs))
    }

    pub fn delete(&self, segs: &[String]) -> std::io::Result<()> {
        let path = self.abs(segs);
        if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    }

    pub fn exists(&self, segs: &[String]) -> bool {
        self.abs(segs).exists()
    }

    pub fn is_dir(&self, segs: &[String]) -> bool {
        self.abs(segs).is_dir()
    }

    pub fn rename(&self, from: &[String], to: &[String]) -> std::io::Result<()> {
        let dst = self.abs(to);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(self.abs(from), dst)
    }

    pub fn copy(&self, from: &[String], to: &[String]) -> std::io::Result<()> {
        let src = self.abs(from);
        let dst = self.abs(to);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_recursive(&src, &dst)
    }
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for item in fs::read_dir(src)? {
            let item = item?;
            copy_recursive(&item.path(), &dst.join(item.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_rules() {
        assert_eq!(clean_path(""), Some(vec![]));
        assert_eq!(clean_path("/"), Some(vec![]));
        assert_eq!(
            clean_path("proj/alice/notes.md"),
            Some(vec!["proj".into(), "alice".into(), "notes.md".into()])
        );
        assert!(clean_path("../etc/passwd").is_none());
        assert!(clean_path("a/../b").is_none());
        assert!(clean_path("a/.hidden").is_none());
        assert!(clean_path("a//b").is_none());
        assert!(clean_path("a/\\b").is_none());
    }

    #[test]
    fn own_space_access() {
        let grants = vec![];
        let segs = clean_path("proj/alice/notes.md").unwrap();
        assert_eq!(Store::access_for("alice", false, &grants, &segs), Access::ReadWrite);
        assert_eq!(Store::access_for("bob", false, &grants, &segs), Access::None);
        assert_eq!(Store::access_for("bob", true, &grants, &segs), Access::ReadWrite);
    }

    #[test]
    fn shared_access_prefix() {
        let grants = vec![("proj/alice".to_string(), "read".to_string())];
        let file = clean_path("proj/alice/notes.md").unwrap();
        assert_eq!(Store::access_for("bob", false, &grants, &file), Access::Read);
        let other = clean_path("proj2/alice/notes.md").unwrap();
        assert_eq!(Store::access_for("bob", false, &grants, &other), Access::None);
        let rw = vec![("proj/alice/notes.md".to_string(), "readwrite".to_string())];
        assert_eq!(Store::access_for("bob", false, &rw, &file), Access::ReadWrite);
    }

    #[test]
    fn fs_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let segs = clean_path("proj/alice/notes.md").unwrap();
        assert!(store.write_file(&segs, "hello").unwrap());
        assert_eq!(store.read_file(&segs).unwrap(), "hello");
        assert!(!store.write_file(&segs, "hello2").unwrap()); // update, not create
        let to = clean_path("proj/alice/renamed.md").unwrap();
        store.rename(&segs, &to).unwrap();
        assert!(!store.exists(&segs));
        assert_eq!(store.read_file(&to).unwrap(), "hello2");
        let copy_to = clean_path("proj2/alice/copy.md").unwrap();
        store.copy(&to, &copy_to).unwrap();
        assert_eq!(store.read_file(&copy_to).unwrap(), "hello2");
        store.delete(&clean_path("proj").unwrap()).unwrap();
        assert!(!store.exists(&to));
        assert!(store.exists(&copy_to));
    }
}
