use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, queue};
use wtm_core::{Sampler, Snapshot, SortKey, sort_processes};

use crate::table;

/// Restores the terminal even if rendering fails or panics.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run(sort: SortKey, interval: Duration) -> anyhow::Result<()> {
    let _guard = TerminalGuard::enter()?;
    event_loop(
        sort,
        interval,
        |timeout| {
            Ok(if event::poll(timeout)? {
                Some(event::read()?)
            } else {
                None
            })
        },
        draw,
    )
}

/// The interactive loop, independent of the real terminal: `next_event`
/// waits up to the given time for input, `draw` renders a frame.
fn event_loop(
    mut sort: SortKey,
    interval: Duration,
    mut next_event: impl FnMut(Duration) -> io::Result<Option<Event>>,
    mut draw: impl FnMut(&Snapshot, SortKey) -> io::Result<()>,
) -> anyhow::Result<()> {
    let mut sampler = Sampler::new();
    let mut sampled = |sort: SortKey| -> anyhow::Result<Snapshot> {
        let mut s = sampler.sample()?;
        sort_processes(&mut s.processes, sort);
        Ok(s)
    };
    let mut snap = sampled(sort)?;
    draw(&snap, sort)?;
    let mut next_sample = Instant::now() + interval;

    loop {
        if Instant::now() >= next_sample {
            snap = sampled(sort)?;
            draw(&snap, sort)?;
            next_sample = Instant::now() + interval;
        }

        let timeout = next_sample.saturating_duration_since(Instant::now());
        let Some(event) = next_event(timeout)? else {
            continue;
        };
        match event {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                let new_sort = match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Char('c') => SortKey::Cpu,
                    KeyCode::Char('m') => SortKey::Memory,
                    KeyCode::Char('d') => SortKey::Disk,
                    KeyCode::Char('p') => SortKey::Pid,
                    KeyCode::Char('n') => SortKey::Name,
                    _ => continue,
                };
                sort = new_sort;
                sort_processes(&mut snap.processes, sort);
                draw(&snap, sort)?;
            }
            Event::Resize(..) => draw(&snap, sort)?,
            _ => {}
        }
    }
}

fn draw(snap: &Snapshot, sort: SortKey) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    render(
        &mut io::stdout().lock(),
        snap,
        sort,
        cols as usize,
        rows as usize,
    )
}

/// Draws one frame of `cols` x `rows` characters to `out`.
fn render(
    out: &mut impl Write,
    snap: &Snapshot,
    sort: SortKey,
    width: usize,
    rows: usize,
) -> io::Result<()> {
    queue!(out, cursor::MoveTo(0, 0))?;
    line(out, &table::summary(&snap.system), width)?;
    line(
        out,
        &format!("Sort: {sort:?}   [c]pu [m]emory [d]isk [p]id [n]ame   [q]uit"),
        width,
    )?;
    queue!(out, SetAttribute(Attribute::Reverse))?;
    line(out, &format!("{:<width$}", table::header()), width)?;
    queue!(out, SetAttribute(Attribute::Reset))?;

    let body_rows = rows.saturating_sub(3);
    for p in snap.processes.iter().take(body_rows) {
        line(out, &table::row(p, width), width)?;
    }
    queue!(out, Clear(ClearType::FromCursorDown))?;
    out.flush()
}

fn line(out: &mut impl Write, text: &str, width: usize) -> io::Result<()> {
    let clipped: String = text.chars().take(width).collect();
    queue!(
        out,
        Print(clipped),
        Clear(ClearType::UntilNewLine),
        cursor::MoveToNextLine(1)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FailingWriter;
    use wtm_core::Sampler;

    use crossterm::event::KeyEvent;

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn run_with(events: Vec<Option<Event>>) -> Vec<SortKey> {
        let mut events = events.into_iter();
        let mut frames = Vec::new();
        event_loop(
            SortKey::Cpu,
            Duration::from_secs(3600),
            |_| Ok(events.next().expect("loop must stop before input runs out")),
            |_, sort| {
                frames.push(sort);
                Ok(())
            },
        )
        .unwrap();
        frames
    }

    #[test]
    fn keys_change_sort_and_quit() {
        let frames = run_with(vec![
            None,
            Some(key('m')),
            Some(key('d')),
            Some(key('p')),
            Some(key('n')),
            Some(key('c')),
            Some(key('x')), // ignored
            Some(Event::Resize(80, 25)),
            Some(Event::FocusGained), // ignored
            Some(key('q')),
        ]);
        assert_eq!(
            frames,
            [
                SortKey::Cpu,
                SortKey::Memory,
                SortKey::Disk,
                SortKey::Pid,
                SortKey::Name,
                SortKey::Cpu,
                SortKey::Cpu, // resize redraw
            ]
        );
        let ctrl_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(run_with(vec![Some(ctrl_c)]).len(), 1);
        let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(run_with(vec![Some(esc)]).len(), 1);
    }

    /// Runs the loop with `events`; `draw` fails on frame number `fail_at`.
    fn run_failing(events: Vec<io::Result<Option<Event>>>, fail_at: usize) -> anyhow::Result<()> {
        let mut events = events.into_iter();
        let mut frames = 0;
        event_loop(
            SortKey::Cpu,
            Duration::from_secs(3600),
            |_| events.next().expect("loop must stop before input runs out"),
            |_, _| {
                frames += 1;
                if frames == fail_at {
                    Err(io::Error::other("draw failed"))
                } else {
                    Ok(())
                }
            },
        )
    }

    #[test]
    fn errors_end_the_loop() {
        let failed = |r: anyhow::Result<()>, what: &str| {
            assert_eq!(r.unwrap_err().to_string(), what);
        };
        failed(run_failing(vec![], 1), "draw failed");
        failed(
            run_failing(vec![Err(io::Error::other("input failed"))], 0),
            "input failed",
        );
        failed(run_failing(vec![Ok(Some(key('m')))], 2), "draw failed");
        failed(
            run_failing(vec![Ok(Some(Event::Resize(80, 25)))], 2),
            "draw failed",
        );
        // A key without a press (e.g. a release) does nothing.
        let mut release = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let r = run_failing(vec![Ok(Some(Event::Key(release))), Ok(Some(key('q')))], 2);
        assert!(r.is_ok());
    }

    #[test]
    fn resamples_every_interval() {
        let mut events = vec![None, None, Some(key('q'))].into_iter();
        let mut frames = 0;
        event_loop(
            SortKey::Pid,
            Duration::ZERO,
            |timeout| {
                assert_eq!(timeout, Duration::ZERO);
                Ok(events.next().unwrap())
            },
            |snap, sort| {
                assert_eq!(sort, SortKey::Pid);
                assert!(snap.processes.is_sorted_by_key(|p| p.pid));
                frames += 1;
                Ok(())
            },
        )
        .unwrap();
        // The first frame plus one per loop turn.
        assert_eq!(frames, 4);
    }

    #[test]
    fn render_reports_write_errors() {
        let snap = Sampler::new().sample().unwrap();
        // Fail at every possible point of the frame until one fits.
        let mut budget = 0;
        while render(&mut FailingWriter { budget }, &snap, SortKey::Cpu, 80, 6).is_err() {
            budget += 1;
            assert!(budget < 10_000, "render never succeeded");
        }
        assert!(budget > 10, "a frame takes many writes, got {budget}");
    }

    #[test]
    fn renders_header_rows_and_clips_to_size() {
        let mut snap = Sampler::new().sample().unwrap();
        sort_processes(&mut snap.processes, SortKey::Memory);
        let mut buf = Vec::new();
        render(&mut buf, &snap, SortKey::Memory, 80, 10).unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("Sort: Memory"));
        assert!(text.contains("PID"));
        // 3 header lines + 7 body lines, each ends with a line move.
        assert_eq!(text.matches("\u{1b}[1E").count(), 10, "{text:?}");
        // Nothing wider than the terminal.
        for chunk in text.split('\u{1b}') {
            assert!(chunk.chars().count() <= 80 + 8, "{chunk:?}");
        }
    }
}
