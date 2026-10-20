//! Service Control Manager: list services, start/stop/restart, change start
//! type.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_SERVICE_NOT_ACTIVE,
    ERROR_SERVICE_REQUEST_TIMEOUT,
};
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW,
    ENUM_SERVICE_TYPE, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW,
    QueryServiceConfigW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_AUTO_START, SERVICE_BOOT_START,
    SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START, SERVICE_DISABLED,
    SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_START,
    SERVICE_START_TYPE, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP,
    SERVICE_SYSTEM_START, SERVICE_WIN32, StartServiceW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown(u32),
}

impl ServiceState {
    fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartType {
    Boot,
    System,
    Automatic,
    Manual,
    Disabled,
}

impl StartType {
    fn from_raw(raw: SERVICE_START_TYPE) -> Option<Self> {
        Some(match raw {
            SERVICE_BOOT_START => Self::Boot,
            SERVICE_SYSTEM_START => Self::System,
            SERVICE_AUTO_START => Self::Automatic,
            SERVICE_DEMAND_START => Self::Manual,
            SERVICE_DISABLED => Self::Disabled,
            _ => return None,
        })
    }

    fn to_raw(self) -> SERVICE_START_TYPE {
        match self {
            Self::Boot => SERVICE_BOOT_START,
            Self::System => SERVICE_SYSTEM_START,
            Self::Automatic => SERVICE_AUTO_START,
            Self::Manual => SERVICE_DEMAND_START,
            Self::Disabled => SERVICE_DISABLED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub name: String,
    pub display_name: String,
    pub state: ServiceState,
    /// 0 when not running.
    pub pid: u32,
    pub start_type: Option<StartType>,
    pub binary_path: Option<String>,
    pub account: Option<String>,
}

struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> Result<ScHandle> {
    // SAFETY: local SCM, default database.
    Ok(ScHandle(unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access)?
    }))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn open(name: &str, access: u32) -> Result<ScHandle> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    Ok(ScHandle(unsafe {
        OpenServiceW(scm.0, PCWSTR(name.as_ptr()), access)?
    }))
}

/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
unsafe fn read_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    unsafe { p.to_string() }.ok()
}

/// Lists all Win32 services. Start type, binary path and account are
/// `None` where the caller lacks `SERVICE_QUERY_CONFIG` access.
pub fn list_services() -> Result<Vec<ServiceEntry>> {
    list_services_with(64 * 1024)
}

/// `initial_bytes` is the first enumeration buffer size; it grows as needed.
fn list_services_with(initial_bytes: usize) -> Result<Vec<ServiceEntry>> {
    let scm = manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)?;
    // u64 elements keep the buffer pointer-aligned.
    let mut buf: Vec<u64> = vec![0; initial_bytes.div_ceil(8).max(1)];
    let mut out = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut needed = 0u32;
        let mut count = 0u32;
        // SAFETY: the u8 view covers exactly `buf`'s allocation.
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len() * 8) };
        // SAFETY: valid SCM handle, buffer and out-pointers.
        let result = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(bytes),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let more = match result {
            Ok(()) => false,
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(e) => return Err(e.into()),
        };
        let entries = buf.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for i in 0..count as usize {
            // SAFETY: the SCM wrote `count` entries at the start of `buf`;
            // their string pointers point into `buf` as well.
            let e = unsafe { &*entries.add(i) };
            // SAFETY: strings written by the SCM are NUL-terminated.
            let (name, display_name) =
                unsafe { (read_pwstr(e.lpServiceName), read_pwstr(e.lpDisplayName)) };
            let name = name.unwrap_or_default();
            out.push(ServiceEntry {
                display_name: display_name.unwrap_or_else(|| name.clone()),
                state: ServiceState::from_raw(e.ServiceStatusProcess.dwCurrentState.0),
                pid: e.ServiceStatusProcess.dwProcessId,
                start_type: None,
                binary_path: None,
                account: None,
                name,
            });
        }
        if !more {
            break;
        }
        if needed as usize > buf.len() * 8 {
            buf.resize((needed as usize).div_ceil(8), 0);
        }
    }

    for s in &mut out {
        if let Ok(config) = query_config(&s.name) {
            s.start_type = config.0;
            s.binary_path = config.1;
            s.account = config.2;
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    Ok(out)
}

type Config = (Option<StartType>, Option<String>, Option<String>);

fn query_config(name: &str) -> Result<Config> {
    let svc = open(name, SERVICE_QUERY_CONFIG)?;
    let mut needed = 0u32;
    // SAFETY: size query with no buffer.
    match unsafe { QueryServiceConfigW(svc.0, None, 0, &mut needed) } {
        Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        Err(e) => return Err(e.into()),
        Ok(()) => {}
    }
    let mut buf: Vec<u64> = vec![0; (needed as usize).div_ceil(8).max(1)];
    let config = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: `buf` holds at least `needed` bytes, suitably aligned.
    unsafe { QueryServiceConfigW(svc.0, Some(config), (buf.len() * 8) as u32, &mut needed)? };
    // SAFETY: filled by the call above; strings point into `buf`.
    let c = unsafe { &*config };
    // SAFETY: SCM strings are NUL-terminated.
    let (path, account) = unsafe {
        (
            read_pwstr(c.lpBinaryPathName),
            read_pwstr(c.lpServiceStartName),
        )
    };
    Ok((StartType::from_raw(c.dwStartType), path, account))
}

fn query_status(svc: &ScHandle) -> Option<SERVICE_STATUS_PROCESS> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    // SAFETY: the byte view covers exactly status.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            (&raw mut status).cast::<u8>(),
            size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    // SAFETY: valid handle and buffer.
    unsafe { QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(bytes), &mut needed) }
        .ok()?;
    Some(status)
}

/// PID of a running service, None if it is not installed or not running.
pub fn service_pid(name: &str) -> Option<u32> {
    let svc = open(name, SERVICE_QUERY_STATUS).ok()?;
    let status = query_status(&svc)?;
    (status.dwProcessId != 0).then_some(status.dwProcessId)
}

/// Stops `name` (if it runs), waits up to `timeout` for it to stop, and
/// starts it again.
pub fn restart_service(name: &str, timeout: Duration) -> Result<()> {
    let svc = open(name, SERVICE_STOP | SERVICE_START | SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS::default();
    // SAFETY: valid handle; `status` is writable.
    if let Err(e) = unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) }
        && e.code() != ERROR_SERVICE_NOT_ACTIVE.to_hresult()
    {
        return Err(e.into());
    }
    let deadline = Instant::now() + timeout;
    while query_status(&svc)
        .is_some_and(|s| ServiceState::from_raw(s.dwCurrentState.0) != ServiceState::Stopped)
    {
        if Instant::now() >= deadline {
            return Err(
                windows::core::Error::from(ERROR_SERVICE_REQUEST_TIMEOUT.to_hresult()).into(),
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // SAFETY: valid handle; no arguments.
    unsafe { StartServiceW(svc.0, None)? };
    Ok(())
}

pub fn start_service(name: &str) -> Result<()> {
    let svc = open(name, SERVICE_START)?;
    // SAFETY: valid handle; no arguments.
    unsafe { StartServiceW(svc.0, None)? };
    Ok(())
}

pub fn stop_service(name: &str) -> Result<()> {
    let svc = open(name, SERVICE_STOP)?;
    let mut status = SERVICE_STATUS::default();
    // SAFETY: valid handle; `status` is writable.
    unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status)? };
    Ok(())
}

pub fn set_start_type(name: &str, start_type: StartType) -> Result<()> {
    let svc = open(name, SERVICE_CHANGE_CONFIG)?;
    // SAFETY: valid handle; every other setting is left unchanged.
    unsafe {
        ChangeServiceConfigW(
            svc.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            start_type.to_raw(),
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )?
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_well_known_services() {
        let services = list_services().unwrap();
        assert!(services.len() > 50, "only {} services", services.len());
        // The RPC endpoint mapper runs on every Windows machine.
        let rpc = services
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("RpcSs"))
            .unwrap();
        assert_eq!(rpc.state, ServiceState::Running);
        assert!(rpc.pid != 0);
        assert_eq!(rpc.start_type, Some(StartType::Automatic));
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    const MISSING: &str = "NoSuchServiceForRustWTMTests";

    #[test]
    fn unknown_services_fail_cleanly() {
        assert!(start_service(MISSING).is_err());
        assert!(stop_service(MISSING).is_err());
        assert!(restart_service(MISSING, Duration::from_secs(1)).is_err());
        assert!(set_start_type(MISSING, StartType::Manual).is_err());
        assert!(service_pid(MISSING).is_none());
        assert!(query_config(MISSING).is_err());
    }

    #[test]
    fn running_service_has_a_pid() {
        assert!(service_pid("RpcSs").is_some());
    }

    #[test]
    fn state_and_start_type_conversions() {
        for raw in 1..=8 {
            let _ = ServiceState::from_raw(raw);
        }
        assert_eq!(ServiceState::from_raw(99), ServiceState::Unknown(99));
        for t in [
            StartType::Boot,
            StartType::System,
            StartType::Automatic,
            StartType::Manual,
            StartType::Disabled,
        ] {
            assert_eq!(StartType::from_raw(t.to_raw()), Some(t));
        }
        assert_eq!(StartType::from_raw(SERVICE_START_TYPE(99)), None);
    }

    #[test]
    fn null_strings_read_as_none() {
        // SAFETY: a null pointer is allowed and never dereferenced.
        assert_eq!(unsafe { read_pwstr(PWSTR::null()) }, None);
    }

    #[test]
    fn tiny_buffer_grows_to_list_everything() {
        // Smaller than a single entry, so the SCM reports ERROR_MORE_DATA.
        let small = list_services_with(8).unwrap();
        assert!(small.len() > 50, "only {} services", small.len());
        assert!(small.iter().any(|s| s.name.eq_ignore_ascii_case("RpcSs")));
        let mut names: Vec<&str> = small.iter().map(|s| s.name.as_str()).collect();
        names.dedup();
        assert_eq!(names.len(), small.len(), "duplicate entries");
    }
}

#[cfg(test)]
mod scratch_service_tests {
    use super::*;
    use windows::Win32::System::Services::{
        CreateServiceW, DeleteService, SC_MANAGER_CREATE_SERVICE, SERVICE_ERROR_IGNORE,
        SERVICE_WIN32_OWN_PROCESS,
    };

    /// Standard `DELETE` access right.
    const DELETE: u32 = 0x0001_0000;

    /// A throwaway service created by the test, deleted on drop (also when
    /// the test fails). Real services are never started, stopped or changed.
    struct ScratchService(String);

    impl ScratchService {
        /// Needs elevation. Only runs on CI so a developer workstation never
        /// gets a service registered, not even temporarily.
        fn create() -> Option<Self> {
            std::env::var_os("CI")?;
            let scm = manager(SC_MANAGER_CREATE_SERVICE).ok()?;
            let name = format!("RustWTMTest{}", std::process::id());
            let wname = wide(&name);
            let path = wide(r"C:\RustWTM-test\does-not-exist.exe");
            // SAFETY: the strings are NUL-terminated and outlive the call;
            // the returned handle is closed by `ScHandle`.
            let svc = unsafe {
                CreateServiceW(
                    scm.0,
                    PCWSTR(wname.as_ptr()),
                    PCWSTR(wname.as_ptr()),
                    DELETE,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_DEMAND_START,
                    SERVICE_ERROR_IGNORE,
                    PCWSTR(path.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
            }
            .ok()?;
            drop(ScHandle(svc));
            Some(Self(name))
        }
    }

    impl Drop for ScratchService {
        fn drop(&mut self) {
            if let Ok(svc) = open(&self.0, DELETE) {
                // SAFETY: valid handle opened with DELETE access.
                let _ = unsafe { DeleteService(svc.0) };
            }
        }
    }

    #[test]
    fn manages_a_scratch_service() {
        let Some(svc) = ScratchService::create() else {
            return;
        };
        let name = svc.0.as_str();
        set_start_type(name, StartType::Disabled).unwrap();
        assert_eq!(query_config(name).unwrap().0, Some(StartType::Disabled));
        set_start_type(name, StartType::Manual).unwrap();

        // The binary does not exist, and a stopped service cannot be stopped.
        assert!(start_service(name).is_err());
        assert!(stop_service(name).is_err());
        assert_eq!(service_pid(name), None);

        let listed = list_services().unwrap();
        let entry = listed.iter().find(|s| s.name == name).unwrap();
        assert_eq!(entry.state, ServiceState::Stopped);
        assert_eq!(entry.start_type, Some(StartType::Manual));
    }
}
