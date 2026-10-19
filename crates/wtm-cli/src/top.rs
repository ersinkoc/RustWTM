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

pub fn run(mut sort: SortKey, interval: Duration) -> anyhow::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut sampler = Sampler::new();
    let mut snap: Option<Snapshot> = None;
    let mut next_sample = Instant::now();

    loop {
        if Instant::now() >= next_sample {
            let mut s = sampler.sample()?;
            sort_processes(&mut s.processes, sort);
            draw(&s, sort)?;
            snap = Some(s);
            next_sample = Instant::now() + interval;
        }

        let timeout = next_sample.saturating_duration_since(Instant::now());
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
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
                if let Some(s) = snap.as_mut() {
                    sort_processes(&mut s.processes, sort);
                    draw(s, sort)?;
                }
            }
            Event::Resize(..) => {
                if let Some(s) = snap.as_ref() {
                    draw(s, sort)?;
                }
            }
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
    use wtm_core::Sampler;

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
