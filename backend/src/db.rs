//! SQLite metadata layer: users, groups, shares, API tokens, password resets
//! and the persistent change-event log. Memory *content* lives on disk as
//! markdown files (see `store.rs`) — this DB only holds metadata.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub struct Db(Mutex<Connection>);

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn new_id() -> String {
    use rand::Rng;
    let bytes: [u8; 8] = rand::thread_rng().gen();
    hex::encode(bytes)
}

#[derive(Serialize, Clone, Debug)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub is_admin: bool,
    pub has_avatar: bool,
    pub created_at: String,
    #[serde(skip)]
    pub password_hash: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub members: Vec<String>, // user ids
}

#[derive(Serialize, Clone, Debug)]
pub struct Share {
    pub id: String,
    pub path: String,
    pub grantee_type: String, // "user" | "group"
    pub grantee_id: String,
    pub grantee_name: String,
    pub permission: String, // "read" | "readwrite"
    pub created_by: String,
    pub created_at: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    pub token_prefix: String,
    pub scope: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub last_used_ip: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Event {
    pub seq: i64,
    pub project: String,
    pub kind: String, // created | updated | deleted | moved | renamed
    pub path: String,
    pub new_path: Option<String>,
    pub actor: String,
    pub created_at: String,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS users (
                 id TEXT PRIMARY KEY,
                 username TEXT NOT NULL UNIQUE,
                 email TEXT NOT NULL DEFAULT '',
                 password_hash TEXT NOT NULL,
                 is_admin INTEGER NOT NULL DEFAULT 0,
                 avatar_ext TEXT,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS groups (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS group_members (
                 group_id TEXT NOT NULL,
                 user_id TEXT NOT NULL,
                 PRIMARY KEY (group_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS shares (
                 id TEXT PRIMARY KEY,
                 path TEXT NOT NULL,
                 grantee_type TEXT NOT NULL,
                 grantee_id TEXT NOT NULL,
                 permission TEXT NOT NULL,
                 created_by TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 UNIQUE (path, grantee_type, grantee_id)
             );
             CREATE TABLE IF NOT EXISTS api_tokens (
                 id TEXT PRIMARY KEY,
                 user_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 token_prefix TEXT NOT NULL,
                 token_hash TEXT NOT NULL UNIQUE,
                 scope TEXT NOT NULL DEFAULT 'read',
                 created_at TEXT NOT NULL,
                 last_used_at TEXT,
                 last_used_ip TEXT
             );
             CREATE TABLE IF NOT EXISTS password_resets (
                 token_hash TEXT PRIMARY KEY,
                 user_id TEXT NOT NULL,
                 expires_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 project TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 path TEXT NOT NULL,
                 new_path TEXT,
                 actor TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_project ON events (project, seq);",
        )?;
        Ok(Self(Mutex::new(conn)))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().expect("db mutex poisoned")
    }

    // ---- users ------------------------------------------------------------

    fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
        Ok(User {
            id: row.get("id")?,
            username: row.get("username")?,
            email: row.get("email")?,
            is_admin: row.get::<_, i64>("is_admin")? != 0,
            has_avatar: row.get::<_, Option<String>>("avatar_ext")?.is_some(),
            created_at: row.get("created_at")?,
            password_hash: row.get("password_hash")?,
        })
    }

    pub fn create_user(
        &self,
        username: &str,
        email: &str,
        password_hash: &str,
        is_admin: bool,
    ) -> rusqlite::Result<User> {
        let id = new_id();
        self.lock().execute(
            "INSERT INTO users (id, username, email, password_hash, is_admin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, username, email, password_hash, is_admin as i64, now()],
        )?;
        Ok(self.user_by_id(&id).unwrap())
    }

    pub fn user_by_id(&self, id: &str) -> Option<User> {
        self.lock()
            .query_row("SELECT * FROM users WHERE id = ?1", [id], Self::row_to_user)
            .optional()
            .ok()
            .flatten()
    }

    pub fn user_by_username(&self, username: &str) -> Option<User> {
        self.lock()
            .query_row(
                "SELECT * FROM users WHERE username = ?1",
                [username],
                Self::row_to_user,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn list_users(&self) -> Vec<User> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM users ORDER BY username ASC")
            .unwrap();
        let rows = stmt
            .query_map([], Self::row_to_user)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        rows
    }

    pub fn update_user(
        &self,
        id: &str,
        username: Option<&str>,
        email: Option<&str>,
        password_hash: Option<&str>,
        is_admin: Option<bool>,
    ) -> rusqlite::Result<()> {
        let conn = self.lock();
        if let Some(v) = username {
            conn.execute("UPDATE users SET username = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = email {
            conn.execute("UPDATE users SET email = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = password_hash {
            conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = is_admin {
            conn.execute("UPDATE users SET is_admin = ?1 WHERE id = ?2", params![v as i64, id])?;
        }
        Ok(())
    }

    pub fn set_avatar_ext(&self, id: &str, ext: Option<&str>) {
        let _ = self
            .lock()
            .execute("UPDATE users SET avatar_ext = ?1 WHERE id = ?2", params![ext, id]);
    }

    pub fn avatar_ext(&self, id: &str) -> Option<String> {
        self.lock()
            .query_row("SELECT avatar_ext FROM users WHERE id = ?1", [id], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()
            .ok()
            .flatten()
            .flatten()
    }

    pub fn delete_user(&self, id: &str) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM group_members WHERE user_id = ?1", [id]);
        let _ = conn.execute("DELETE FROM api_tokens WHERE user_id = ?1", [id]);
        let _ = conn.execute(
            "DELETE FROM shares WHERE grantee_type = 'user' AND grantee_id = ?1",
            [id],
        );
        let _ = conn.execute("DELETE FROM users WHERE id = ?1", [id]);
    }

    // ---- groups -----------------------------------------------------------

    pub fn create_group(&self, name: &str) -> rusqlite::Result<String> {
        let id = new_id();
        self.lock().execute(
            "INSERT INTO groups (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, now()],
        )?;
        Ok(id)
    }

    pub fn delete_group(&self, id: &str) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM group_members WHERE group_id = ?1", [id]);
        let _ = conn.execute(
            "DELETE FROM shares WHERE grantee_type = 'group' AND grantee_id = ?1",
            [id],
        );
        let _ = conn.execute("DELETE FROM groups WHERE id = ?1", [id]);
    }

    pub fn rename_group(&self, id: &str, name: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])?;
        Ok(())
    }

    pub fn set_group_members(&self, id: &str, user_ids: &[String]) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM group_members WHERE group_id = ?1", [id]);
        for uid in user_ids {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?1, ?2)",
                params![id, uid],
            );
        }
    }

    pub fn list_groups(&self) -> Vec<Group> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT id, name FROM groups ORDER BY name ASC")
            .unwrap();
        let groups: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        groups
            .into_iter()
            .map(|(id, name)| {
                let mut m = conn
                    .prepare("SELECT user_id FROM group_members WHERE group_id = ?1")
                    .unwrap();
                let members = m
                    .query_map([&id], |r| r.get::<_, String>(0))
                    .unwrap()
                    .filter_map(Result::ok)
                    .collect();
                Group { id, name, members }
            })
            .collect()
    }

    pub fn user_group_ids(&self, user_id: &str) -> Vec<String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT group_id FROM group_members WHERE user_id = ?1")
            .unwrap();
        stmt.query_map([user_id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    // ---- shares -----------------------------------------------------------

    pub fn upsert_share(
        &self,
        path: &str,
        grantee_type: &str,
        grantee_id: &str,
        permission: &str,
        created_by: &str,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO shares (id, path, grantee_type, grantee_id, permission, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (path, grantee_type, grantee_id)
             DO UPDATE SET permission = excluded.permission",
            params![new_id(), path, grantee_type, grantee_id, permission, created_by, now()],
        )?;
        Ok(())
    }

    pub fn delete_share(&self, id: &str) {
        let _ = self.lock().execute("DELETE FROM shares WHERE id = ?1", [id]);
    }

    pub fn share_by_id(&self, id: &str) -> Option<Share> {
        self.lock()
            .query_row("SELECT * FROM shares WHERE id = ?1", [id], |row| {
                Ok(Share {
                    id: row.get("id")?,
                    path: row.get("path")?,
                    grantee_type: row.get("grantee_type")?,
                    grantee_id: row.get("grantee_id")?,
                    grantee_name: String::new(),
                    permission: row.get("permission")?,
                    created_by: row.get("created_by")?,
                    created_at: row.get("created_at")?,
                })
            })
            .optional()
            .ok()
            .flatten()
    }

    pub fn shares_for_path(&self, path: &str) -> Vec<Share> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM shares WHERE path = ?1 ORDER BY created_at ASC")
            .unwrap();
        let mut out: Vec<Share> = stmt
            .query_map([path], |row| {
                Ok(Share {
                    id: row.get("id")?,
                    path: row.get("path")?,
                    grantee_type: row.get("grantee_type")?,
                    grantee_id: row.get("grantee_id")?,
                    grantee_name: String::new(),
                    permission: row.get("permission")?,
                    created_by: row.get("created_by")?,
                    created_at: row.get("created_at")?,
                })
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for s in &mut out {
            s.grantee_name = if s.grantee_type == "user" {
                conn.query_row("SELECT username FROM users WHERE id = ?1", [&s.grantee_id], |r| {
                    r.get(0)
                })
                .unwrap_or_else(|_| "?".into())
            } else {
                conn.query_row("SELECT name FROM groups WHERE id = ?1", [&s.grantee_id], |r| {
                    r.get(0)
                })
                .unwrap_or_else(|_| "?".into())
            };
        }
        out
    }

    /// All (path, permission) pairs granted to this user directly or via groups.
    pub fn grants_for_user(&self, user_id: &str, group_ids: &[String]) -> Vec<(String, String)> {
        let conn = self.lock();
        let mut out: Vec<(String, String)> = Vec::new();
        let mut stmt = conn
            .prepare("SELECT path, permission FROM shares WHERE grantee_type = 'user' AND grantee_id = ?1")
            .unwrap();
        out.extend(
            stmt.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .filter_map(Result::ok),
        );
        for gid in group_ids {
            let mut stmt = conn
                .prepare("SELECT path, permission FROM shares WHERE grantee_type = 'group' AND grantee_id = ?1")
                .unwrap();
            out.extend(
                stmt.query_map([gid], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .filter_map(Result::ok),
            );
        }
        out
    }

    /// Rewrite share paths after a move/rename: `old` prefix becomes `new`.
    pub fn rewrite_share_paths(&self, old: &str, new: &str) {
        let conn = self.lock();
        let _ = conn.execute(
            "UPDATE OR REPLACE shares SET path = ?2 WHERE path = ?1",
            params![old, new],
        );
        let like = format!("{}/%", old);
        let mut stmt = conn
            .prepare("SELECT id, path FROM shares WHERE path LIKE ?1")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([&like], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for (id, path) in rows {
            let rewritten = format!("{}{}", new, &path[old.len()..]);
            let _ = conn.execute(
                "UPDATE OR REPLACE shares SET path = ?1 WHERE id = ?2",
                params![rewritten, id],
            );
        }
    }

    /// Drop shares on a deleted path (and everything under it).
    pub fn delete_shares_under(&self, path: &str) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM shares WHERE path = ?1", [path]);
        let _ = conn.execute(
            "DELETE FROM shares WHERE path LIKE ?1",
            [format!("{}/%", path)],
        );
    }

    // ---- api tokens -------------------------------------------------------

    fn row_to_token(row: &rusqlite::Row) -> rusqlite::Result<ApiToken> {
        Ok(ApiToken {
            id: row.get("id")?,
            name: row.get("name")?,
            token_prefix: row.get("token_prefix")?,
            scope: row.get("scope")?,
            created_at: row.get("created_at")?,
            last_used_at: row.get("last_used_at")?,
            last_used_ip: row.get("last_used_ip")?,
        })
    }

    pub fn create_api_token(
        &self,
        user_id: &str,
        name: &str,
        prefix: &str,
        hash: &str,
        scope: &str,
    ) -> rusqlite::Result<ApiToken> {
        let id = new_id();
        self.lock().execute(
            "INSERT INTO api_tokens (id, user_id, name, token_prefix, token_hash, scope, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, user_id, name, prefix, hash, scope, now()],
        )?;
        let conn = self.lock();
        conn.query_row("SELECT * FROM api_tokens WHERE id = ?1", [&id], Self::row_to_token)
    }

    pub fn list_api_tokens(&self, user_id: &str) -> Vec<ApiToken> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT * FROM api_tokens WHERE user_id = ?1 ORDER BY created_at DESC")
            .unwrap();
        stmt.query_map([user_id], Self::row_to_token)
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    /// Look up a token by hash; returns (token, owning user id).
    pub fn api_token_by_hash(&self, hash: &str) -> Option<(ApiToken, String)> {
        let conn = self.lock();
        conn.query_row(
            "SELECT * FROM api_tokens WHERE token_hash = ?1",
            [hash],
            |row| Ok((Self::row_to_token(row)?, row.get::<_, String>("user_id")?)),
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn touch_api_token(&self, id: &str, ip: Option<&str>) {
        let _ = self.lock().execute(
            "UPDATE api_tokens SET last_used_at = ?1, last_used_ip = ?2 WHERE id = ?3",
            params![now(), ip, id],
        );
    }

    pub fn delete_api_token(&self, user_id: &str, id: &str) -> bool {
        self.lock()
            .execute(
                "DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    // ---- password resets --------------------------------------------------

    pub fn create_password_reset(&self, user_id: &str, token_hash: &str, expires_at: &str) {
        let conn = self.lock();
        let _ = conn.execute("DELETE FROM password_resets WHERE user_id = ?1", [user_id]);
        let _ = conn.execute(
            "INSERT INTO password_resets (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![token_hash, user_id, expires_at],
        );
    }

    pub fn consume_password_reset(&self, token_hash: &str) -> Option<String> {
        let conn = self.lock();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT user_id, expires_at FROM password_resets WHERE token_hash = ?1",
                [token_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (user_id, expires_at) = row?;
        let _ = conn.execute("DELETE FROM password_resets WHERE token_hash = ?1", [token_hash]);
        if expires_at < now() {
            return None;
        }
        Some(user_id)
    }

    // ---- events -----------------------------------------------------------

    pub fn record_event(
        &self,
        project: &str,
        kind: &str,
        path: &str,
        new_path: Option<&str>,
        actor: &str,
    ) -> Event {
        let created_at = now();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO events (project, kind, path, new_path, actor, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project, kind, path, new_path, actor, created_at],
        )
        .unwrap();
        let seq = conn.last_insert_rowid();
        Event {
            seq,
            project: project.into(),
            kind: kind.into(),
            path: path.into(),
            new_path: new_path.map(String::from),
            actor: actor.into(),
            created_at,
        }
    }

    pub fn events_since(&self, project: &str, since: i64, limit: i64) -> Vec<Event> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT seq, project, kind, path, new_path, actor, created_at
                 FROM events WHERE project = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )
            .unwrap();
        stmt.query_map(params![project, since, limit], |row| {
            Ok(Event {
                seq: row.get(0)?,
                project: row.get(1)?,
                kind: row.get(2)?,
                path: row.get(3)?,
                new_path: row.get(4)?,
                actor: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn latest_seq(&self) -> i64 {
        self.lock()
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))
            .unwrap_or(0)
    }
}
