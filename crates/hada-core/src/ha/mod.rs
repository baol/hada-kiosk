//! Home Assistant connectivity.

pub mod auth;
pub mod http;
pub mod messages;
pub mod ws;

pub use auth::{Auth, AuthState};
pub use ws::{run, DashboardSource, HaCommand, HaConfig, HaEvent};
