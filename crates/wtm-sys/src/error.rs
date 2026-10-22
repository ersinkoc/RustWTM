use windows::Win32::Foundation::{
    E_ACCESSDENIED, ERROR_CANCELLED, ERROR_INVALID_PARAMETER, STATUS_ACCESS_DENIED,
    STATUS_INVALID_PARAMETER,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{call} failed with NTSTATUS {status:#010x}")]
    Nt { call: &'static str, status: u32 },

    #[error(transparent)]
    Win32(#[from] windows::core::Error),

    #[error("process {pid} exited and its PID now belongs to another process")]
    Reused { pid: u32 },
}

impl Error {
    pub fn is_access_denied(&self) -> bool {
        match self {
            Error::Nt { status, .. } => *status == STATUS_ACCESS_DENIED.0 as u32,
            Error::Win32(e) => e.code() == E_ACCESSDENIED,
            Error::Reused { .. } => false,
        }
    }

    /// The user declined a UAC prompt.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Win32(e) if e.code() == ERROR_CANCELLED.to_hresult())
    }

    /// `OpenProcess` reports a nonexistent PID as an invalid parameter.
    pub fn is_invalid_parameter(&self) -> bool {
        match self {
            Error::Nt { status, .. } => *status == STATUS_INVALID_PARAMETER.0 as u32,
            Error::Win32(e) => e.code() == ERROR_INVALID_PARAMETER.to_hresult(),
            Error::Reused { .. } => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_access_denied_and_invalid_parameter() {
        let nt_denied = Error::Nt {
            call: "x",
            status: STATUS_ACCESS_DENIED.0 as u32,
        };
        let nt_invalid = Error::Nt {
            call: "x",
            status: STATUS_INVALID_PARAMETER.0 as u32,
        };
        let w_denied = Error::Win32(E_ACCESSDENIED.into());
        let w_invalid = Error::Win32(ERROR_INVALID_PARAMETER.to_hresult().into());
        assert!(Error::Win32(ERROR_CANCELLED.to_hresult().into()).is_cancelled());
        assert!(!w_invalid.is_cancelled());
        assert!(nt_denied.is_access_denied() && !nt_denied.is_invalid_parameter());
        assert!(nt_invalid.is_invalid_parameter() && !nt_invalid.is_access_denied());
        assert!(w_denied.is_access_denied() && !w_denied.is_invalid_parameter());
        assert!(w_invalid.is_invalid_parameter() && !w_invalid.is_access_denied());
        assert!(nt_denied.to_string().contains("0xc0000022"));
    }
}
