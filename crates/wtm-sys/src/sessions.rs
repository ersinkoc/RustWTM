//! Logon sessions (Remote Desktop Services API).

use serde::{Deserialize, Serialize};
use windows::Win32::System::RemoteDesktop::{
    WTS_CONNECTSTATE_CLASS, WTS_INFO_CLASS, WTS_SESSION_INFOW, WTSActive, WTSConnectQuery,
    WTSConnected, WTSDisconnected, WTSDomainName, WTSDown, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSIdle, WTSInit, WTSListen, WTSQuerySessionInformationW, WTSReset, WTSShadow, WTSUserName,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
