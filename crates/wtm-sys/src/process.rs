//! System-wide process enumeration.

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::STATUS_INFO_LENGTH_MISMATCH;

use crate::ntapi;
use crate::{Error, Result};

/// One process as reported by the kernel. Times are in 100 ns units.
#[derive(Debug, Clone)]
pub struct RawProcess {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    /// FILETIME (100 ns intervals since 1601-01-01 UTC). Zero for the idle process.
    pub create_time: i64,
    pub user_time: u64,
    pub kernel_time: u64,
    pub cycle_time: u64,
    pub base_priority: i32,
    pub session_id: u32,
    pub thread_count: u32,
    pub handle_count: u32,
    pub working_set: u64,
    pub private_working_set: u64,
    /// Private committed bytes.
    pub private_bytes: u64,
    pub virtual_size: u64,
    pub page_faults: u32,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub io_other_bytes: u64,
}

/// Takes process snapshots, reusing its buffer between calls.
/// One thread as the system snapshot reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadInfo {
    pub tid: u32,
    /// `KTHREAD_STATE`; 5 is Waiting.
    pub state: u32,
    /// `KWAIT_REASON` while waiting.
    pub wait_reason: u32,
    pub context_switches: u32,
}

impl ThreadInfo {
    /// Waiting on something that may lead to another thread or process,
    /// rather than sleeping, suspended or idle in a thread pool.
    pub fn blocked(&self) -> bool {
        // DelayExecution, Suspended, WrDelayExecution, WrSuspended, WrQueue.
        const IDLE: [u32; 5] = [4, 5, 11, 12, 15];
        self.state == 5 && !IDLE.contains(&self.wait_reason)
    }
}

#[derive(Debug, Default)]
pub struct ProcessSnapshotter {
    // u64 elements keep the buffer 8-byte aligned for the native structures.
    buf: Vec<u64>,
    /// Processes in the previous snapshot, to size the next one's vector.
    last_count: usize,
}

const INITIAL_BUFFER_BYTES: usize = 512 * 1024;
const GROWTH_SLACK_BYTES: usize = 64 * 1024;

impl ProcessSnapshotter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&mut self) -> Result<Vec<RawProcess>> {
        // Sized like the previous snapshot (plus a few) so it never regrows.
        let capacity = self.last_count + 32;
        self.collect(Vec::with_capacity(capacity), |_, _| true)
    }

    /// Like [`snapshot`](Self::snapshot), but converts (and names) only the
    /// processes `keep` accepts, given each one's PID and creation time.
    /// Polling for new processes this way skips the rest unconverted.
    pub fn snapshot_where(
        &mut self,
        keep: impl FnMut(u32, i64) -> bool,
    ) -> Result<Vec<RawProcess>> {
        self.collect(Vec::new(), keep)
    }

    fn collect(
        &mut self,
        mut out: Vec<RawProcess>,
        mut keep: impl FnMut(u32, i64) -> bool,
    ) -> Result<Vec<RawProcess>> {
        let len = self.query()?;
        let mut count = 0usize;
        for info in self.entries(len) {
            count += 1;
            if keep(info.unique_process_id as usize as u32, info.create_time) {
                out.push(to_raw(info));
            }
        }
        self.last_count = count;
        Ok(out)
    }

    /// Fills the buffer and returns the number of valid bytes.
    fn query(&mut self) -> Result<usize> {
        if self.buf.is_empty() {
            self.buf.resize(INITIAL_BUFFER_BYTES / 8, 0);
        }
        loop {
            let capacity = self.buf.len() * 8;
            let mut needed = 0u32;
            // SAFETY: the buffer is valid for `capacity` bytes and outlives the call.
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr().cast(),
                    u32::try_from(capacity).unwrap_or(u32::MAX),
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                let wanted = (needed as usize).max(capacity * 2) + GROWTH_SLACK_BYTES;
                self.buf.resize(wanted.div_ceil(8), 0);
                continue;
            }
            if status.is_err() {
                return Err(Error::Nt {
                    call: "NtQuerySystemInformation",
                    status: status.0 as u32,
                });
            }
            return Ok((needed as usize).min(capacity));
        }
    }

    /// The threads of `pid`, from a fresh system snapshot (the process
    /// itself is not opened). Empty when the process is gone.
    pub fn threads(&mut self, pid: u32) -> Result<Vec<ThreadInfo>> {
        let len = self.query()?;
        let base = self.buf.as_ptr().cast::<u8>();
        let entry_size = size_of::<ntapi::SystemProcessInformation>();
        let thread_size = size_of::<ntapi::SystemThreadInformation>();
        let Some((offset, info)) = self
            .entries_at(len)
            .find(|(_, info)| info.unique_process_id as usize as u32 == pid)
        else {
            return Ok(Vec::new());
        };
        let first = offset + entry_size;
        let count = (info.number_of_threads as usize).min(len.saturating_sub(first) / thread_size);
        Ok((0..count)
            .map(|i| {
                // SAFETY: `count` entries fit inside the valid bytes.
                let t = unsafe {
                    &*base
                        .add(first + i * thread_size)
                        .cast::<ntapi::SystemThreadInformation>()
                };
                ThreadInfo {
                    tid: t.unique_thread as usize as u32,
                    state: t.thread_state,
                    wait_reason: t.wait_reason,
                    context_switches: t.context_switches,
                }
            })
            .collect())
    }

    /// The process entries among the first `len` bytes of the buffer.
    fn entries(&self, len: usize) -> impl Iterator<Item = &ntapi::SystemProcessInformation> {
        self.entries_at(len).map(|(_, info)| info)
    }

    /// The process entries with their byte offsets.
    fn entries_at(
        &self,
        len: usize,
    ) -> impl Iterator<Item = (usize, &ntapi::SystemProcessInformation)> {
        let base = self.buf.as_ptr().cast::<u8>();
        let entry_size = size_of::<ntapi::SystemProcessInformation>();
        let len = len.min(self.buf.len() * 8);
        let mut next = Some(0usize);
        std::iter::from_fn(move || {
            let offset = next.filter(|&o| o + entry_size <= len)?;
            // SAFETY: bounds checked above; the kernel aligns every entry to
            // 8 bytes; the buffer is borrowed for the iterator's lifetime.
            let info = unsafe { &*base.add(offset).cast::<ntapi::SystemProcessInformation>() };
            next = (info.next_entry_offset != 0).then(|| offset + info.next_entry_offset as usize);
            Some((offset, info))
        })
    }
}

fn to_raw(info: &ntapi::SystemProcessInformation) -> RawProcess {
    let pid = info.unique_process_id as usize as u32;
    RawProcess {
        pid,
        parent_pid: info.inherited_from_unique_process_id as usize as u32,
        name: image_name(info, pid),
        create_time: info.create_time,
        user_time: info.user_time.max(0) as u64,
        kernel_time: info.kernel_time.max(0) as u64,
        cycle_time: info.cycle_time,
        base_priority: info.base_priority,
        session_id: info.session_id,
        thread_count: info.number_of_threads,
        handle_count: info.handle_count,
        working_set: info.working_set_size as u64,
        private_working_set: info.working_set_private_size.max(0) as u64,
        private_bytes: info.pagefile_usage as u64,
        virtual_size: info.virtual_size as u64,
        page_faults: info.page_fault_count,
        io_read_bytes: info.read_transfer_count.max(0) as u64,
        io_write_bytes: info.write_transfer_count.max(0) as u64,
        io_other_bytes: info.other_transfer_count.max(0) as u64,
    }
}

fn image_name(info: &ntapi::SystemProcessInformation, pid: u32) -> String {
    let name = &info.image_name;
    if name.buffer.is_null() || name.length == 0 {
        return if pid == 0 { "System Idle Process" } else { "" }.to_owned();
    }
    // SAFETY: the kernel points `buffer` into the same snapshot buffer, which
    // is borrowed for the lifetime of `info`; `length` is in bytes.
    let chars = unsafe { std::slice::from_raw_parts(name.buffer, name.length as usize / 2) };
    crate::utf16_lossy(chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_contains_current_and_system_process() {
        let procs = ProcessSnapshotter::new().snapshot().unwrap();
        let me = std::process::id();
        assert!(procs.iter().any(|p| p.pid == me && !p.name.is_empty()));
        assert!(procs.iter().any(|p| p.pid == 4 && p.name == "System"));
        assert!(procs.iter().any(|p| p.pid == 0));
    }

    #[test]
    fn snapshotter_is_reusable() {
        let mut s = ProcessSnapshotter::new();
        let a = s.snapshot().unwrap();
        let b = s.snapshot().unwrap();
        assert!(!a.is_empty() && !b.is_empty());
    }

    #[test]
    fn grows_a_too_small_buffer() {
        // Far smaller than one entry, so the kernel asks for more space.
        let mut s = ProcessSnapshotter {
            buf: vec![0; 1],
            ..Default::default()
        };
        let procs = s.snapshot().unwrap();
        assert!(s.buf.len() > 1, "buffer was not grown");
        assert!(procs.iter().any(|p| p.pid == std::process::id()));
    }

    #[test]
    fn snapshot_where_converts_only_accepted_processes() {
        let mut s = ProcessSnapshotter::new();
        let me = std::process::id();
        let mut seen = 0;
        let mine = s
            .snapshot_where(|pid, _| {
                seen += 1;
                pid == me
            })
            .unwrap();
        assert!(seen > 2, "every process is offered");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].pid, me);
        assert!(!mine[0].name.is_empty());
        let all = s.snapshot().unwrap();
        let full = all.iter().find(|p| p.pid == me).unwrap();
        assert_eq!(full.create_time, mine[0].create_time);
        assert_eq!(full.name, mine[0].name);
    }
}
