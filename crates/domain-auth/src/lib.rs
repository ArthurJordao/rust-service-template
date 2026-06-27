//! Auth domain: users, scopes, JWT issuance, login/register/refresh/logout.
pub mod auth;
pub mod domain;
pub mod models;
pub mod openapi;
pub mod ports;

pub use ports::http::{router, AuthState};
