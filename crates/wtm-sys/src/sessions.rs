//! Logon sessions (Remote Desktop Services API).

use serde::{Deserialize, Serialize};
use windows::Win32::System::RemoteDesktop::{
    WTS_CONNECTSTATE_CLASS, WTS_INFO_CLASS, WTS_SESSION_INFOW, WTSActive, WTSConnectQuery,
    WTSConnected, WTSDisconnectSession, WTSDisconnected, WTSDomainName, WTSDown,
    WTSEnumerateSessionsW, WTSFreeMemory, WTSIdle, WTSInit, WTSListen, WTSLogoffSession,
    WTSQuerySessionInformationW, WTSReset, WTSSendMessageW, WTSShadow, WTSUserName,
};
use windows::core::PWSTR;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Active,
    Connected,
    ConnectQuery,
    Shadow,
    Disconnected,
    Idle,
    Listen,
    Reset,
    Down,
    Init,
    Unknown,
}

impl SessionState {
    // The windows crate names these constants in CamelCase.
    #[allow(non_upper_case_globals)]
    fn from_raw(raw: WTS_CONNECTSTATE_CLASS) -> Self {
        match raw {
            WTSActive => Self::Active,
            WTSConnected => Self::Connected,
            WTSConnectQuery => Self::ConnectQuery,
            WTSShadow => Self::Shadow,
            WTSDisconnected => Self::Disconnected,
            WTSIdle => Self::Idle,
            WTSListen => Self::Listen,
            WTSReset => Self::Reset,
            WTSDown => Self::Down,
            WTSInit => Self::Init,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: u32,
    pub station: String,
    /// Empty for sessions without a logged-on user (e.g. session 0).
    pub user: String,
    pub domain: String,
    pub state: SessionState,
}

fn query_string(session: u32, class: WTS_INFO_CLASS) -> String {
    let mut buf = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: local server; the API allocates `buf`, freed below.
    if unsafe { WTSQuerySessionInformationW(None, session, class, &mut buf, &mut bytes) }.is_err() {
        return String::new();
    }
    // SAFETY: on success `buf` is a NUL-terminated string from the API.
    let s = unsafe { buf.to_string() }.unwrap_or_default();
    // SAFETY: allocated by WTSQuerySessionInformationW.
    unsafe { WTSFreeMemory(buf.as_ptr().cast()) };
    s
}

pub fn sessions() -> Result<Vec<Session>> {
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: local server; the API allocates the array, freed below.
    unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut info, &mut count)? };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        // SAFETY: the API returned `count` entries.
        let s = unsafe { &*info.add(i) };
        out.push(Session {
            id: s.SessionId,
            // SAFETY: NUL-terminated string inside the API allocation.
            station: unsafe { s.pWinStationName.to_string() }.unwrap_or_default(),
            user: query_string(s.SessionId, WTSUserName),
            domain: query_string(s.SessionId, WTSDomainName),
            state: SessionState::from_raw(s.State),
        });
    }
    // SAFETY: allocated by WTSEnumerateSessionsW.
    unsafe { WTSFreeMemory(info.cast()) };
    Ok(out)
}

/// Disconnects `session`: its programs keep running and the user can sign
/// in again to continue. Other users' sessions need administrator rights.
pub fn disconnect(session: u32) -> Result<()> {
    // SAFETY: local server; plain FFI call.
    unsafe { WTSDisconnectSession(None, session, false)? };
    Ok(())
}

/// Longest title and text sent, in characters.
pub const MESSAGE_TITLE_MAX: usize = 256;
pub const MESSAGE_TEXT_MAX: usize = 4096;

/// UTF-16 of `s`, cut to `max` characters.
fn utf16_bounded(s: &str, max: usize) -> Vec<u16> {
    s.chars()
        .take(max)
        .collect::<String>()
        .encode_utf16()
        .collect()
}

/// Shows a message box with `title` and `text` on `session`'s desktop, as
/// Task Manager's Send message, without waiting for the user to close it.
/// Other users' sessions need administrator rights.
pub fn send_message(session: u32, title: &str, text: &str) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MESSAGEBOX_RESULT};
    let mut title = utf16_bounded(title, MESSAGE_TITLE_MAX);
    let mut text = utf16_bounded(text, MESSAGE_TEXT_MAX);
    let (title_bytes, text_bytes) = ((title.len() * 2) as u32, (text.len() * 2) as u32);
    title.push(0);
    text.push(0);
    let mut response = MESSAGEBOX_RESULT(0);
    // SAFETY: both strings are NUL-terminated and outlive the call, their
    // lengths are given in bytes without the NUL; `bwait` is false, so the
    // call returns at once and `response` is only written, never awaited.
    unsafe {
        WTSSendMessageW(
            None,
            session,
            windows::core::PCWSTR(title.as_ptr()),
            title_bytes,
            windows::core::PCWSTR(text.as_ptr()),
            text_bytes,
            MB_OK | MB_ICONINFORMATION,
            0,
            &mut response,
            false,
        )?
    };
    Ok(())
}

/// Signs the user of `session` out; their programs close and unsaved work
/// is lost. Other users' sessions need administrator rights.
pub fn log_off(session: u32) -> Result<()> {
    // SAFETY: local server; plain FFI call.
    unsafe { WTSLogoffSession(None, session, false)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_cut_to_their_limits_by_character() {
        assert_eq!(
            utf16_bounded("hello", 3),
            "hel".encode_utf16().collect::<Vec<_>>()
        );
        // A character outside the BMP stays whole (two UTF-16 units).
        assert_eq!(utf16_bounded("😀ab", 1).len(), 2);
        assert!(utf16_bounded("", 10).is_empty());
    }

    #[test]
    fn unknown_sessions_cannot_be_disconnected_or_signed_out() {
        assert!(disconnect(0xFFFF_0000).is_err());
        assert!(log_off(0xFFFF_0000).is_err());
    }

    #[test]
    fn enumerates_sessions() {
        let sessions = sessions().unwrap();
        // Session 0 hosts services on every Windows installation.
        assert!(sessions.iter().any(|s| s.id == 0), "{sessions:?}");
        // On an interactive desktop the current user has a session too
        // (CI runners may not have one).
        if let Ok(me) = std::env::var("USERNAME")
            && std::env::var_os("CI").is_none()
        {
            assert!(sessions.iter().any(|s| s.user.eq_ignore_ascii_case(&me)));
        }
    }

    #[test]
    fn maps_every_connect_state() {
        let states: Vec<SessionState> = [
            WTSActive,
            WTSConnected,
            WTSConnectQuery,
            WTSShadow,
            WTSDisconnected,
            WTSIdle,
            WTSListen,
            WTSReset,
            WTSDown,
            WTSInit,
            WTS_CONNECTSTATE_CLASS(99),
        ]
        .into_iter()
        .map(SessionState::from_raw)
        .collect();
        assert_eq!(
            states,
            [
                SessionState::Active,
                SessionState::Connected,
                SessionState::ConnectQuery,
                SessionState::Shadow,
                SessionState::Disconnected,
                SessionState::Idle,
                SessionState::Listen,
                SessionState::Reset,
                SessionState::Down,
                SessionState::Init,
                SessionState::Unknown,
            ]
        );
    }

    #[test]
    fn unknown_session_has_empty_strings() {
        assert_eq!(query_string(u32::MAX - 1, WTSUserName), "");
    }
}
