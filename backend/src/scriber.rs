//! External memory provider client for a Scriber instance.
//!
//! Scriber is mounted as a read/write *virtual project* (default name
//! `scriber`): each Scriber participant appears as a user folder containing a
//! single `memory.md`. The wire contract is Scriber's public API (see the
//! README section "Compatible memory providers"):
//!
//!   GET /api/v1/participants?limit=&offset=   -> {"total", "items":[{"id","display_name",..}]}
//!   GET /api/v1/participants/{id}/memory      -> {"content": "..."}
//!   PUT /api/v1/participants/{id}/memory      {"content": "..."} -> {"ok": true}
//!
//! Auth: `Authorization: Bearer <api token>` (readwrite scope needed for PUT).
//! Any service implementing these three endpoints can be mounted the same way.

use serde::Deserialize;

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

#[derive(Deserialize)]
struct MemoryBody {
    content: String,
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

    pub async fn read_memory(&self, participant_id: &str) -> Result<String, String> {
        let url = format!(
            "{}/api/v1/participants/{}/memory",
            self.base_url, participant_id
        );
        let body: MemoryBody = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        Ok(body.content)
    }

    pub async fn write_memory(&self, participant_id: &str, content: &str) -> Result<(), String> {
        let url = format!(
            "{}/api/v1/participants/{}/memory",
            self.base_url, participant_id
        );
        self.auth(self.http.put(&url))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Folder label for a participant: "<display_name> (<id>)" or the id.
    pub fn folder_name(p: &Participant) -> String {
        match &p.display_name {
            Some(name) if !name.trim().is_empty() => {
                // Keep it a valid single path segment.
                let safe: String = name
                    .chars()
                    .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
                    .collect();
                format!("{} ({})", safe.trim(), p.id)
            }
            _ => p.id.clone(),
        }
    }

    /// Extract the participant id back out of a folder label.
    pub fn participant_id_from_folder(folder: &str) -> String {
        if let Some(open) = folder.rfind('(') {
            if let Some(close) = folder[open..].find(')') {
                return folder[open + 1..open + close].to_string();
            }
        }
        folder.to_string()
    }
}
