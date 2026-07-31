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
    Auth(AuthArgs),
    Agent(AgentArgs),
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

#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub provider: AuthProvider,
}

#[derive(Debug, Subcommand)]
pub enum AuthProvider {
    Telegram(TelegramAuthArgs),
}

#[derive(Debug, Args)]
pub struct TelegramAuthArgs {
    /// Public infraBot API base URL.
    #[arg(long, env = "INFRABOT_URL")]
    pub endpoint: String,

    /// Source identifier declared in infraBot's .infra registry.
    #[arg(long, env = "INFRA_SOURCE")]
    pub source: String,

    /// Print the Telegram deep link without opening it.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    /// .infra file containing the agent input/output contract.
    #[arg(long, env = "INFRA_CONFIG", default_value = ".infra")]
    pub config: PathBuf,

    /// Poll once, update state, and exit.
    #[arg(long)]
    pub once: bool,
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

    #[test]
    fn parses_telegram_auth() {
        let cli = Cli::try_parse_from([
            "infra",
            "auth",
            "telegram",
            "--endpoint",
            "https://bot.example",
            "--source",
            "primary",
            "--no-open",
        ])
        .unwrap();
        let Some(Command::Auth(args)) = cli.command else {
            panic!("expected auth command");
        };
        let AuthProvider::Telegram(args) = args.provider;
        assert_eq!(args.endpoint, "https://bot.example");
        assert_eq!(args.source, "primary");
        assert!(args.no_open);
    }

    #[test]
    fn parses_agent_command() {
        let cli =
            Cli::try_parse_from(["infra", "agent", "--config", "/etc/infra/.infra", "--once"])
                .unwrap();
        let Some(Command::Agent(args)) = cli.command else {
            panic!("expected agent command");
        };
        assert_eq!(args.config, PathBuf::from("/etc/infra/.infra"));
        assert!(args.once);
    }
}
