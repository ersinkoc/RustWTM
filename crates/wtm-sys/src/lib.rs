//! Safe wrappers over the Win32 / NT APIs used by RustWTM.
//!
//! This is the only crate in the workspace that contains `unsafe` code.

#![cfg(windows)]

pub mod control;
pub mod cpu;
pub mod disk;
pub mod disktrace;
pub mod ending;
mod error;
pub mod etw;
mod fasthash;
pub mod firewall;
pub mod gpu;
mod handle;
pub mod http;
pub mod icon;
pub mod job;
pub mod modules;
pub mod net;
pub mod nettrace;
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
pub mod volumes;
pub mod waits;
pub mod window;

pub use error::{Error, Result};

/// `String::from_utf16_lossy` with room for the common all-ASCII case
/// reserved up front: that one grows from half the length, so most short
/// strings (process names, paths) were allocated twice.
pub(crate) fn utf16_lossy(units: &[u16]) -> String {
    let mut s = String::with_capacity(units.len());
    s.extend(
        char::decode_utf16(units.iter().copied()).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)),
    );
    s
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn utf16_lossy_matches_std_in_one_allocation() {
        let cases: [&[u16]; 4] = [
            &[],
            &[0x61, 0x62, 0x63],
            // Non-ASCII, a surrogate pair and a lone surrogate.
            &[0x00e7, 0xd83d, 0xde00, 0xd800, 0x41],
            &[0xdc00],
        ];
        for units in cases {
            assert_eq!(super::utf16_lossy(units), String::from_utf16_lossy(units));
        }
        let ascii: Vec<u16> = "explorer.exe".encode_utf16().collect();
        assert_eq!(super::utf16_lossy(&ascii).capacity(), ascii.len());
    }
}
