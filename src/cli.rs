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
    Rm(RmArgs),
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

#[derive(Debug, Args)]
pub struct RmArgs {
    /// Remove one service while preserving .infra.
    pub service: Option<String>,

    /// Remove all managed services while preserving .infra.
    #[arg(short = 'a', long = "all", conflicts_with = "service")]
    pub all: bool,

    /// Configuration path used for deinitialization or service resolution.
    #[arg(long = "path")]
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
        let name = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("service name is required"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_rm_service() {
        let cli = Cli::try_parse_from(["infra", "rm", "api"]).unwrap();
        let Some(Command::Rm(args)) = cli.command else {
            panic!("expected rm command");
        };
        assert_eq!(args.service.as_deref(), Some("api"));
        assert!(!args.all);
    }

    #[test]
    fn parses_rm_all_aliases() {
        for flag in ["-a", "--all"] {
            let cli = Cli::try_parse_from(["infra", "rm", flag]).unwrap();
            let Some(Command::Rm(args)) = cli.command else {
                panic!("expected rm command");
            };
            assert!(args.all);
            assert!(args.service.is_none());
        }
    }

    #[test]
    fn rejects_rm_all_with_service() {
        assert!(Cli::try_parse_from(["infra", "rm", "api", "--all"]).is_err());
    }
}
