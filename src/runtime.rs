use crate::config::ProjectConfig;
use anyhow::{Context, Result};
use std::process::Command;

pub trait Runtime {
    fn status(&self, path: &crate::config::ConfigPath) -> Result<()>;
    fn apply(&self, config: &ProjectConfig) -> Result<()>;
    fn stop_project(&self, config: &ProjectConfig) -> Result<()>;
    fn service_status(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn logs(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn restart(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn stop_service(&self, config: &ProjectConfig, service: &str) -> Result<()>;
}

pub struct DockerRuntime;

impl DockerRuntime {
    fn ensure_service<'a>(&self, config: &'a ProjectConfig, service: &str) -> Result<&'a str> {
        if config.service.name != service {
            anyhow::bail!("unknown service: {service}");
        }
        Ok(service)
    }

    fn docker(&self, args: &[&str]) -> Result<()> {
        let status = Command::new("docker")
            .args(args)
            .status()
            .context("docker runtime is unavailable")?;
        if !status.success() {
            anyhow::bail!("docker command failed with {status}");
        }
        Ok(())
    }

    fn container_name(config: &ProjectConfig, service: &str) -> String {
        format!("{}-{service}", config.project)
    }
}

impl Runtime for DockerRuntime {
    fn status(&self, path: &crate::config::ConfigPath) -> Result<()> {
        let config = ProjectConfig::read(path)?;
        println!("project  {}", config.project);
        println!("runtime  {}", config.runtime);
        println!("service  {}", config.service.name);
        Ok(())
    }

    fn apply(&self, config: &ProjectConfig) -> Result<()> {
        if config.runtime != "docker" {
            anyhow::bail!("unsupported runtime: {}", config.runtime);
        }
        let tag = format!("{}-{}:latest", config.project, config.service.name);
        let container = Self::container_name(config, &config.service.name);
        println!("applying configuration...");
        self.docker(&["build", "-f", &config.service.build_file, "-t", &tag, &config.service.source])?;
        let _ = Command::new("docker").args(["rm", "-f", &container]).status();
        self.docker(&[
            "run", "-d", "--name", &container,
            "-p", &format!("{}:{}", config.service.port, config.service.port),
            &tag,
        ])?;
        println!("configuration applied");
        Ok(())
    }

    fn stop_project(&self, config: &ProjectConfig) -> Result<()> {
        let container = Self::container_name(config, &config.service.name);
        self.docker(&["stop", &container])?;
        println!("project stopped");
        Ok(())
    }

    fn service_status(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        let container = Self::container_name(config, service);
        self.docker(&["ps", "-a", "--filter", &format!("name=^{container}$")])
    }

    fn logs(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        self.docker(&["logs", &Self::container_name(config, service)])
    }

    fn restart(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        self.docker(&["restart", &Self::container_name(config, service)])
    }

    fn stop_service(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        self.docker(&["stop", &Self::container_name(config, service)])
    }
}
