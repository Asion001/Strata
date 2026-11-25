//! Strata server library: everything except process start-up lives here so
//! integration tests can build the router directly.

pub mod auth;
pub mod background;
pub mod config;
pub mod connections;
pub mod error;
pub mod files;
pub mod health;
pub mod journal;
pub mod mounts;
pub mod range;
pub mod routes;
pub mod safe;
pub mod shares;
pub mod state;
pub mod thumbs;
pub mod trash;
pub mod uploads;
pub mod vfs;

pub use config::Config;
pub use state::AppState;
