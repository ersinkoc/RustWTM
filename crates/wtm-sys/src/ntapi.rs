//! Native structures not (fully) exposed by the `windows` crate.
//!
//! Layouts follow the phnt headers. The documented `SYSTEM_PROCESS_INFORMATION`
//! hides CPU times and I/O counters behind reserved fields, so we define the
//! full layout ourselves.

use std::ffi::c_void;

#[repr(C)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *const u16,
}

#[repr(C)]
pub struct SystemProcessInformation {
    pub next_entry_offset: u32,
    pub number_of_threads: u32,
    pub working_set_private_size: i64,
    pub hard_fault_count: u32,
    pub number_of_threads_high_watermark: u32,
    pub cycle_time: u64,
    pub create_time: i64,
    pub user_time: i64,
    pub kernel_time: i64,
    pub image_name: UnicodeString,
    pub base_priority: i32,
    pub unique_process_id: *mut c_void,
    pub inherited_from_unique_process_id: *mut c_void,
    pub handle_count: u32,
    pub session_id: u32,
    pub unique_process_key: usize,
    pub peak_virtual_size: usize,
    pub virtual_size: usize,
    pub page_fault_count: u32,
    pub peak_working_set_size: usize,
    pub working_set_size: usize,
    pub quota_peak_paged_pool_usage: usize,
    pub quota_paged_pool_usage: usize,
    pub quota_peak_non_paged_pool_usage: usize,
    pub quota_non_paged_pool_usage: usize,
    pub pagefile_usage: usize,
    pub peak_pagefile_usage: usize,
    pub private_page_count: usize,
    pub read_operation_count: i64,
    pub write_operation_count: i64,
    pub other_operation_count: i64,
    pub read_transfer_count: i64,
    pub write_transfer_count: i64,
    pub other_transfer_count: i64,
    // Followed by `number_of_threads` SYSTEM_THREAD_INFORMATION entries.
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<SystemProcessInformation>() == 0x100);

/// `SYSTEM_THREAD_INFORMATION`, after each process entry.
#[repr(C)]
pub struct SystemThreadInformation {
    pub kernel_time: i64,
    pub user_time: i64,
    pub create_time: i64,
    pub wait_time: u32,
    pub start_address: *mut c_void,
    pub unique_process: *mut c_void,
    pub unique_thread: *mut c_void,
    pub priority: i32,
    pub base_priority: i32,
    pub context_switches: u32,
    pub thread_state: u32,
    pub wait_reason: u32,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<SystemThreadInformation>() == 0x50);
