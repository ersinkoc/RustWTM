//! `wtm config`: show, check and replace the service's configuration.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use wtm_ipc::{Client, Request, Response};
use wtm_policy::{
    AppLimit, Config, DEFAULT_CONFIG_TOML, LimitScope, remove_app_limit, set_app_limit,
};

#[derive(Subcommand)]
pub enum Action {
    /// Print the service's configuration file
    Show,
    /// Print the default configuration (with comments)
    Default,
    /// Validate a configuration file without applying it
    Check { file: PathBuf },
    /// Validate a file and make it the service's configuration (elevated)
    Apply { file: PathBuf },
}

#[derive(Subcommand)]
pub enum LimitAction {
    /// Show the configured limits and their current usage
    List,
    /// Add or change the limit of an app, e.g. `node.exe --cores 2`
    Set {
        /// Image-name wildcard, e.g. `node.exe` or `chrome*.exe`
        name: String,
        /// CPU budget in cores, e.g. `2` or `0.5`
        #[arg(long)]
        cores: Option<f32>,
        /// Memory budget, e.g. `8G`, `512M`
        #[arg(long, value_parser = crate::parse::size)]
        memory: Option<u64>,
        /// A budget per top-level instance instead of one for all processes
        #[arg(long)]
        each: bool,
    },
    /// Remove the limit of an app
    Remove {
        name: String,
        /// The per-instance limit (`--each`) instead of the shared one
        #[arg(long)]
        each: bool,
    },
}

fn scope(each: bool) -> LimitScope {
    if each {
        LimitScope::Each
    } else {
        LimitScope::Group
    }
}

/// Reads the service's configuration text.
fn fetch(client: &mut Client) -> anyhow::Result<String> {
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

pub fn app_limit(
    action: LimitAction,
    client: impl FnOnce() -> io::Result<Client>,
    out: &mut impl Write,
) -> anyhow::Result<ExitCode> {
    let mut client = connect(client)?;
    let text = fetch(&mut client)?;
    match action {
        LimitAction::List => {
            let config = Config::from_toml(&text)?;
            let usage = match client.call(&Request::Status)? {
                Response::Status(s) => s.app_limits,
                _ => Vec::new(),
            };
            if config.limits.is_empty() {
                writeln!(
                    out,
                    "No app limits. Add one with `wtm app-limit set <name> --cores N --memory SIZE`."
                )?;
            } else {
                writeln!(
                    out,
                    "{:<24} {:>8} {:>10} {:<10} {:>6} {:>10} {:>10}",
                    "APP", "CORES", "MEMORY", "SCOPE", "PROCS", "CPU NOW", "MEM NOW"
                )?;
            }
            for l in &config.limits {
                let groups = usage.iter().filter(|u| u.group.is_for(l));
                let (procs, cpu, mem) = groups.fold((0, 0.0, 0u64), |(n, c, m), g| {
                    (n + g.processes, c + g.cpu_percent, m + g.memory_bytes)
                });
                writeln!(
                    out,
                    "{:<24} {:>8} {:>10} {:<10} {:>6} {:>9.1}% {:>10}",
                    l.name,
                    l.cpu_cores.map_or("-".to_owned(), |c| c.to_string()),
                    l.memory_gb.map_or("-".to_owned(), |m| format!("{m} GB")),
                    if l.scope == LimitScope::Each {
                        "each"
                    } else {
                        "group"
                    },
                    procs,
                    cpu,
                    crate::table::bytes(mem as f64)
                )?;
            }
            if config.dry_run && !config.limits.is_empty() {
                writeln!(
                    out,
                    "\nDry-run mode: limits are only previewed (set dry_run = false to enforce)."
                )?;
            }
        }
        LimitAction::Set {
            name,
            cores,
            memory,
            each,
        } => {
            let limit = AppLimit {
                name,
                cpu_cores: cores,
                memory_gb: memory.map(|b| (b as f64 / f64::from(1u32 << 30)) as f32),
                scope: scope(each),
            };
            let new = set_app_limit(&text, &limit).map_err(anyhow::Error::msg)?;
            store(&mut client, new)?;
            writeln!(
                out,
                "Limit for {} saved; the service applies it within a second.",
                limit.name
            )?;
        }
        LimitAction::Remove { name, each } => {
            let new = remove_app_limit(&text, &name, scope(each)).map_err(anyhow::Error::msg)?;
            if new == text {
                anyhow::bail!(
                    "{name} has no {} limit",
                    if each { "per-instance" } else { "shared" }
                );
            }
            store(&mut client, new)?;
            writeln!(out, "Limit for {name} removed.")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn read_valid(file: &Path) -> anyhow::Result<(String, Config)> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", file.display()))?;
    let config =
        Config::from_toml(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
    Ok((text, config))
}

fn connect(client: impl FnOnce() -> io::Result<Client>) -> anyhow::Result<Client> {
    client().map_err(|e| anyhow::anyhow!("RustWTM service not reachable ({e})"))
}

/// `client` is only called by the actions that need the service.
pub fn run(
    action: Action,
    client: impl FnOnce() -> io::Result<Client>,
    out: &mut impl Write,
) -> anyhow::Result<ExitCode> {
    match action {
        Action::Show => match connect(client)?.call(&Request::GetConfig)? {
            Response::Config { toml, .. } => write!(out, "{toml}")?,
            Response::Error(e) => anyhow::bail!(e),
            other => anyhow::bail!("unexpected response: {other:?}"),
        },
        Action::Default => write!(out, "{DEFAULT_CONFIG_TOML}")?,
        Action::Check { file } => {
            let (_, c) = read_valid(&file)?;
            writeln!(
                out,
                "{}: valid (dry_run={}, governor={}, rules={})",
                file.display(),
                c.dry_run,
                if c.governor.enabled { "on" } else { "off" },
                c.rules.len()
            )?;
        }
        Action::Apply { file } => {
            let (toml, c) = read_valid(&file)?;
            match connect(client)?.call(&Request::SetConfig { toml })? {
                Response::Done => writeln!(
                    out,
                    "Configuration applied (dry_run={}, rules={}); the service reloads it within a second.",
                    c.dry_run,
                    c.rules.len()
                )?,
                Response::Error(e) => anyhow::bail!(e),
                other => anyhow::bail!("unexpected response: {other:?}"),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FailingWriter, with_fake_service, with_reply};

    fn temp_file(name: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("wtm-cli-{name}-{}.toml", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn text(action: Action, reply: Option<Response>) -> (Option<Request>, anyhow::Result<String>) {
        let mut out = Vec::new();
        let (request, result) = match reply {
            Some(reply) => with_reply(reply, |c| run(action, || c, &mut out)),
            None => (
                None,
                run(action, || unreachable!("no service needed"), &mut out),
            ),
        };
        (request, result.map(|_| String::from_utf8(out).unwrap()))
    }

    #[test]
    fn default_and_check_work_offline() {
        let (_, out) = text(Action::Default, None);
        assert_eq!(out.unwrap(), DEFAULT_CONFIG_TOML);

        let good = temp_file("good", DEFAULT_CONFIG_TOML);
        let (_, out) = text(Action::Check { file: good.clone() }, None);
        assert!(
            out.unwrap()
                .ends_with("valid (dry_run=true, governor=on, rules=0)\n")
        );
        let bad = temp_file("bad", "dry_run = 3");
        let (_, out) = text(Action::Check { file: bad.clone() }, None);
        assert!(out.unwrap_err().to_string().contains("bad"));
        let missing = PathBuf::from(r"C:\definitely\missing.toml");
        let (_, out) = text(Action::Check { file: missing }, None);
        assert!(out.unwrap_err().to_string().starts_with("reading"));
        // Invalid files are never sent to the service.
        let (_, out) = text(Action::Apply { file: bad.clone() }, None);
        assert!(out.is_err());
        for f in [good, bad] {
            std::fs::remove_file(f).unwrap();
        }
    }

    #[test]
    fn show_and_apply_talk_to_the_service() {
        with_fake_service(|| {
            let reply = Response::Config {
                toml: "dry_run = true\n".into(),
                path: "c.toml".into(),
            };
            let (request, out) = text(Action::Show, Some(reply));
            assert_eq!(request, Some(Request::GetConfig));
            assert_eq!(out.unwrap(), "dry_run = true\n");
            let (_, out) = text(Action::Show, Some(Response::Error("denied".into())));
            assert_eq!(out.unwrap_err().to_string(), "denied");
            let (_, out) = text(Action::Show, Some(Response::Done));
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));

            let file = temp_file("apply", DEFAULT_CONFIG_TOML);
            let apply = || Action::Apply { file: file.clone() };
            let (request, out) = text(apply(), Some(Response::Done));
            assert_eq!(
                request,
                Some(Request::SetConfig {
                    toml: DEFAULT_CONFIG_TOML.into()
                })
            );
            assert!(
                out.unwrap()
                    .starts_with("Configuration applied (dry_run=true")
            );
            let refused = Response::Error("administrator rights required".into());
            let (_, out) = text(apply(), Some(refused));
            assert!(out.unwrap_err().to_string().starts_with("administrator"));
            let (_, out) = text(apply(), Some(Response::Events(Vec::new())));
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));

            // Output errors are reported.
            let (_, result) = with_reply(Response::Done, |c| {
                run(apply(), || c, &mut FailingWriter { budget: 0 })
            });
            assert!(result.is_err());
            std::fs::remove_file(&file).unwrap();
        });
    }

    #[test]
    fn unreachable_service_is_reported() {
        let refused = || Err(io::Error::new(io::ErrorKind::NotFound, "no pipe"));
        let e = run(Action::Show, refused, &mut Vec::new()).unwrap_err();
        assert_eq!(e.to_string(), "RustWTM service not reachable (no pipe)");
    }

    fn limit_text(
        action: LimitAction,
        replies: Vec<Response>,
    ) -> (Vec<Request>, anyhow::Result<String>) {
        let mut out = Vec::new();
        let (requests, result) =
            crate::tests::with_replies(replies, |c| app_limit(action, || c, &mut out));
        (requests, result.map(|_| String::from_utf8(out).unwrap()))
    }

    fn config_reply(toml: &str) -> Response {
        Response::Config {
            toml: toml.into(),
            path: "c.toml".into(),
        }
    }

    #[test]
    fn app_limits_are_listed_set_and_removed() {
        with_fake_service(|| {
            let text = "dry_run = true\n[[limit]]\nname = \"zen.exe\"\nmemory_gb = 8.0\nscope = \"each\"\n";
            let usage = wtm_policy::GroupUsage {
                group: wtm_policy::GroupId {
                    app: "zen.exe".into(),
                    scope: LimitScope::Each,
                    root: Some((1, 1)),
                },
                limit: Config::from_toml(text).unwrap().limits[0].clone(),
                processes: 3,
                members: Vec::new(),
                outside: 0,
                cpu_percent: 4.0,
                memory_bytes: 2 << 30,
            };
            let status = wtm_ipc::Status {
                app_limits: vec![usage],
                ..Default::default()
            };
            let (requests, out) = limit_text(
                LimitAction::List,
                vec![config_reply(text), Response::Status(status)],
            );
            assert_eq!(requests, [Request::GetConfig, Request::Status]);
            let out = out.unwrap();
            assert!(out.starts_with("APP"), "{out}");
            assert!(
                out.contains("zen.exe") && out.contains("8 GB") && out.contains("each"),
                "{out}"
            );
            assert!(out.contains("Dry-run mode"), "{out}");

            let (_, out) = limit_text(LimitAction::List, vec![config_reply(""), Response::Done]);
            assert!(out.unwrap().starts_with("No app limits"));

            let set = LimitAction::Set {
                name: "node.exe".into(),
                cores: Some(2.0),
                memory: Some(4 << 30),
                each: false,
            };
            let (requests, out) = limit_text(set, vec![config_reply(text), Response::Done]);
            assert!(out.unwrap().starts_with("Limit for node.exe saved"));
            let Request::SetConfig { toml } = &requests[1] else {
                panic!("{requests:?}");
            };
            let limits = Config::from_toml(toml).unwrap().limits;
            assert_eq!(limits.len(), 2);
            assert_eq!(limits[1].cpu_cores, Some(2.0));
            assert_eq!(limits[1].memory_gb, Some(4.0));

            let remove = || LimitAction::Remove {
                name: "ZEN.exe".into(),
                each: true,
            };
            let (requests, out) = limit_text(remove(), vec![config_reply(text), Response::Done]);
            assert_eq!(out.unwrap(), "Limit for ZEN.exe removed.\n");
            assert!(matches!(&requests[1], Request::SetConfig { toml } if !toml.contains("zen")));
            let shared = LimitAction::Remove {
                name: "zen.exe".into(),
                each: false,
            };
            let (_, out) = limit_text(shared, vec![config_reply(text)]);
            assert_eq!(out.unwrap_err().to_string(), "zen.exe has no shared limit");
            let (_, out) = limit_text(
                remove(),
                vec![config_reply(text), Response::Error("denied".into())],
            );
            assert_eq!(out.unwrap_err().to_string(), "denied");
            let (_, out) = limit_text(
                remove(),
                vec![config_reply(text), Response::Events(Vec::new())],
            );
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));

            // Invalid input and service errors.
            let bad = LimitAction::Set {
                name: "x.exe".into(),
                cores: None,
                memory: None,
                each: true,
            };
            let (_, out) = limit_text(bad, vec![config_reply(text)]);
            assert!(out.unwrap_err().to_string().contains("cpu_cores"));
            let (_, out) = limit_text(LimitAction::List, vec![Response::Error("no config".into())]);
            assert_eq!(out.unwrap_err().to_string(), "no config");
            let (_, out) = limit_text(LimitAction::List, vec![Response::Done]);
            assert!(out.unwrap_err().to_string().starts_with("unexpected"));
        });
    }
}
