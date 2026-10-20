//! Safe wrappers over the Win32 / NT APIs used by RustWTM.
//!
//! This is the only crate in the workspace that contains `unsafe` code.

#![cfg(windows)]

pub mod control;
pub mod cpu;
pub mod disk;
mod error;
pub mod gpu;
mod handle;
pub mod http;
pub mod icon;
pub mod job;
pub mod net;
mod ntapi;
pub mod pipe;
pub mod power;
pub mod privilege;
pub mod process;
pub mod security;
pub mod services;
pub mod sessions;
pub mod shell;
pub mod system;
pub mod window;

pub use error::{Error, Result};

/// The user's Windows UI language, independent of regional number formats.
pub fn system_ui_language() -> String {
    // SAFETY: GetUserDefaultUILanguage takes no pointers or owned resources.
    let language = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    match language & 0x03ff {
        0x1f => "tr",
        0x07 => "de",
        0x0c => "fr",
        0x0a => "es",
        _ => "en",
    }
    .to_owned()
}
