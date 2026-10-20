use clap::{Subcommand, ValueEnum};
use std::process::ExitCode;
use wtm_core::power::{self, FrequencyRequest, PowerSupply};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Supply {
    Ac,
    Dc,
    Both,
}
impl From<Supply> for PowerSupply {
    fn from(v: Supply) -> Self {
        match v {
            Supply::Ac => Self::Ac,
            Supply::Dc => Self::Dc,
            Supply::Both => Self::Both,
        }
    }
}
#[derive(Subcommand)]
pub enum PowerCommand {
    /// Read the active plan's AC/DC frequency policies and saved backup
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Set a shared power-plan limit, not a per-process MHz cap
    Limit {
        /// Windows power policy class, not an assumed P/E core label
        #[arg(long,value_parser=clap::value_parser!(u8).range(0..=1))]
        class: u8,
        #[arg(long,value_parser=clap::value_parser!(u32).range(100..=64000))]
        mhz: u32,
        #[arg(long,value_enum,default_value_t=Supply::Ac)]
        source: Supply,
        /// Confirm this affects all workloads using this policy class
        #[arg(long)]
        yes: bool,
    },
    /// Restore saved original values without switching to an inactive plan
    Restore {
        #[arg(long)]
        yes: bool,
    },
}
pub fn run(command: PowerCommand) -> anyhow::Result<ExitCode> {
    match command {
        PowerCommand::Show { json } => {
            let report = power::frequency_report().map_err(anyhow::Error::msg)?;
            if json {
                serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
                println!();
            } else {
                println!("Active power plan: {}", report.plan);
                for v in &report.values {
                    println!(
                        "Policy class {} | {} | {}",
                        v.class,
                        v.source.label(),
                        v.mhz.map_or_else(
                            || "unavailable".into(),
                            |v| if v == 0 {
                                "Windows default (no fixed MHz cap)".into()
                            } else {
                                format!("{v} MHz")
                            }
                        )
                    );
                }
                if let Some(plan) = report.backup_plan {
                    println!(
                        "Original settings saved for plan {plan}; use wtm power restore --yes"
                    );
                }
                if let Some(error) = report.backup_error {
                    println!("Backup unavailable: {error}");
                }
                println!(
                    "Frequency policies affect shared cores and all their workloads. Class 1 does not automatically mean E-cores."
                );
            }
        }
        PowerCommand::Limit {
            class,
            mhz,
            source,
            yes,
        } => {
            anyhow::ensure!(
                yes,
                "This changes a shared Windows power plan for all workloads, not one PID. Pass --yes to confirm; originals are saved for wtm power restore --yes"
            );
            power::set_frequency(&FrequencyRequest {
                plan: power::frequency_report().map_err(anyhow::Error::msg)?.plan,
                class,
                source: source.into(),
                mhz,
            })
            .map_err(anyhow::Error::msg)?;
            println!(
                "Shared frequency limit saved. Use wtm power restore --yes to restore original settings."
            );
        }
        PowerCommand::Restore { yes } => {
            anyhow::ensure!(
                yes,
                "Pass --yes to restore the saved Windows power-plan values"
            );
            power::restore_frequency().map_err(anyhow::Error::msg)?;
            println!("Original frequency policy restored. No power plan was switched.");
        }
    }
    Ok(ExitCode::SUCCESS)
}
