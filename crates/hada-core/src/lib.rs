//! Core library for `hada`: Lovelace dashboard model, Home Assistant websocket
//! client, entity state store, per-domain display/control rules and action mapping.
//!
//! This crate has no UI dependency so that everything in it can be unit tested.

pub mod actions;
pub mod artwork;
pub mod domain;
pub mod ha;
pub mod icons;
pub mod lovelace;
pub mod media;
pub mod model;
pub mod normalise;
pub mod state;
pub mod timefmt;
pub mod translations;
pub mod weather;

pub use actions::{Action, EntityAction, ServiceCall};
pub use model::Dashboard;
pub use state::{EntityState, StateStore};
