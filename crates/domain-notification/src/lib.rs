//! Notification domain: consumes account.created, renders + dispatches notifications.
pub mod models;
pub mod openapi;
pub mod ports;

pub use ports::http::{router, NotificationState};
