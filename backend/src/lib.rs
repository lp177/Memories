//! Memories library crate — exposed so integration tests (and potential
//! embedders) can reuse the modules; the binary in `main.rs` wires them up.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod events;
pub mod mail;
pub mod mcp;
pub mod scriber;
pub mod store;
