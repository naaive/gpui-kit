//! The launcher as a desktop citizen: platform services the pages rely on.
//!
//! Everything platform-specific lives behind this module so the rest of the
//! launcher states intent ("paste into the previous application") and never
//! branches on the operating system.

pub mod platform;
pub mod settings;
