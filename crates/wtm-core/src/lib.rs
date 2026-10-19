//! Process model, sampling and control for RustWTM.

#![forbid(unsafe_code)]

pub mod control;
mod model;
mod protection;
mod sampler;
pub mod startup;
mod tree;

pub use model::{ProcessInfo, Snapshot, SortKey, SystemStats, format_bytes, sort_processes};
pub use protection::protection_reason;
pub use sampler::{Sampler, StartWatcher};
pub use tree::descendants;
pub use wtm_sys::net::{Connection, Protocol, TcpState, connections};
pub use wtm_sys::services::{
    ServiceEntry, ServiceState, StartType, list_services, set_start_type, start_service,
    stop_service,
};
pub use wtm_sys::sessions::{Session, SessionState, sessions};
pub use wtm_sys::{Error, Result};
