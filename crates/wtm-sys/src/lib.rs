//! Safe wrappers over the Win32 / NT APIs used by RustWTM.
//!
//! This is the only crate in the workspace that contains `unsafe` code.

#![cfg(windows)]

pub mod control;
mod error;
pub mod gpu;
mod handle;
pub mod job;
pub mod net;
mod ntapi;
pub mod pipe;
pub mod privilege;
pub mod process;
pub mod security;
pub mod services;
pub mod sessions;
pub mod system;

pub use error::{Error, Result};
