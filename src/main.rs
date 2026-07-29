mod cli;
mod config;
mod runtime;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ServiceAction};
use config::{ConfigPath, ProjectConfig, Wizard};
use runtime::{DockerRuntime, Runtime};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = DockerRuntime;

    match cli.command {
        None => runtime.status(&ConfigPath::default())?,
        Some(Command::Conf(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            let config = Wizard::stdio().run(&path)?;
            config.write(&path)?;
            println!("configuration written to {}", path.display());
            if args.apply {
                runtime.apply(&config)?;
            }
        }
        Some(Command::Apply(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            let config = ProjectConfig::read(&path)?;
            runtime.apply(&config)?;
        }
        Some(Command::Stop(args)) => {
            let path = ConfigPath::resolve(args.path.as_deref())?;
            let config = ProjectConfig::read(&path)?;
            runtime.stop_project(&config)?;
        }
        Some(Command::Service(args)) => {
            let path = ConfigPath::default();
            let config = ProjectConfig::read(&path)?;
            match args.action {
                None => runtime.service_status(&config, &args.name)?,
                Some(ServiceAction::Logs) => runtime.logs(&config, &args.name)?,
                Some(ServiceAction::Restart) => runtime.restart(&config, &args.name)?,
                Some(ServiceAction::Stop) => runtime.stop_service(&config, &args.name)?,
            }
        }
    }

    Ok(())
}
