use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "infra", version, about = "predictable infrastructure control")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Conf(ConfArgs),
    Apply(PathArgs),
    Stop(PathArgs),
    #[command(external_subcommand)]
    Service(Vec<String>),
}

#[derive(Debug, Args)]
pub struct ConfArgs {
    pub path: Option<PathBuf>,
    #[arg(short = 'a', long = "apply")]
    pub apply: bool,
}

#[derive(Debug, Args)]
pub struct PathArgs {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum ServiceAction {
    Logs,
    Restart,
    Stop,
}

#[derive(Debug)]
pub struct ServiceArgs {
    pub name: String,
    pub action: Option<ServiceAction>,
}

impl TryFrom<Vec<String>> for ServiceArgs {
    type Error = anyhow::Error;

    fn try_from(parts: Vec<String>) -> Result<Self, Self::Error> {
        let mut parts = parts.into_iter();
        let name = parts.next().ok_or_else(|| anyhow::anyhow!("service name is required"))?;
        let action = match parts.next().as_deref() {
            None => None,
            Some("logs") => Some(ServiceAction::Logs),
            Some("restart") => Some(ServiceAction::Restart),
            Some("stop") => Some(ServiceAction::Stop),
            Some(other) => anyhow::bail!("unknown service action: {other}"),
        };
        if parts.next().is_some() {
            anyhow::bail!("too many service arguments");
        }
        Ok(Self { name, action })
    }
}
