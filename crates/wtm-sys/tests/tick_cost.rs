//! What one sampling tick costs: time and heap allocations of each call the
//! samplers make every second. Ignored by default; run with
//! `cargo test -p wtm-sys --release --test tick_cost -- --ignored --nocapture`.

#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

/// Counts allocations made by the current thread.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator unchanged; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` `rounds` times after a warm-up; prints µs and allocations per call.
fn measure<T>(what: &str, rounds: u32, mut f: impl FnMut() -> T) {
    drop(f());
    let before = ALLOCATIONS.with(Cell::get);
    let start = Instant::now();
    for _ in 0..rounds {
        drop(std::hint::black_box(f()));
    }
    let micros = start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds);
    let allocs = (ALLOCATIONS.with(Cell::get) - before) as f64 / f64::from(rounds);
    println!("{what:<28} {micros:>10.1} us {allocs:>10.1} allocs");
}

#[test]
#[ignore = "timing harness"]
fn per_tick_calls() {
    use wtm_sys::{disk::DiskSampler, gpu::GpuSampler, net, process, system};

    let mut snap = process::ProcessSnapshotter::new();
    measure("process snapshot", 200, || snap.snapshot().unwrap());
    let me = std::process::id();
    measure("process snapshot (new only)", 200, || {
        snap.snapshot_where(|pid, _| pid == me).unwrap()
    });
    measure("cpu_times", 2000, || system::cpu_times().unwrap());
    measure("per_cpu_times", 2000, || system::per_cpu_times().unwrap());
    measure("memory_status", 2000, || system::memory_status().unwrap());
    measure("net_counters", 200, || system::net_counters().unwrap());
    measure("connections", 100, || net::connections().unwrap());
    if let Ok(mut gpu) = GpuSampler::new() {
        measure("gpu sample", 100, || gpu.sample().unwrap());
    }
    if let Ok(mut disks) = DiskSampler::new() {
        measure("disk sample", 200, || disks.sample().unwrap());
    }
    let h = wtm_sys::control::ProcessHandle::open(me, wtm_sys::control::Access::QUERY).unwrap();
    measure("image_path", 2000, || h.image_path().unwrap());
}
