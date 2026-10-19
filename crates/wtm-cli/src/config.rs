//! `wtm config`: show, check and replace the service's configuration.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use wtm_ipc::{Client, Request, Response};
use wtm_policy::{Config, DEFAULT_CONFIG_TOML};

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
}
