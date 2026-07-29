use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPath(PathBuf);

impl ConfigPath {
    pub fn resolve(path: Option<&Path>) -> Result<Self> {
        let path = path.unwrap_or_else(|| Path::new("."));
        let resolved = if path.file_name().is_some_and(|name| name == ".infra")
            || path.extension().is_some_and(|ext| ext == "infra")
        {
            path.to_path_buf()
        } else {
            path.join(".infra")
        };
        Ok(Self(resolved))
    }

    pub fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }
}

impl Default for ConfigPath {
    fn default() -> Self {
        Self(PathBuf::from("./.infra"))
    }
}

impl AsRef<Path> for ConfigPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    pub version: u8,
    pub project: String,
    pub runtime: String,
    pub service: ServiceConfig,
    #[serde(default)]
    pub extensions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceConfig {
    pub name: String,
    pub source: String,
    pub build_file: String,
    pub port: u16,
    pub health_path: String,
    pub environment: Option<String>,
}

impl ProjectConfig {
    pub fn render(&self) -> String {
        let environment = self
            .service
            .environment
            .as_deref()
            .map(|value| format!("        environment = \"{value}\"\n"))
            .unwrap_or_default();

        let mut rendered = format!(
            "infra {}\n\nproject \"{}\" {{\n    runtime = \"{}\"\n\n    service \"{}\" {{\n        source = \"{}\"\n        build = \"{}\"\n        expose = {}\n        health = \"{}\"\n{}    }}\n}}\n",
            self.version,
            self.project,
            self.runtime,
            self.service.name,
            self.service.source,
            self.service.build_file,
            self.service.port,
            self.service.health_path,
            environment,
        );

        let extensions = self.extensions.trim();
        if !extensions.is_empty() {
            rendered.push('\n');
            rendered.push_str(extensions);
            rendered.push('\n');
        }
        rendered
    }

    pub fn write(&self, path: &ConfigPath) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }

        let mut config = self.clone();
        if config.extensions.trim().is_empty() && path.as_ref().exists() {
            let existing = fs::read_to_string(path.as_ref())
                .with_context(|| format!("read {} before replacement", path.display()))?;
            let (_, extensions) = split_core_and_extensions(&existing)?;
            config.extensions = extensions;
        }

        fs::write(path.as_ref(), config.render())
            .with_context(|| format!("write {}", path.display()))
    }

    pub fn read(path: &ConfigPath) -> Result<Self> {
        let input = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read {}", path.display()))?;
        parse_generated_config(&input)
    }
}

fn split_core_and_extensions(input: &str) -> Result<(&str, String)> {
    let project_start = input.find("project \"").context("missing project block")?;
    let block_start = input[project_start..]
        .find('{')
        .map(|offset| project_start + offset)
        .context("project block has no opening brace")?;

    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in input[block_start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => depth = depth.saturating_add(1),
            '}' => {
                if depth == 0 {
                    bail!("project block has an unmatched closing brace");
                }
                depth -= 1;
                if depth == 0 {
                    let core_end = block_start + offset + character.len_utf8();
                    return Ok((&input[..core_end], input[core_end..].trim().to_owned()));
                }
            }
            _ => {}
        }
    }

    bail!("project block is not closed")
}

fn parse_generated_config(input: &str) -> Result<ProjectConfig> {
    fn quoted(line: &str) -> Option<String> {
        let start = line.find('"')? + 1;
        let end = line[start..].find('"')? + start;
        Some(line[start..end].to_owned())
    }

    let (core, extensions) = split_core_and_extensions(input)?;
    let mut version = None;
    let mut project = None;
    let mut runtime = None;
    let mut service = None;
    let mut source = None;
    let mut build_file = None;
    let mut port = None;
    let mut health_path = None;
    let mut environment = None;

    for line in core.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("infra ") {
            version = Some(value.parse()?);
        } else if line.starts_with("project ") {
            project = quoted(line);
        } else if line.starts_with("runtime =") {
            runtime = quoted(line);
        } else if line.starts_with("service ") {
            service = quoted(line);
        } else if line.starts_with("source =") {
            source = quoted(line);
        } else if line.starts_with("build =") {
            build_file = quoted(line);
        } else if let Some(value) = line.strip_prefix("expose = ") {
            port = Some(value.parse()?);
        } else if line.starts_with("health =") {
            health_path = quoted(line);
        } else if line.starts_with("environment =") {
            environment = quoted(line);
        }
    }

    Ok(ProjectConfig {
        version: version.context("missing infra version")?,
        project: project.context("missing project")?,
        runtime: runtime.context("missing runtime")?,
        service: ServiceConfig {
            name: service.context("missing service")?,
            source: source.context("missing source")?,
            build_file: build_file.context("missing build file")?,
            port: port.context("missing exposed port")?,
            health_path: health_path.context("missing health path")?,
            environment,
        },
        extensions,
    })
}

pub struct Wizard<R, W> {
    reader: R,
    writer: W,
}

impl Wizard<io::StdinLock<'static>, io::Stdout> {
    pub fn stdio() -> Self {
        let stdin = Box::leak(Box::new(io::stdin()));
        Self {
            reader: stdin.lock(),
            writer: io::stdout(),
        }
    }
}

impl<R: BufRead, W: Write> Wizard<R, W> {
    #[cfg(test)]
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    pub fn run(mut self, path: &ConfigPath) -> Result<ProjectConfig> {
        let cwd = env::current_dir()?;
        let project_default = cwd.file_name().and_then(|v| v.to_str()).unwrap_or("app");

        let project = self.prompt_default("project name", project_default, '[', ']')?;
        let runtime = self.prompt_default("runtime", "docker", '[', ']')?;
        let source = self.prompt_default("service source", ".", '[', ']')?;

        let auto_fill =
            self.prompt_yes_no("use detected values for the remaining fields?", true)?;
        let detected = DetectedValues::from_source(&source);

        let mut config = if auto_fill {
            detected.into_config(project, runtime, source)
        } else {
            ProjectConfig {
                version: 1,
                project,
                runtime,
                service: ServiceConfig {
                    name: self.prompt_default("service name", &detected.service_name, '[', ']')?,
                    source,
                    build_file: self.prompt_default(
                        "build file",
                        &detected.build_file,
                        '[',
                        ']',
                    )?,
                    port: self
                        .prompt_default("port", &detected.port.to_string(), '[', ']')?
                        .parse()?,
                    health_path: self.prompt_default(
                        "health path",
                        &detected.health_path,
                        '[',
                        ']',
                    )?,
                    environment: self
                        .prompt_optional("environment", detected.environment.as_deref())?,
                },
                extensions: String::new(),
            }
        };

        loop {
            self.summary(&config, path)?;
            let answer = self.prompt_choice("write configuration? [y/e/n]: ")?;
            match answer.as_str() {
                "" | "y" | "yes" => return Ok(config),
                "n" | "no" => anyhow::bail!("configuration not written"),
                "e" | "edit" => self.edit_auto_fields(&mut config, path)?,
                _ => writeln!(self.writer, "enter y, e, or n")?,
            }
        }
    }

    fn edit_auto_fields(&mut self, config: &mut ProjectConfig, path: &ConfigPath) -> Result<()> {
        config.service.build_file =
            self.prompt_default("build file", &config.service.build_file, '(', ')')?;

        if !self.prompt_yes_no("edit remaining fields?", false)? {
            return Ok(());
        }

        config.service.name =
            self.prompt_default("service name", &config.service.name, '(', ')')?;
        config.service.port = self
            .prompt_default("port", &config.service.port.to_string(), '(', ')')?
            .parse()?;
        config.service.health_path =
            self.prompt_default("health path", &config.service.health_path, '(', ')')?;
        config.service.environment = self
            .prompt_optional_parenthesized("environment", config.service.environment.as_deref())?;

        let current_output = path.display().to_string();
        let output = self.prompt_default("output", &current_output, '(', ')')?;
        if output != current_output {
            anyhow::bail!(
                "output path editing is not supported after wizard start; run infra conf <path>"
            );
        }
        Ok(())
    }

    fn summary(&mut self, config: &ProjectConfig, path: &ConfigPath) -> Result<()> {
        writeln!(self.writer, "\nconfiguration\n")?;
        writeln!(self.writer, "project      {}", config.project)?;
        writeln!(self.writer, "runtime      {}", config.runtime)?;
        writeln!(self.writer, "service      {}", config.service.name)?;
        writeln!(self.writer, "source       {}", config.service.source)?;
        writeln!(self.writer, "build        {}", config.service.build_file)?;
        writeln!(self.writer, "port         {}", config.service.port)?;
        writeln!(self.writer, "health       {}", config.service.health_path)?;
        writeln!(
            self.writer,
            "environment  {}",
            config.service.environment.as_deref().unwrap_or("none")
        )?;
        writeln!(self.writer, "output       {}\n", path.display())?;
        Ok(())
    }

    fn prompt_default(
        &mut self,
        label: &str,
        default: &str,
        open: char,
        close: char,
    ) -> Result<String> {
        let value = self.prompt_value(&format!("{label} {open}{default}{close}: "))?;
        Ok(if value.is_empty() {
            default.to_owned()
        } else {
            value
        })
    }

    fn prompt_optional(&mut self, label: &str, default: Option<&str>) -> Result<Option<String>> {
        let value = self.prompt_default(label, default.unwrap_or("none"), '[', ']')?;
        Ok((!value.eq_ignore_ascii_case("none")).then_some(value))
    }

    fn prompt_optional_parenthesized(
        &mut self,
        label: &str,
        default: Option<&str>,
    ) -> Result<Option<String>> {
        let value = self.prompt_default(label, default.unwrap_or("none"), '(', ')')?;
        Ok((!value.eq_ignore_ascii_case("none")).then_some(value))
    }

    fn prompt_yes_no(&mut self, label: &str, default_yes: bool) -> Result<bool> {
        loop {
            let value = self.prompt_choice(&format!("{label} [y/n]: "))?;
            match value.as_str() {
                "" => return Ok(default_yes),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => writeln!(self.writer, "enter y or n")?,
            }
        }
    }

    fn prompt_choice(&mut self, prompt: &str) -> Result<String> {
        Ok(self.prompt_value(prompt)?.to_ascii_lowercase())
    }

    fn prompt_value(&mut self, prompt: &str) -> Result<String> {
        write!(self.writer, "{prompt}")?;
        self.writer.flush()?;
        let mut value = String::new();
        self.reader.read_line(&mut value)?;
        Ok(value.trim().to_owned())
    }
}

struct DetectedValues {
    service_name: String,
    build_file: String,
    port: u16,
    health_path: String,
    environment: Option<String>,
}

impl DetectedValues {
    fn from_source(source: &str) -> Self {
        let build_file = if Path::new(source).join("Containerfile").exists() {
            "Containerfile"
        } else {
            "Dockerfile"
        };
        Self {
            service_name: "api".to_owned(),
            build_file: build_file.to_owned(),
            port: 8080,
            health_path: "/health".to_owned(),
            environment: Path::new(source)
                .join(".env")
                .exists()
                .then(|| ".env".to_owned()),
        }
    }

    fn into_config(self, project: String, runtime: String, source: String) -> ProjectConfig {
        ProjectConfig {
            version: 1,
            project,
            runtime,
            service: ServiceConfig {
                name: self.service_name,
                source,
                build_file: self.build_file,
                port: self.port,
                health_path: self.health_path,
                environment: self.environment,
            },
            extensions: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_config() -> ProjectConfig {
        ProjectConfig {
            version: 1,
            project: "market".into(),
            runtime: "docker".into(),
            service: ServiceConfig {
                name: "api".into(),
                source: ".".into(),
                build_file: "Dockerfile".into(),
                port: 8080,
                health_path: "/health".into(),
                environment: Some(".env".into()),
            },
            extensions: String::new(),
        }
    }

    #[test]
    fn directory_paths_resolve_to_dot_infra() {
        assert_eq!(
            ConfigPath::resolve(Some(Path::new("/srv/app")))
                .unwrap()
                .as_ref(),
            Path::new("/srv/app/.infra")
        );
    }

    #[test]
    fn explicit_infra_filename_is_preserved() {
        assert_eq!(
            ConfigPath::resolve(Some(Path::new("prod.infra")))
                .unwrap()
                .as_ref(),
            Path::new("prod.infra")
        );
    }

    #[test]
    fn generated_config_round_trips() {
        let config = sample_config();
        assert_eq!(parse_generated_config(&config.render()).unwrap(), config);
    }

    #[test]
    fn application_extensions_round_trip() {
        let mut config = sample_config();
        config.extensions = "infrabot {\n    public_url = \"https://bot.example\"\n}".into();
        assert_eq!(parse_generated_config(&config.render()).unwrap(), config);
    }

    #[test]
    fn write_preserves_existing_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let path = ConfigPath(directory.path().join(".infra"));
        let mut existing = sample_config();
        existing.extensions = "infrabot {\n    public_url = \"https://bot.example\"\n}".into();
        fs::write(path.as_ref(), existing.render()).unwrap();

        let replacement = ProjectConfig {
            project: "renamed".into(),
            ..sample_config()
        };
        replacement.write(&path).unwrap();
        let written = ProjectConfig::read(&path).unwrap();
        assert_eq!(written.project, "renamed");
        assert_eq!(written.extensions, existing.extensions);
    }

    #[test]
    fn values_preserve_case_but_choices_are_case_insensitive() {
        let input = Cursor::new(b"Dockerfile\nY\n");
        let output = Vec::new();
        let mut wizard = Wizard::new(input, output);

        assert_eq!(
            wizard
                .prompt_default("build file", "Containerfile", '(', ')')
                .unwrap(),
            "Dockerfile"
        );
        assert!(wizard.prompt_yes_no("continue?", false).unwrap());
    }
}
