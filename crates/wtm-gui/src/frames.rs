//! Measures how long the window spends per frame, so a slowdown shows up as
//! numbers rather than impressions.
//!
//! With `RUSTWTM_FRAME_REPORT=<file>` set, the window instead walks through
//! every page, repainting continuously, writes per-page timings to the file
//! and closes: a repeatable benchmark of the UI.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Frames kept for the live summary.
const KEPT: usize = 300;
/// Report mode: frames measured per page, after the warm-up ones.
const BENCH_FRAMES: usize = 120;
const BENCH_WARMUP: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Waiting for the data the background worker publishes.
    pub wait: Duration,
    /// The whole frame, wait included.
    pub total: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub average: Duration,
    pub p95: Duration,
    pub slowest: Duration,
    pub slowest_wait: Duration,
}

impl Summary {
    pub fn of(frames: &[Frame]) -> Option<Self> {
        if frames.is_empty() {
            return None;
        }
        let mut totals: Vec<_> = frames.iter().map(|f| f.total).collect();
        totals.sort_unstable();
        let sum: Duration = totals.iter().sum();
        Some(Self {
            frames: frames.len(),
            average: sum / frames.len() as u32,
            p95: totals[(totals.len() * 95).div_ceil(100) - 1],
            slowest: totals[totals.len() - 1],
            slowest_wait: frames.iter().map(|f| f.wait).max().unwrap_or_default(),
        })
    }
}

#[derive(Debug)]
struct Bench {
    path: PathBuf,
    page: usize,
    seen: usize,
    pages: Vec<Vec<Frame>>,
}

#[derive(Debug, Default)]
pub struct FrameStats {
    recent: VecDeque<Frame>,
    bench: Option<Bench>,
}

impl FrameStats {
    pub fn from_env() -> Self {
        Self {
            recent: VecDeque::new(),
            bench: std::env::var_os("RUSTWTM_FRAME_REPORT").map(|path| Bench {
                path: path.into(),
                page: 0,
                seen: 0,
                pages: Vec::new(),
            }),
        }
    }

    /// The page report mode wants shown now, as an index into the pages.
    pub fn bench_page(&self) -> Option<usize> {
        self.bench.as_ref().map(|b| b.page)
    }

    pub fn summary(&self) -> Option<Summary> {
        let frames: Vec<_> = self.recent.iter().copied().collect();
        Summary::of(&frames)
    }

    /// Records a frame that started at `start` and got its data at `ready`.
    /// In report mode, returns `true` once every page in `pages` was
    /// measured and the report written.
    pub fn record(&mut self, start: Instant, ready: Instant, pages: &[&str]) -> bool {
        let now = Instant::now();
        let frame = Frame {
            wait: ready.saturating_duration_since(start),
            total: now.saturating_duration_since(start),
        };
        self.recent.push_back(frame);
        while self.recent.len() > KEPT {
            self.recent.pop_front();
        }
        let Some(bench) = self.bench.as_mut() else {
            return false;
        };
        bench.seen += 1;
        if bench.seen <= BENCH_WARMUP {
            return false;
        }
        if bench.pages.len() <= bench.page {
            bench.pages.push(Vec::new());
        }
        bench.pages[bench.page].push(frame);
        if bench.pages[bench.page].len() < BENCH_FRAMES {
            return false;
        }
        bench.page += 1;
        bench.seen = 0;
        if bench.page < pages.len() {
            return false;
        }
        let _ = std::fs::write(&bench.path, report(pages, &bench.pages));
        self.bench = None;
        true
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn report(pages: &[&str], measured: &[Vec<Frame>]) -> String {
    let mut out = String::from("page\tframes\tavg_ms\tp95_ms\tmax_ms\tmax_wait_ms\n");
    for (name, frames) in pages.iter().zip(measured) {
        if let Some(s) = Summary::of(frames) {
            let _ = writeln!(
                out,
                "{name}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
                s.frames,
                ms(s.average),
                ms(s.p95),
                ms(s.slowest),
                ms(s.slowest_wait)
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(total_ms: u64, wait_ms: u64) -> Frame {
        Frame {
            wait: Duration::from_millis(wait_ms),
            total: Duration::from_millis(total_ms),
        }
    }

    #[test]
    fn summarizes_average_tail_and_worst() {
        assert_eq!(Summary::of(&[]), None);
        let frames: Vec<_> = (1..=100).map(|i| frame(i, i % 7)).collect();
        let s = Summary::of(&frames).unwrap();
        assert_eq!(s.frames, 100);
        assert_eq!(s.p95, Duration::from_millis(95));
        assert_eq!(s.slowest, Duration::from_millis(100));
        assert_eq!(s.slowest_wait, Duration::from_millis(6));
        assert_eq!(s.average.as_micros(), 50_500);
    }

    #[test]
    fn live_summary_keeps_only_recent_frames() {
        let mut stats = FrameStats::default();
        let start = Instant::now();
        for _ in 0..KEPT + 10 {
            assert!(!stats.record(start, start, &["x"]));
        }
        assert_eq!(stats.summary().unwrap().frames, KEPT);
        assert_eq!(stats.bench_page(), None);
    }

    #[test]
    fn report_mode_walks_every_page_then_writes_the_file() {
        let path = std::env::temp_dir().join(format!("wtm-frames-{}.tsv", std::process::id()));
        let mut stats = FrameStats {
            recent: VecDeque::new(),
            bench: Some(Bench {
                path: path.clone(),
                page: 0,
                seen: 0,
                pages: Vec::new(),
            }),
        };
        let pages = ["Overview", "Processes"];
        let start = Instant::now();
        let mut done = false;
        let mut frames = 0;
        while !done {
            assert!(stats.bench_page().unwrap() < pages.len());
            done = stats.record(start, start, &pages);
            frames += 1;
        }
        assert_eq!(frames, 2 * (BENCH_WARMUP + BENCH_FRAMES));
        assert_eq!(stats.bench_page(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[1].starts_with("Overview\t120\t"), "{text}");
        assert!(lines[2].starts_with("Processes\t120\t"), "{text}");
    }
}
