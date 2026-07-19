//! Engine-agnostic product logic for the AI workspace.
//!
//! The single seam between an agent engine and the rest of the application is
//! [`engine::AgentEngine`], which streams [`engine::EngineEvent`] values. No
//! engine-specific type may cross that boundary — see `engine/adk/` for the
//! only module permitted to import `adk_*` crates.

pub mod engine;
pub mod error;
pub mod providers;

pub use error::{Error, Result};
