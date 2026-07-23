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

/// Explorer ordering: folders first, then files, each group in ascending
/// ASCII (byte) name order.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| (!a.is_dir, a.name.as_bytes()).cmp(&(!b.is_dir, b.name.as_bytes())));
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

    // ---- ownership-anchor policy ------------------------------------------
    //
    // The depth-2 folder `<project>/<username>` is an *ownership anchor*: its
    // second segment names the user who owns every memory beneath it. Renaming
    // that folder, moving it, or materialising one under a different username
    // reassigns ownership — which is exactly how a non-admin could inject
    // memory into (or tear down) another user's space. `access_for` alone does
    // not defend this: it grants a user ReadWrite on their own anchor, so a
    // plain write-permission check would happily let them rename it away. These
    // predicates add the missing structural rules and are the privacy deploy
    // gate — the integration suite exercises the same decisions over HTTP.

    /// True when `segs` points at an ownership anchor (`<project>/<username>`).
    pub fn is_ownership_anchor(segs: &[String]) -> bool {
        segs.len() == 2
    }

    /// Whether `username` may rename the item at `segs` to a sibling name.
    /// Non-admins can never rename an ownership anchor: the new name would hand
    /// the memories to a different username (or to nobody).
    pub fn can_rename(
        username: &str,
        is_admin: bool,
        grants: &[(String, String)],
        segs: &[String],
    ) -> bool {
        if is_admin {
            return true;
        }
        if Self::is_ownership_anchor(segs) {
            return false;
        }
        Self::access_for(username, is_admin, grants, segs) == Access::ReadWrite
    }

    /// Whether `username` may delete the item at `segs`. A non-admin may delete
    /// their own space, but never another user's ownership anchor — even when
    /// they hold a readwrite share on it.
    pub fn can_delete(
        username: &str,
        is_admin: bool,
        grants: &[(String, String)],
        segs: &[String],
    ) -> bool {
        if is_admin {
            return true;
        }
        if Self::is_ownership_anchor(segs) && segs[1] != username {
            return false;
        }
        Self::access_for(username, is_admin, grants, segs) == Access::ReadWrite
    }

    /// Whether a non-admin may move the *source* item at `segs`. Relocating an
    /// ownership anchor is admin-only — it carries a whole user's namespace.
    pub fn can_move_source(is_admin: bool, segs: &[String]) -> bool {
        is_admin || !Self::is_ownership_anchor(segs)
    }

    /// Whether `username` may have a moved/copied item *land* at `to`. The
    /// destination may not be a bare project root, nor an ownership anchor for
    /// any username other than your own — either would inject a foreign space.
    pub fn can_place_at(username: &str, is_admin: bool, to: &[String]) -> bool {
        if is_admin {
            return true;
        }
        if to.len() < 2 {
            return false;
        }
        if Self::is_ownership_anchor(to) && to[1] != username {
            return false;
        }
        true
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
        sort_entries(&mut out);
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
    fn entries_sort_folders_first_then_ascii() {
        let e = |name: &str, is_dir: bool| Entry {
            name: name.into(),
            path: name.into(),
            is_dir,
            size: 0,
            modified_at: None,
            access: "readwrite".into(),
            shared: false,
        };
        let mut v = vec![e("b.md", false), e("a", true), e("Z.md", false), e("A", true), e("1", true)];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        // Folders first, then files; each group in ASCII order (digits <
        // uppercase < lowercase).
        assert_eq!(names, ["1", "A", "a", "Z.md", "b.md"]);
    }

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

    // ---- ownership-anchor policy (privacy-critical) -----------------------

    fn segs(p: &str) -> Vec<String> {
        clean_path(p).unwrap()
    }

    #[test]
    fn anchor_is_depth_two() {
        assert!(!Store::is_ownership_anchor(&segs("proj")));
        assert!(Store::is_ownership_anchor(&segs("proj/alice")));
        assert!(!Store::is_ownership_anchor(&segs("proj/alice/notes.md")));
    }

    #[test]
    fn non_admin_cannot_rename_any_anchor() {
        // Own anchor: renaming it would push alice's memories into another
        // username's namespace — the core injection vector. Denied.
        assert!(!Store::can_rename("alice", false, &[], &segs("proj/alice")));
        // Someone else's anchor, no grant: denied.
        assert!(!Store::can_rename("alice", false, &[], &segs("proj/bob")));
        // Even a readwrite share on bob's space must not let alice rename the
        // anchor (that would break bob's own-space access entirely).
        let rw = vec![("proj/bob".to_string(), "readwrite".to_string())];
        assert!(!Store::can_rename("alice", false, &rw, &segs("proj/bob")));
        // Admins may reorganize anchors.
        assert!(Store::can_rename("admin", true, &[], &segs("proj/alice")));
    }

    #[test]
    fn non_admin_can_rename_within_own_space() {
        // Files and subfolders inside your own space rename freely.
        assert!(Store::can_rename("alice", false, &[], &segs("proj/alice/notes.md")));
        assert!(Store::can_rename("alice", false, &[], &segs("proj/alice/sub")));
        // But not inside someone else's space without write access.
        assert!(!Store::can_rename("alice", false, &[], &segs("proj/bob/notes.md")));
        let rw = vec![("proj/bob".to_string(), "readwrite".to_string())];
        assert!(Store::can_rename("alice", false, &rw, &segs("proj/bob/notes.md")));
    }

    #[test]
    fn delete_protects_other_users_anchor() {
        // Deleting your own space is allowed.
        assert!(Store::can_delete("alice", false, &[], &segs("proj/alice")));
        // Deleting another user's anchor is denied, even with a readwrite share.
        assert!(!Store::can_delete("alice", false, &[], &segs("proj/bob")));
        let rw = vec![("proj/bob".to_string(), "readwrite".to_string())];
        assert!(!Store::can_delete("alice", false, &rw, &segs("proj/bob")));
        // A readwrite share still allows deleting files *inside* bob's space.
        assert!(Store::can_delete("alice", false, &rw, &segs("proj/bob/notes.md")));
        // Admin can delete anything.
        assert!(Store::can_delete("admin", true, &[], &segs("proj/bob")));
    }

    #[test]
    fn move_source_anchor_is_admin_only() {
        assert!(!Store::can_move_source(false, &segs("proj/alice")));
        assert!(Store::can_move_source(false, &segs("proj/alice/sub")));
        assert!(Store::can_move_source(true, &segs("proj/alice")));
    }

    #[test]
    fn place_cannot_forge_foreign_anchor() {
        // Landing at a bare project root is not allowed for non-admins.
        assert!(!Store::can_place_at("alice", false, &segs("proj")));
        // Landing as another username's anchor injects a foreign space: denied.
        assert!(!Store::can_place_at("alice", false, &segs("proj/bob")));
        // Your own anchor, or anything deeper, is fine.
        assert!(Store::can_place_at("alice", false, &segs("proj/alice")));
        assert!(Store::can_place_at("alice", false, &segs("proj/alice/sub")));
        // Admins may create anchors anywhere.
        assert!(Store::can_place_at("admin", true, &segs("proj/bob")));
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
