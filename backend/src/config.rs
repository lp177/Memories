//! Environment-based configuration. Values come from the process environment
//! and an optional `.env` file (loaded by `dotenvy` in `main`).

use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    /// Bind address, e.g. `0.0.0.0:8090`.
    pub bind: String,
    /// Root data directory (markdown tree, SQLite DB, avatars).
    pub data_dir: PathBuf,
    /// HMAC key for session tokens. Auto-generated at startup when empty
    /// (sessions then reset on restart — set WEB_SECRET for persistence).
    pub web_secret: String,
    /// Bootstrap admin credentials (created on first run).
    pub admin_username: String,
    pub admin_password: String,
    /// Public base URL used in password-reset links, e.g. `https://memories.example.com`.
    pub public_url: String,
    /// Optional SMTP relay for password-reset mails. When unset, reset links
    /// are returned to the admin UI instead of being e-mailed.
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: String,
    /// Optional external scriber provider (mounted as a virtual project).
    pub scriber_url: Option<String>,
    pub scriber_token: Option<String>,
    pub scriber_mount: String,
}

fn var(key: &str, default: &str) -> String {
    env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn var_opt(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Self {
        let web_secret = var_opt("WEB_SECRET").unwrap_or_else(|| {
            use rand::Rng;
            let bytes: [u8; 32] = rand::thread_rng().gen();
            tracing::warn!("WEB_SECRET is not set — generated an ephemeral one (sessions reset on restart).");
            hex::encode(bytes)
        });
        Self {
            bind: format!(
                "{}:{}",
                var("WEB_HOST", "0.0.0.0"),
                var("WEB_PORT", "8090")
            ),
            data_dir: PathBuf::from(var("MEMORIES_DATA_DIR", "./data")),
            web_secret,
            admin_username: var("ADMIN_USERNAME", "admin"),
            admin_password: var("ADMIN_PASSWORD", "change-me"),
            public_url: var("PUBLIC_URL", "http://localhost:8090"),
            smtp_host: var_opt("SMTP_HOST"),
            smtp_port: var("SMTP_PORT", "587").parse().unwrap_or(587),
            smtp_username: var_opt("SMTP_USERNAME"),
            smtp_password: var_opt("SMTP_PASSWORD"),
            smtp_from: var("SMTP_FROM", "memories@localhost"),
            scriber_url: var_opt("SCRIBER_URL"),
            scriber_token: var_opt("SCRIBER_TOKEN"),
            scriber_mount: var("SCRIBER_MOUNT", "scriber"),
        }
    }
}
