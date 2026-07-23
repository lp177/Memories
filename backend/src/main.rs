//! Memories — self-hosted memory store for LLM workflows.
//! One process serves the REST API (/api), the MCP endpoint (/mcp) and the
//! built Vue SPA (/) when frontend/dist exists.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use memories::api::{self, AppState};
use memories::auth;
use memories::config::Config;
use memories::db::Db;
use memories::events::EventHub;
use memories::mcp;
use memories::scriber::ScriberClient;
use memories::store::Store;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=warn".into()),
        )
        .init();

    let cfg = Config::from_env();
    std::fs::create_dir_all(&cfg.data_dir).expect("create data dir");
    let db = Db::open(&cfg.data_dir.join("memories.db")).expect("open database");

    // Bootstrap the admin account on first run.
    if db.list_users().is_empty() {
        let hash = auth::hash_password(&cfg.admin_password);
        db.create_user(&cfg.admin_username, "", &hash, true)
            .expect("bootstrap admin");
        tracing::info!(
            "Created bootstrap admin '{}' (set ADMIN_USERNAME / ADMIN_PASSWORD to change).",
            cfg.admin_username
        );
    }

    let scriber = match (&cfg.scriber_url, &cfg.scriber_token) {
        (Some(url), Some(token)) => {
            tracing::info!("External scriber provider mounted at '{}' -> {}", cfg.scriber_mount, url);
            Some(ScriberClient::new(url, token))
        }
        _ => None,
    };

    let store = Store::new(&cfg.data_dir);
    let state = Arc::new(AppState {
        store,
        db,
        hub: EventHub::new(),
        scriber,
        cfg: cfg.clone(),
    });

    let mut app = Router::new()
        .merge(api::router(state.clone()))
        .merge(mcp::router(state.clone()));

    // Serve the built SPA when present (dev setups can run Vite instead).
    let dist = std::path::Path::new("../frontend/dist");
    let dist = if dist.is_dir() {
        Some(dist.to_path_buf())
    } else {
        let alt = std::path::Path::new("frontend/dist");
        alt.is_dir().then(|| alt.to_path_buf())
    };
    if let Some(dist) = dist {
        let index = dist.join("index.html");
        app = app.fallback_service(ServeDir::new(&dist).fallback(ServeFile::new(index)));
        tracing::info!("Serving SPA from {}", dist.display());
    }

    let addr: SocketAddr = cfg.bind.parse().expect("valid WEB_HOST/WEB_PORT");
    tracing::info!("Listening on http://{addr} (API /api, MCP /mcp)");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server");
}
