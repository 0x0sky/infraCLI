use crate::config::ProjectConfig;
use anyhow::{Context, Result};
use std::process::Command;

pub trait Runtime {
    fn status(&self, path: &crate::config::ConfigPath) -> Result<()>;
    fn apply(&self, config: &ProjectConfig) -> Result<()>;
    fn stop_project(&self, config: &ProjectConfig) -> Result<()>;
    fn remove_all_services(&self, config: &ProjectConfig) -> Result<()>;
    fn service_status(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn logs(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn restart(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn stop_service(&self, config: &ProjectConfig, service: &str) -> Result<()>;
    fn remove_service(&self, config: &ProjectConfig, service: &str) -> Result<()>;
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

    fn image_tag(config: &ProjectConfig, service: &str) -> String {
        format!("{}-{service}:latest", config.project)
    }

    fn run_service(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        let container = Self::container_name(config, service);
        let tag = Self::image_tag(config, service);
        let port = format!("{}:{}", config.service.port, config.service.port);
        self.docker(&["run", "-d", "--name", &container, "-p", &port, &tag])
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
        let tag = Self::image_tag(config, &config.service.name);
        let container = Self::container_name(config, &config.service.name);
        println!("applying configuration...");
        self.docker(&[
            "build",
            "-f",
            &config.service.build_file,
            "-t",
            &tag,
            &config.service.source,
        ])?;
        let _ = Command::new("docker").args(["rm", "-f", &container]).status();
        self.run_service(config, &config.service.name)?;
        println!("configuration applied");
        Ok(())
    }

    fn stop_project(&self, config: &ProjectConfig) -> Result<()> {
        self.stop_service(config, &config.service.name)?;
        println!("project stopped");
        Ok(())
    }

    fn remove_all_services(&self, config: &ProjectConfig) -> Result<()> {
        self.remove_service(config, &config.service.name)?;
        println!("all services removed");
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
        let container = Self::container_name(config, service);
        println!("restarting {service}...");
        let _ = Command::new("docker").args(["rm", "-f", &container]).status();
        self.run_service(config, service)?;
        println!("{service} running");
        Ok(())
    }

    fn stop_service(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        self.docker(&["stop", &Self::container_name(config, service)])?;
        println!("{service} stopped");
        Ok(())
    }

    fn remove_service(&self, config: &ProjectConfig, service: &str) -> Result<()> {
        self.ensure_service(config, service)?;
        self.docker(&["rm", "-f", &Self::container_name(config, service)])?;
        println!("{service} removed");
        Ok(())
    }
}
