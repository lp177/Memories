//! External memory provider client for a Scriber instance.
//!
//! Scriber is mounted as a read/write *virtual project* (default name
//! `scriber`): each Scriber participant appears as a user folder containing a
//! single `memory.md`, and a `meetings` folder holds one Markdown file per
//! meeting (the AI-generated minutes). The wire contract is Scriber's public
//! API (see the README section "Compatible memory providers"):
//!
//!   GET /api/v1/participants?limit=&offset=    -> {"total", "items":[{"id","display_name",..}]}
//!   GET /api/v1/participants/{id}/memory       -> {"content": "..."}
//!   PUT /api/v1/participants/{id}/memory       {"content": "..."} -> {"ok": true}
//!   GET /api/v1/meetings?limit=&offset=        -> {"total", "items":[{"id","started_at",
//!                                                  "voice_channel_name","guild_name","has_summary",..}]}
//!   GET /api/v1/meetings/{id}/summary          -> the minutes as text/markdown (NOT JSON)
//!   PUT /api/v1/meetings/{id}/summary          {"content": "..."} -> {"ok": true}
//!
//! Auth: `Authorization: Bearer <api token>` (readwrite scope needed for PUT).
//! Any service implementing these endpoints can be mounted the same way.
//!
//! The path synthesis/classification helpers below are the single source of
//! truth shared by the REST handlers (api.rs) and the MCP tools (mcp.rs) so
//! the two surfaces cannot drift.

use serde::Deserialize;

use crate::store::{sort_entries, Entry};

/// Reserved folder at the mount root holding one file per meeting. Cannot
/// collide with a participant folder: those are either a bare id or end in
/// "(<id>)".
pub const MEETINGS_DIR: &str = "meetings";

#[derive(Clone)]
pub struct ScriberClient {
    pub base_url: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Deserialize, Debug)]
pub struct Participant {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
struct ParticipantsPage {
    total: i64,
    items: Vec<Participant>,
}

#[derive(Deserialize, Debug)]
pub struct Meeting {
    pub id: String,
    pub started_at: Option<String>,
    pub voice_channel_name: Option<String>,
    pub guild_name: Option<String>,
    #[serde(default)]
    pub has_summary: bool,
}

#[derive(Deserialize)]
struct MeetingsPage {
    total: i64,
    items: Vec<Meeting>,
}

/// A file inside the mount, classified from its path.
pub enum MountFile {
    /// `<mount>/<participant folder>/memory.md` -> participant id
    ParticipantMemory(String),
    /// `<mount>/meetings/<file>.md` -> meeting id
    MeetingSummary(String),
}

/// Upstream failure, keeping 404 distinguishable so callers can answer
/// "file not found" instead of "bad gateway".
#[derive(Debug, PartialEq)]
pub enum ScriberError {
    NotFound,
    Upstream(String),
}

fn up(e: reqwest::Error) -> ScriberError {
    ScriberError::Upstream(e.to_string())
}

impl ScriberClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("Bearer {}", self.token))
    }

    /// All participants (paginated under the hood).
    pub async fn participants(&self) -> Result<Vec<Participant>, String> {
        let mut out = Vec::new();
        let mut offset = 0i64;
        loop {
            let url = format!(
                "{}/api/v1/participants?limit=100&offset={}",
                self.base_url, offset
            );
            let page: ParticipantsPage = self
                .auth(self.http.get(&url))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let n = page.items.len() as i64;
            out.extend(page.items);
            offset += n;
            if offset >= page.total || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    /// All meetings (paginated under the hood), newest first as served.
    pub async fn meetings(&self) -> Result<Vec<Meeting>, String> {
        let mut out = Vec::new();
        let mut offset = 0i64;
        loop {
            let url = format!(
                "{}/api/v1/meetings?limit=100&offset={}",
                self.base_url, offset
            );
            let page: MeetingsPage = self
                .auth(self.http.get(&url))
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            let n = page.items.len() as i64;
            out.extend(page.items);
            offset += n;
            if offset >= page.total || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn read_memory(&self, participant_id: &str) -> Result<String, ScriberError> {
        let url = format!(
            "{}/api/v1/participants/{}/memory",
            self.base_url, participant_id
        );
        let resp = self.auth(self.http.get(&url)).send().await.map_err(up)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ScriberError::NotFound);
        }
        let body: MemoryBody = resp.error_for_status().map_err(up)?.json().await.map_err(up)?;
        Ok(body.content)
    }

    pub async fn write_memory(&self, participant_id: &str, content: &str) -> Result<(), ScriberError> {
        let url = format!(
            "{}/api/v1/participants/{}/memory",
            self.base_url, participant_id
        );
        let resp = self
            .auth(self.http.put(&url))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(up)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ScriberError::NotFound);
        }
        resp.error_for_status().map_err(up)?;
        Ok(())
    }

    /// The meeting minutes, served by Scriber as raw text/markdown.
    pub async fn read_meeting_summary(&self, meeting_id: &str) -> Result<String, ScriberError> {
        let url = format!("{}/api/v1/meetings/{}/summary", self.base_url, meeting_id);
        let resp = self.auth(self.http.get(&url)).send().await.map_err(up)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ScriberError::NotFound);
        }
        resp.error_for_status().map_err(up)?.text().await.map_err(up)
    }

    pub async fn write_meeting_summary(&self, meeting_id: &str, content: &str) -> Result<(), ScriberError> {
        let url = format!("{}/api/v1/meetings/{}/summary", self.base_url, meeting_id);
        let resp = self
            .auth(self.http.put(&url))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(up)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ScriberError::NotFound);
        }
        resp.error_for_status().map_err(up)?;
        Ok(())
    }

    /// Keep a label a valid single path segment: no separators, no leading
    /// dot (the store rejects hidden files).
    fn safe_segment(s: &str) -> String {
        s.chars()
            .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
            .collect::<String>()
            .trim()
            .trim_start_matches('.')
            .trim_start()
            .to_string()
    }

    /// Cap a display label so the full synthesized segment stays well under
    /// the store's 255-byte segment limit (cut on a char boundary).
    fn clip_label(s: &str) -> String {
        const MAX: usize = 80;
        if s.len() <= MAX {
            return s.to_string();
        }
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].trim_end().to_string()
    }

    /// Folder label for a participant: "<display_name> (<id>)" or the id.
    pub fn folder_name(p: &Participant) -> String {
        match p.display_name.as_deref().map(Self::safe_segment) {
            Some(name) if !name.is_empty() => format!("{} ({})", Self::clip_label(&name), p.id),
            // A bare id that collides with the reserved meetings folder keeps
            // a decorated (still reversible) label.
            _ if p.id == MEETINGS_DIR => format!("participant ({})", p.id),
            _ => p.id.clone(),
        }
    }

    /// Extract the id back out of a "<label> (<id>)" segment; a segment
    /// without that trailing group IS the id (so opaque ids containing
    /// parentheses still round-trip).
    pub fn participant_id_from_folder(folder: &str) -> String {
        if folder.ends_with(')') {
            if let Some(open) = folder.rfind(" (") {
                return folder[open + 2..folder.len() - 1].to_string();
            }
        }
        folder.to_string()
    }

    /// File label for a meeting: "<date> <channel> (<id>).md" or "<id>.md".
    pub fn meeting_file_name(m: &Meeting) -> String {
        let date = m.started_at.as_deref().and_then(|s| s.get(..10)).unwrap_or("");
        let label = [m.voice_channel_name.as_deref(), m.guild_name.as_deref()]
            .into_iter()
            .flatten()
            .map(Self::safe_segment)
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let prefix = Self::clip_label(&Self::safe_segment(&format!("{date} {label}")));
        if prefix.is_empty() {
            format!("{}.md", m.id)
        } else {
            format!("{} ({}).md", prefix, m.id)
        }
    }

    /// Extract the meeting id back out of a meeting file name.
    pub fn meeting_id_from_file(file: &str) -> String {
        let stem = file.strip_suffix(".md").unwrap_or(file);
        Self::participant_id_from_folder(stem)
    }

    /// Classify a full mount path (`<mount>/<folder>/<file>`) as a readable/
    /// writable file. `None` = no such file in the virtual layout.
    pub fn classify_file(segs: &[String]) -> Option<MountFile> {
        if segs.len() != 3 {
            return None;
        }
        if segs[1] == MEETINGS_DIR {
            if !segs[2].ends_with(".md") {
                return None;
            }
            Some(MountFile::MeetingSummary(Self::meeting_id_from_file(&segs[2])))
        } else if segs[2] == "memory.md" {
            Some(MountFile::ParticipantMemory(Self::participant_id_from_folder(
                &segs[1],
            )))
        } else {
            None
        }
    }

    pub async fn read_mount_file(&self, file: &MountFile) -> Result<String, ScriberError> {
        match file {
            MountFile::ParticipantMemory(id) => self.read_memory(id).await,
            MountFile::MeetingSummary(id) => self.read_meeting_summary(id).await,
        }
    }

    pub async fn write_mount_file(&self, file: &MountFile, content: &str) -> Result<(), ScriberError> {
        match file {
            MountFile::ParticipantMemory(id) => self.write_memory(id, content).await,
            MountFile::MeetingSummary(id) => self.write_meeting_summary(id, content).await,
        }
    }

    /// Synthesize the explorer entries for a mount path: participant folders
    /// plus the `meetings` folder at the root, `memory.md` inside a
    /// participant, one file per summarized meeting inside `meetings`.
    /// `Ok(None)` = the path does not exist in the virtual layout.
    pub async fn mount_entries(
        &self,
        segs: &[String],
        access: &str,
    ) -> Result<Option<Vec<Entry>>, String> {
        let entry = |name: String, parent: &str, is_dir: bool| Entry {
            path: format!("{parent}/{name}"),
            name,
            is_dir,
            size: 0,
            modified_at: None,
            access: access.into(),
            shared: false,
        };
        match segs.len() {
            1 => {
                let mut out = vec![entry(MEETINGS_DIR.into(), &segs[0], true)];
                out.extend(
                    self.participants()
                        .await?
                        .iter()
                        .map(|p| entry(Self::folder_name(p), &segs[0], true)),
                );
                sort_entries(&mut out);
                Ok(Some(out))
            }
            2 if segs[1] == MEETINGS_DIR => {
                let mut out: Vec<Entry> = self
                    .meetings()
                    .await?
                    .iter()
                    .filter(|m| m.has_summary)
                    .map(|m| entry(Self::meeting_file_name(m), &format!("{}/{}", segs[0], segs[1]), false))
                    .collect();
                sort_entries(&mut out);
                Ok(Some(out))
            }
            2 => Ok(Some(vec![entry(
                "memory.md".into(),
                &format!("{}/{}", segs[0], segs[1]),
                false,
            )])),
            _ => Ok(None),
        }
    }
}

#[derive(Deserialize)]
struct MemoryBody {
    content: String,
}
