//! Process model, sampling and control for RustWTM.

#![forbid(unsafe_code)]

pub mod control;
pub mod diskscan;
pub use wtm_sys::cpu;
pub mod hardware;
pub mod launch;
mod model;
pub mod ports;
pub mod power;
mod protection;
mod sampler;
pub mod startup;
mod tree;
pub mod update;

pub use model::{
    ProcessInfo, Snapshot, SortKey, SystemStats, format_bytes, format_duration, sort_processes,
};
pub use protection::protection_reason;
pub use sampler::{Sampler, StartWatcher};
pub use tree::descendants;
pub use wtm_sys::disk::DiskUsage;
pub use wtm_sys::disktrace::{DiskTracer, FileIo};
pub use wtm_sys::ending::{EndReport, Holder, why_alive};
pub use wtm_sys::etw::{owned_name as trace_session_name, stop_orphans as stop_trace_orphans};

/// Prefixes of the trace sessions a window started as administrator runs
/// without the service. The service clears those a closed window left.
pub const GUI_DISK_TRACE: &str = "RustWTM-GUI-Disk";
pub const GUI_NET_TRACE: &str = "RustWTM-GUI-Network";
pub use wtm_sys::firewall;
pub use wtm_sys::icon::{IconImage, small_icon};
pub use wtm_sys::modules::{Module, Signature, process_modules, signature};
pub use wtm_sys::net::{Connection, Protocol, TcpState, close_tcp, connections};
pub use wtm_sys::services::{
    ServiceEntry, ServiceState, StartType, list_services, restart_service, set_start_type,
    start_service, stop_service,
};
pub use wtm_sys::sessions::{
    Session, SessionState, disconnect as disconnect_session, log_off as log_off_session, sessions,
};
pub use wtm_sys::system_ui_language;
pub use wtm_sys::volumes::{DriveKind, Volume, volumes};
pub use wtm_sys::waits::{
    Chain, HungWindow, Link, LinkStatus, WaitKind, WaitReport, hung_windows, wait_chains,
};
pub use wtm_sys::window::{self, foreground_fullscreen, foreground_pid};
pub use wtm_sys::{Error, Result};
