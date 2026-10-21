//! `wtm kill-list` and `wtm panic`: the service's kill list from the command
//! line, and ending everything on it at once.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Subcommand;
use wtm_core::ProcessInfo;
use wtm_core::control::ProcessRef;
use wtm_ipc::{Client, Request, Response};
use wtm_policy::{Config, KillEntry, panic_targets, remove_kill_entry, set_kill_entry};

#[derive(Subcommand)]
pub enum KillAction {
    /// Show the kill list
    List,
    /// Add a program, e.g. `node.exe --cmdline vitest --cpu 50 --secs 60`
    Add {
        /// Image-name wildcard, e.g. `node.exe`
        name: String,
        /// Only processes whose command line contains this text (or matches
        /// it, with `*` / `?`)
        #[arg(long)]
        cmdline: Option<String>,
        /// End it automatically at this share of the whole machine's CPU (one
        /// busy core on 8 logical processors is 12.5%); without it only
        /// `wtm panic` ends it
        #[arg(long, value_parser = crate::parse::percent, value_name = "PERCENT")]
        cpu: Option<f32>,
        /// How long it must keep that up
        #[arg(long, default_value_t = 60, value_name = "SECONDS")]
        secs: u64,
        /// Only while the whole PC is at least this busy
        #[arg(long, value_parser = crate::parse::percent, value_name = "PERCENT")]
        when_pc_above: Option<f32>,
        /// Leave the processes it started alone
        #[arg(long)]
        no_tree: bool,
    },
    /// Remove entry number N (see `wtm kill-list list`)
    Remove { number: usize },
}

pub fn fetch(client: &mut Client) -> anyhow::Result<String> {
    match client.call(&Request::GetConfig)? {
        Response::Config { toml, .. } => Ok(toml),
        Response::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
}

fn store(client: &mut Client, toml: String) -> anyhow::Result<()> {
    match client.call(&Request::SetConfig { toml })? {
        Response::Done => Ok(()),
        Response::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
}

fn connect(client: impl FnOnce() -> io::Result<Client>) -> anyhow::Result<Client> {
    client().map_err(|e| anyhow::anyhow!("RustWTM service not reachable ({e})"))
}

pub fn kill_list(
    action: KillAction,
    client: impl FnOnce() -> io::Result<Client>,
    out: &mut impl Write,
) -> anyhow::Result<ExitCode> {
    let mut client = connect(client)?;
    let text = fetch(&mut client)?;
    match action {
        KillAction::List => {
            let config = Config::from_toml(&text)?;
            if config.kill.is_empty() {
                writeln!(
                    out,
                    "The kill list is empty. Add one with `wtm kill-list add <name> [--cmdline TEXT] [--cpu N]`."
                )?;
            }
            for (i, e) in config.kill.iter().enumerate() {
                writeln!(
                    out,
                    "{:>3}  {:<40} {}{}",
                    i + 1,
                    e.label(),
                    e.describe(),
                    if e.tree { ", with its children" } else { "" }
                )?;
            }
            if config.dry_run && config.kill.iter().any(|e| e.cpu_above.is_some()) {
                writeln!(
                    out,
                    "\nDry-run mode: automatic ends are only logged; `wtm panic` always works."
                )?;
            }
        }
        KillAction::Add {
            name,
            cmdline,
            cpu,
            secs,
            when_pc_above,
            no_tree,
        } => {
            let entry = KillEntry {
                name,
                cmdline,
                cpu_above: cpu,
                for_secs: secs,
                system_cpu_above: when_pc_above,
                tree: !no_tree,
            };
            let new = set_kill_entry(&text, &entry, None).map_err(anyhow::Error::msg)?;
            store(&mut client, new)?;
            writeln!(
                out,
                "Added {} to the kill list: {}.",
                entry.label(),
                entry.describe()
            )?;
        }
        KillAction::Remove { number } => {
            let count = Config::from_toml(&text)?.kill.len();
            if number == 0 || number > count {
                anyhow::bail!("there is no entry {number} (the kill list has {count})");
            }
            let new = remove_kill_entry(&text, number - 1).map_err(anyhow::Error::msg)?;
            store(&mut client, new)?;
            writeln!(out, "Entry {number} removed from the kill list.")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Lists what is on the kill list and running; ends it all with `yes`.
/// `end` ends one process (through the service when it can).
pub fn panic(
    yes: bool,
    entries: &[KillEntry],
    procs: &[ProcessInfo],
    command_line: &mut dyn FnMut(&ProcessInfo) -> Option<String>,
    end: &mut dyn FnMut(&ProcessRef) -> Result<(), String>,
    out: &mut impl Write,
) -> anyhow::Result<ExitCode> {
    let targets = panic_targets(entries, procs, command_line);
    if targets.is_empty() {
        writeln!(out, "Nothing on the kill list is running.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut failed = 0;
    for t in &targets {
        let p = t.process;
        let name = if t.child {
            format!("└ {}", p.name)
        } else {
            p.name.clone()
        };
        let result = if yes {
            match end(&ProcessRef::from(p)) {
                Ok(()) => "ended".to_owned(),
                Err(e) => {
                    failed += 1;
                    format!("failed: {e}")
                }
            }
        } else {
            "would be ended".to_owned()
        };
        writeln!(
            out,
            "{:>7}  {:<28} {:>6.1}%  {:<36} {result}",
            p.pid,
            name,
            p.cpu_percent,
            entries[t.entry].label()
        )?;
    }
    if !yes {
        writeln!(
            out,
            "\n{} processes. Run `wtm panic --yes` to end them now.",
            targets.len()
        )?;
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{with_fake_service, with_replies};
    use std::time::Duration;

    fn reply(toml: &str) -> Response {
        Response::Config {
            toml: toml.into(),
            path: "c.toml".into(),
        }
    }

    fn run(action: KillAction, replies: Vec<Response>) -> (Vec<Request>, anyhow::Result<String>) {
        let mut out = Vec::new();
        let (requests, result) = with_replies(replies, |c| kill_list(action, || c, &mut out));
        (requests, result.map(|_| String::from_utf8(out).unwrap()))
    }

    const TEXT: &str = "dry_run = true\n[[kill]]\nname = \"node.exe\"\ncmdline = \"vitest\"\ncpu_above = 50.0\nfor_secs = 60\n";

    #[test]
    fn lists_adds_and_removes_entries() {
        with_fake_service(|| {
            let (_, out) = run(KillAction::List, vec![reply(TEXT)]);
            let out = out.unwrap();
            assert!(out.contains("  1  node.exe running \"vitest\""), "{out}");
            assert!(
                out.contains("with its children") && out.contains("Dry-run"),
                "{out}"
            );
            let (_, out) = run(KillAction::List, vec![reply("")]);
            assert!(out.unwrap().starts_with("The kill list is empty"));

            let add = KillAction::Add {
                name: "python.exe".into(),
                cmdline: None,
                cpu: None,
                secs: 60,
                when_pc_above: None,
                no_tree: true,
            };
            let (requests, out) = run(add, vec![reply(TEXT), Response::Done]);
            assert_eq!(
                out.unwrap(),
                "Added python.exe to the kill list: only with the panic button.\n"
            );
            let Request::SetConfig { toml } = &requests[1] else {
                panic!("{requests:?}");
            };
            let kill = Config::from_toml(toml).unwrap().kill;
            assert_eq!(kill.len(), 2);
            assert!(!kill[1].tree);

            let (requests, out) = run(
                KillAction::Remove { number: 1 },
                vec![reply(TEXT), Response::Done],
            );
            assert_eq!(out.unwrap(), "Entry 1 removed from the kill list.\n");
            assert!(matches!(&requests[1], Request::SetConfig { toml } if !toml.contains("kill")));
            let (_, out) = run(KillAction::Remove { number: 2 }, vec![reply(TEXT)]);
            assert_eq!(
                out.unwrap_err().to_string(),
                "there is no entry 2 (the kill list has 1)"
            );
            let bad = KillAction::Add {
                name: " ".into(),
                cmdline: None,
                cpu: None,
                secs: 60,
                when_pc_above: None,
                no_tree: false,
            };
            let (_, out) = run(bad, vec![reply(TEXT)]);
            assert_eq!(out.unwrap_err().to_string(), "name must not be empty");
            let (_, out) = run(KillAction::List, vec![Response::Error("denied".into())]);
            assert_eq!(out.unwrap_err().to_string(), "denied");
            let (_, out) = run(KillAction::List, vec![Response::Done]);
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));
            let add = || KillAction::Add {
                name: "a.exe".into(),
                cmdline: None,
                cpu: None,
                secs: 1,
                when_pc_above: None,
                no_tree: false,
            };
            let (_, out) = run(
                add(),
                vec![reply(TEXT), Response::Error("not admin".into())],
            );
            assert_eq!(out.unwrap_err().to_string(), "not admin");
            let (_, out) = run(add(), vec![reply(TEXT), Response::Events(Vec::new())]);
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));
        });
        let mut out = Vec::new();
        let e = kill_list(
            KillAction::List,
            || Err(io::Error::other("no pipe")),
            &mut out,
        )
        .unwrap_err();
        assert_eq!(e.to_string(), "RustWTM service not reachable (no pipe)");
    }

    fn proc(pid: u32, parent: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            name: name.into(),
            cpu_percent: 40.0,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 0,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            suspended: false,
            create_time: i64::from(pid),
        }
    }

    #[test]
    fn panic_previews_then_ends_matching_processes() {
        let entries = Config::from_toml(TEXT).unwrap().kill;
        let procs = [
            proc(10, 1, "node.exe"),
            proc(11, 10, "esbuild.exe"),
            proc(20, 1, "node.exe"),
        ];
        let mut lines = |p: &ProcessInfo| {
            Some(
                if p.pid == 10 {
                    "node vitest run"
                } else {
                    "node app.js"
                }
                .into(),
            )
        };
        let mut ended = Vec::new();
        let mut out = Vec::new();
        panic(
            false,
            &entries,
            &procs,
            &mut lines,
            &mut |t| {
                ended.push(t.pid);
                Ok(())
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(ended.is_empty(), "a preview ends nothing");
        assert!(
            text.contains("node.exe") && text.contains("└ esbuild.exe"),
            "{text}"
        );
        assert!(
            text.contains("2 processes. Run `wtm panic --yes`"),
            "{text}"
        );

        let mut out = Vec::new();
        let code = panic(
            true,
            &entries,
            &procs,
            &mut lines,
            &mut |t| {
                ended.push(t.pid);
                if t.pid == 11 {
                    Err("access denied".into())
                } else {
                    Ok(())
                }
            },
            &mut out,
        )
        .unwrap();
        assert_eq!(ended, [10, 11]);
        assert_eq!(code, ExitCode::FAILURE);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ended") && text.contains("failed: access denied"),
            "{text}"
        );

        let mut out = Vec::new();
        panic(
            true,
            &entries,
            &procs[2..],
            &mut lines,
            &mut |_| Ok(()),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Nothing on the kill list is running.\n"
        );
    }
}
