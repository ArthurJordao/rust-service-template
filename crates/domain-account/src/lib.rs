//! Account domain: pure rules + ports + HTTP/event adapters.
pub mod domain;
pub mod models;
pub mod openapi;
pub mod ports;

pub use ports::http::{router, AccountState};
