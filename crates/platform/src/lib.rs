//! Cross-cutting platform concerns shared by all domains.
pub mod auth;
pub mod config;
pub mod db;
pub mod http_client;
pub mod metrics;
pub mod observability;
pub mod server;

pub use config::Settings;
pub use server::AppError;
