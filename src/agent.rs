use anyhow::{Context, Result, bail};
use reqwest::{Url, blocking::Client};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env, fs,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const ALLOWED_INPUT_FIELDS: &[&str] = &[
    "input",
    "docker_address",
    "container",
    "container_id",
    "project",
    "service",
    "image",
    "state",
    "health",
];
const ALLOWED_OUTPUT_FIELDS: &[&str] = &[
    "kind",
    "input",
    "docker_address",
    "container",
    "container_id",
    "project",
    "service",
    "image",
    "state",
    "health",
    "previous_status",
    "status",
];

pub struct AgentOptions {
    pub config: PathBuf,
    pub once: bool,
}

#[derive(Debug, Clone)]
struct AgentConfig {
    poll_interval: Duration,
    health_bind: SocketAddr,
    state_file: PathBuf,
    inputs: Vec<DockerInput>,
    output: InfraBotOutput,
}

#[derive(Debug, Clone)]
struct DockerInput {
    id: String,
    addresses: Vec<String>,
    projects: HashSet<String>,
    services: HashSet<String>,
    fields: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct InfraBotOutput {
    id: String,
    endpoint: String,
    source: String,
    credential: PathBuf,
    events: HashSet<String>,
    fields: Vec<String>,
}

#[derive(Debug, Clone)]
enum ValueRef {
    Literal(String),
    Environment(String),
}

#[derive(Default)]
struct AgentBuilder {
    poll_interval_seconds: Option<u64>,
    health_bind: Option<ValueRef>,
    state_file: Option<ValueRef>,
    inputs: Vec<InputBuilder>,
    outputs: Vec<OutputBuilder>,
}

struct InputBuilder {
    id: String,
    driver: Option<String>,
    addresses: Option<Vec<String>>,
    projects: Option<Vec<String>>,
    services: Option<Vec<String>>,
    fields: Option<Vec<String>>,
}

struct OutputBuilder {
    id: String,
    driver: Option<String>,
    endpoint: Option<ValueRef>,
    source: Option<String>,
    credential: Option<ValueRef>,
    events: Option<Vec<String>>,
    fields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerContainerSummary {
    id: String,
    names: Vec<String>,
    image: String,
    state: String,
    status: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ObservedContainer {
    input: String,
    docker_address: String,
    container: String,
    container_id: String,
    project: String,
    service: String,
    image: String,
    state: String,
    health: String,
    status: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentState {
    #[serde(default)]
    containers: BTreeMap<String, ObservedContainer>,
}

#[derive(Debug, Deserialize)]
struct CredentialFile {
    version: u8,
    endpoint: String,
    source: String,
    token_type: String,
    access_token: String,
    expires_at: u64,
}

#[derive(Serialize)]
struct EventRequest<'a> {
    output: &'a str,
    kind: &'a str,
    project: &'a str,
    service: Option<&'a str>,
    status: Option<&'a str>,
    message: String,
    fields: BTreeMap<String, String>,
}

pub fn run_agent(options: AgentOptions) -> Result<()> {
    let input = fs::read_to_string(&options.config)
        .with_context(|| format!("read agent configuration from {}", options.config.display()))?;
    let config = parse_document(&input, &|name| env::var(name).ok()).with_context(|| {
        format!(
            "parse agent configuration from {}",
            options.config.display()
        )
    })?;
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("infra-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build agent HTTP client")?;

    spawn_health_server(config.health_bind)?;
    eprintln!(
        "infra agent started: {} input(s), output {}, interval {}s",
        config.inputs.len(),
        config.output.id,
        config.poll_interval.as_secs()
    );

    let mut state = read_state(&config.state_file)?;
    loop {
        match poll_once(&client, &config, &state) {
            Ok(next) => {
                write_state(&config.state_file, &next)?;
                state = next;
            }
            Err(error) => eprintln!("infra agent poll failed: {error:#}"),
        }

        if options.once {
            return Ok(());
        }
        thread::sleep(config.poll_interval);
    }
}

fn poll_once(client: &Client, config: &AgentConfig, previous: &AgentState) -> Result<AgentState> {
    let current = collect_snapshot(client, &config.inputs)?;
    if previous.containers.is_empty() {
        eprintln!(
            "infra agent baseline captured: {} container(s)",
            current.len()
        );
        return Ok(AgentState {
            containers: current,
        });
    }

    let credential = read_credential(&config.output)?;
    let events = transitions(&previous.containers, &current);
    for (kind, before, after) in events {
        if !config.output.events.contains(&kind) {
            continue;
        }
        let observed = after
            .as_ref()
            .or(before.as_ref())
            .context("transition has no state")?;
        let input_fields = config
            .inputs
            .iter()
            .find(|input| input.id == observed.input)
            .map(|input| &input.fields)
            .context("transition input is not configured")?;
        send_event(
            client,
            &config.output,
            &credential,
            &kind,
            before.as_ref(),
            after.as_ref(),
            input_fields,
        )?;
    }

    Ok(AgentState {
        containers: current,
    })
}

fn collect_snapshot(
    client: &Client,
    inputs: &[DockerInput],
) -> Result<BTreeMap<String, ObservedContainer>> {
    let mut snapshot = BTreeMap::new();
    for input in inputs {
        for address in &input.addresses {
            let url = format!("{}/containers/json?all=1", address.trim_end_matches('/'));
            let containers = client
                .get(&url)
                .send()
                .with_context(|| format!("query Docker input {} at {address}", input.id))?
                .error_for_status()
                .with_context(|| format!("Docker input {} rejected container listing", input.id))?
                .json::<Vec<DockerContainerSummary>>()
                .with_context(|| format!("decode Docker input {} response", input.id))?;

            for container in containers {
                let project = container
                    .labels
                    .get("com.docker.compose.project")
                    .cloned()
                    .unwrap_or_default();
                let service = container
                    .labels
                    .get("com.docker.compose.service")
                    .cloned()
                    .unwrap_or_default();
                if !input.projects.is_empty() && !input.projects.contains(&project) {
                    continue;
                }
                if !input.services.is_empty() && !input.services.contains(&service) {
                    continue;
                }

                let health = health_from_status(&container.status);
                let status = normalized_status(&container.state, &health).to_owned();
                let name = container
                    .names
                    .first()
                    .map(|value| value.trim_start_matches('/').to_owned())
                    .unwrap_or_else(|| container.id.chars().take(12).collect());
                let observed = ObservedContainer {
                    input: input.id.clone(),
                    docker_address: address.clone(),
                    container: name,
                    container_id: container.id.clone(),
                    project,
                    service,
                    image: container.image,
                    state: container.state,
                    health,
                    status,
                };
                let key = format!("{}:{}", input.id, container.id);
                if snapshot.insert(key.clone(), observed).is_some() {
                    bail!("duplicate Docker container identity {key}");
                }
            }
        }
    }
    Ok(snapshot)
}

fn transitions(
    previous: &BTreeMap<String, ObservedContainer>,
    current: &BTreeMap<String, ObservedContainer>,
) -> Vec<(String, Option<ObservedContainer>, Option<ObservedContainer>)> {
    let mut events = Vec::new();
    for (key, after) in current {
        match previous.get(key) {
            None => events.push((
                if after.status == "healthy" {
                    "service.started"
                } else {
                    "service.failed"
                }
                .to_owned(),
                None,
                Some(after.clone()),
            )),
            Some(before) if before.status != after.status => {
                let kind = match (before.status.as_str(), after.status.as_str()) {
                    ("failed", "healthy") => "service.recovered",
                    (_, "failed") => "service.failed",
                    (_, "healthy") => "service.started",
                    _ => "service.changed",
                };
                events.push((kind.to_owned(), Some(before.clone()), Some(after.clone())));
            }
            _ => {}
        }
    }
    for (key, before) in previous {
        if !current.contains_key(key) {
            events.push(("service.removed".to_owned(), Some(before.clone()), None));
        }
    }
    events
}

fn send_event(
    client: &Client,
    output: &InfraBotOutput,
    credential: &CredentialFile,
    kind: &str,
    before: Option<&ObservedContainer>,
    after: Option<&ObservedContainer>,
    input_fields: &BTreeSet<String>,
) -> Result<()> {
    let observed = after.or(before).context("transition has no state")?;
    let previous_status = before
        .map(|value| value.status.as_str())
        .unwrap_or("absent");
    let current_status = after.map(|value| value.status.as_str()).unwrap_or("absent");
    let available = event_fields(
        kind,
        previous_status,
        current_status,
        observed,
        input_fields,
    );
    let fields = output
        .fields
        .iter()
        .filter_map(|name| {
            available
                .get(name)
                .map(|value| (name.clone(), value.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let subject = if observed.service.is_empty() {
        observed.container.as_str()
    } else {
        observed.service.as_str()
    };
    let request = EventRequest {
        output: &output.id,
        kind,
        project: if observed.project.is_empty() {
            &observed.input
        } else {
            &observed.project
        },
        service: Some(subject),
        status: Some(current_status),
        message: format!("{subject}: {previous_status} -> {current_status}"),
        fields,
    };

    client
        .post(format!(
            "{}/v1/events",
            output.endpoint.trim_end_matches('/')
        ))
        .bearer_auth(&credential.access_token)
        .json(&request)
        .send()
        .with_context(|| format!("deliver {kind} through output {}", output.id))?
        .error_for_status()
        .with_context(|| format!("infraBot output {} rejected {kind}", output.id))?;
    Ok(())
}

fn event_fields(
    kind: &str,
    previous_status: &str,
    current_status: &str,
    observed: &ObservedContainer,
    input_fields: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::from([
        ("kind".to_owned(), kind.to_owned()),
        ("input".to_owned(), observed.input.clone()),
        ("docker_address".to_owned(), observed.docker_address.clone()),
        ("container".to_owned(), observed.container.clone()),
        ("container_id".to_owned(), observed.container_id.clone()),
        ("project".to_owned(), observed.project.clone()),
        ("service".to_owned(), observed.service.clone()),
        ("image".to_owned(), observed.image.clone()),
        ("state".to_owned(), observed.state.clone()),
        ("health".to_owned(), observed.health.clone()),
        ("previous_status".to_owned(), previous_status.to_owned()),
        ("status".to_owned(), current_status.to_owned()),
    ]);
    fields.retain(|name, _| {
        matches!(name.as_str(), "kind" | "previous_status" | "status")
            || input_fields.contains(name)
    });
    fields
}

fn health_from_status(status: &str) -> String {
    if status.contains("(unhealthy)") {
        "unhealthy"
    } else if status.contains("(healthy)") {
        "healthy"
    } else if status.contains("health: starting") || status.contains("(health: starting)") {
        "starting"
    } else {
        "none"
    }
    .to_owned()
}

fn normalized_status(state: &str, health: &str) -> &'static str {
    if health == "unhealthy" || matches!(state, "exited" | "dead") {
        "failed"
    } else if state == "running" && matches!(health, "healthy" | "none") {
        "healthy"
    } else {
        "unknown"
    }
}

fn read_credential(output: &InfraBotOutput) -> Result<CredentialFile> {
    let content = fs::read_to_string(&output.credential)
        .with_context(|| format!("read output credential {}", output.credential.display()))?;
    let credential: CredentialFile = serde_json::from_str(&content)
        .with_context(|| format!("decode output credential {}", output.credential.display()))?;
    if credential.version != 1
        || credential.token_type != "Bearer"
        || credential.access_token.is_empty()
        || credential.source != output.source
    {
        bail!("output credential does not match source {}", output.source);
    }
    if credential.endpoint.is_empty() {
        bail!("output credential endpoint is empty");
    }
    if credential.expires_at <= unix_time()? {
        bail!("output credential for source {} has expired", output.source);
    }
    Ok(credential)
}

fn read_state(path: &Path) -> Result<AgentState> {
    if !path.exists() {
        return Ok(AgentState::default());
    }
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("decode {}", path.display()))
}

fn write_state(path: &Path, state: &AgentState) -> Result<()> {
    let parent = path.parent().context("agent state path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let content = serde_json::to_vec_pretty(state).context("encode agent state")?;
    let temporary = parent.join(format!(".state-{}.tmp", std::process::id()));
    let mut file =
        fs::File::create(&temporary).with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(&content)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

fn spawn_health_server(bind: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(bind).with_context(|| format!("bind agent health {bind}"))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0_u8; 1024];
            let size = stream.read(&mut request).unwrap_or(0);
            let first_line = String::from_utf8_lossy(&request[..size]);
            let healthy = first_line.starts_with("GET /health ");
            let (status, body) = if healthy {
                ("200 OK", "ok")
            } else {
                ("404 Not Found", "not found")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    Ok(())
}

fn parse_document<F>(input: &str, resolver: &F) -> Result<AgentConfig>
where
    F: Fn(&str) -> Option<String>,
{
    let lines = significant_lines(input);
    let Some(start) = lines.iter().position(|(_, line)| *line == "agent {") else {
        bail!("missing agent block");
    };
    let mut index = start + 1;
    let builder = parse_agent(&lines, &mut index)?;
    build_config(builder, resolver)
}

fn significant_lines(input: &str) -> Vec<(usize, &str)> {
    input
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            (!line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
                .then_some((index + 1, line))
        })
        .collect()
}

fn parse_agent(lines: &[(usize, &str)], index: &mut usize) -> Result<AgentBuilder> {
    let mut builder = AgentBuilder::default();
    while let Some((line_number, line)) = lines.get(*index).copied() {
        *index += 1;
        if line == "}" {
            return Ok(builder);
        }
        if let Some(id) = named_block(line, "input") {
            builder.inputs.push(parse_input(lines, index, id)?);
            continue;
        }
        if let Some(id) = named_block(line, "output") {
            builder.outputs.push(parse_output(lines, index, id)?);
            continue;
        }
        let (key, value) = assignment(line_number, line)?;
        match key {
            "poll_interval_seconds" => set_once(
                &mut builder.poll_interval_seconds,
                integer(line_number, value)?,
                line_number,
                key,
            )?,
            "health_bind" => set_once(
                &mut builder.health_bind,
                value_ref(line_number, value)?,
                line_number,
                key,
            )?,
            "state_file" => set_once(
                &mut builder.state_file,
                value_ref(line_number, value)?,
                line_number,
                key,
            )?,
            _ => bail!("line {line_number}: unknown agent field {key}"),
        }
    }
    bail!("agent block is not closed")
}

fn parse_input(lines: &[(usize, &str)], index: &mut usize, id: String) -> Result<InputBuilder> {
    validate_identifier("input", &id)?;
    let mut builder = InputBuilder {
        id,
        driver: None,
        addresses: None,
        projects: None,
        services: None,
        fields: None,
    };
    while let Some((line_number, line)) = lines.get(*index).copied() {
        *index += 1;
        if line == "}" {
            return Ok(builder);
        }
        let (key, value) = assignment(line_number, line)?;
        match key {
            "driver" => set_once(
                &mut builder.driver,
                quoted(line_number, value)?,
                line_number,
                key,
            )?,
            "addresses" => set_once(
                &mut builder.addresses,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            "projects" => set_once(
                &mut builder.projects,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            "services" => set_once(
                &mut builder.services,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            "fields" => set_once(
                &mut builder.fields,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            _ => bail!("line {line_number}: unknown input field {key}"),
        }
    }
    bail!("input block is not closed")
}

fn parse_output(lines: &[(usize, &str)], index: &mut usize, id: String) -> Result<OutputBuilder> {
    validate_identifier("output", &id)?;
    let mut builder = OutputBuilder {
        id,
        driver: None,
        endpoint: None,
        source: None,
        credential: None,
        events: None,
        fields: None,
    };
    while let Some((line_number, line)) = lines.get(*index).copied() {
        *index += 1;
        if line == "}" {
            return Ok(builder);
        }
        let (key, value) = assignment(line_number, line)?;
        match key {
            "driver" => set_once(
                &mut builder.driver,
                quoted(line_number, value)?,
                line_number,
                key,
            )?,
            "endpoint" => set_once(
                &mut builder.endpoint,
                value_ref(line_number, value)?,
                line_number,
                key,
            )?,
            "source" => set_once(
                &mut builder.source,
                quoted(line_number, value)?,
                line_number,
                key,
            )?,
            "credential" => set_once(
                &mut builder.credential,
                value_ref(line_number, value)?,
                line_number,
                key,
            )?,
            "events" => set_once(
                &mut builder.events,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            "fields" => set_once(
                &mut builder.fields,
                string_list(line_number, value)?,
                line_number,
                key,
            )?,
            _ => bail!("line {line_number}: unknown output field {key}"),
        }
    }
    bail!("output block is not closed")
}

fn build_config<F>(builder: AgentBuilder, resolver: &F) -> Result<AgentConfig>
where
    F: Fn(&str) -> Option<String>,
{
    let poll_interval_seconds = builder.poll_interval_seconds.unwrap_or(15);
    if !(5..=3600).contains(&poll_interval_seconds) {
        bail!("poll_interval_seconds must be between 5 and 3600");
    }
    let health_bind = resolve_or(builder.health_bind, "0.0.0.0:9090", "health_bind", resolver)?
        .parse::<SocketAddr>()
        .context("health_bind must be a socket address")?;
    let state_file = PathBuf::from(resolve_or(
        builder.state_file,
        "/var/lib/infra/state.json",
        "state_file",
        resolver,
    )?);
    if !state_file.is_absolute() {
        bail!("state_file must be absolute");
    }

    let mut input_ids = HashSet::new();
    let mut inputs = Vec::new();
    for input in builder.inputs {
        if !input_ids.insert(input.id.clone()) {
            bail!("duplicate input {}", input.id);
        }
        if input.driver.as_deref() != Some("docker") {
            bail!("input {} driver must be docker", input.id);
        }
        let addresses = input.addresses.context("input.addresses is required")?;
        if addresses.is_empty() {
            bail!("input {} must declare at least one address", input.id);
        }
        for address in &addresses {
            validate_private_or_https_url("input.address", address)?;
        }
        let fields = validate_fields(
            "input.fields",
            input.fields.context("input.fields is required")?,
            ALLOWED_INPUT_FIELDS,
        )?;
        inputs.push(DockerInput {
            id: input.id,
            addresses,
            projects: input.projects.unwrap_or_default().into_iter().collect(),
            services: input.services.unwrap_or_default().into_iter().collect(),
            fields,
        });
    }
    if inputs.is_empty() {
        bail!("agent must declare at least one input");
    }

    if builder.outputs.len() != 1 {
        bail!("agent must declare exactly one output in version 1");
    }
    let output = builder
        .outputs
        .into_iter()
        .next()
        .context("missing output")?;
    if output.driver.as_deref() != Some("infrabot") {
        bail!("output {} driver must be infrabot", output.id);
    }
    let endpoint = resolve_required(output.endpoint, "output.endpoint", resolver)?;
    validate_private_or_https_url("output.endpoint", &endpoint)?;
    let source = output.source.context("output.source is required")?;
    validate_identifier("output source", &source)?;
    let credential = PathBuf::from(resolve_required(
        output.credential,
        "output.credential",
        resolver,
    )?);
    if !credential.is_absolute() {
        bail!("output.credential must be an absolute path");
    }
    let events = output
        .events
        .context("output.events is required")?
        .into_iter()
        .collect::<HashSet<_>>();
    if events.is_empty() {
        bail!("output {} must declare at least one event", output.id);
    }
    let fields = validate_fields_ordered(
        "output.fields",
        output.fields.context("output.fields is required")?,
        ALLOWED_OUTPUT_FIELDS,
    )?;

    Ok(AgentConfig {
        poll_interval: Duration::from_secs(poll_interval_seconds),
        health_bind,
        state_file,
        inputs,
        output: InfraBotOutput {
            id: output.id,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            source,
            credential,
            events,
            fields,
        },
    })
}

fn validate_fields(name: &str, values: Vec<String>, allowed: &[&str]) -> Result<BTreeSet<String>> {
    let ordered = validate_fields_ordered(name, values, allowed)?;
    Ok(ordered.into_iter().collect())
}

fn validate_fields_ordered(
    name: &str,
    values: Vec<String>,
    allowed: &[&str],
) -> Result<Vec<String>> {
    if values.is_empty() {
        bail!("{name} must not be empty");
    }
    let allowed = allowed.iter().copied().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    for value in &values {
        if !allowed.contains(value.as_str()) {
            bail!("{name} contains unknown field {value}");
        }
        if !seen.insert(value.clone()) {
            bail!("{name} contains duplicate field {value}");
        }
    }
    Ok(values)
}

fn validate_private_or_https_url(name: &str, value: &str) -> Result<()> {
    let parsed = Url::parse(value).with_context(|| format!("{name} is not a valid URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("{name} must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("{name} must not contain a query or fragment");
    }
    let host = parsed
        .host_str()
        .with_context(|| format!("{name} has no host"))?;
    if parsed.scheme() == "https" {
        return Ok(());
    }
    if parsed.scheme() != "http" || !private_host(host) {
        bail!("{name} must use HTTPS or private-network HTTP");
    }
    Ok(())
}

fn private_host(host: &str) -> bool {
    if matches!(host, "localhost" | "127.0.0.1" | "::1") || !host.contains('.') {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(value) => value.is_private() || value.is_loopback() || value.is_link_local(),
        IpAddr::V6(value) => {
            value.is_loopback() || value.is_unique_local() || value.is_unicast_link_local()
        }
    })
}

fn named_block(line: &str, kind: &str) -> Option<String> {
    let prefix = format!("{kind} \"");
    let rest = line.strip_prefix(&prefix)?;
    let end = rest.find('"')?;
    (rest[end + 1..].trim() == "{").then(|| rest[..end].to_owned())
}

fn assignment(line_number: usize, line: &str) -> Result<(&str, &str)> {
    let (key, value) = line
        .split_once('=')
        .with_context(|| format!("line {line_number}: expected assignment"))?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        bail!("line {line_number}: incomplete assignment");
    }
    Ok((key, value))
}

fn quoted(line_number: usize, value: &str) -> Result<String> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        bail!("line {line_number}: expected quoted string");
    }
    Ok(value[1..value.len() - 1].to_owned())
}

fn value_ref(line_number: usize, value: &str) -> Result<ValueRef> {
    if let Some(inner) = value
        .strip_prefix("env(\"")
        .and_then(|v| v.strip_suffix("\")"))
    {
        validate_environment_name(inner)
            .with_context(|| format!("line {line_number}: invalid environment reference"))?;
        return Ok(ValueRef::Environment(inner.to_owned()));
    }
    Ok(ValueRef::Literal(quoted(line_number, value)?))
}

fn string_list(line_number: usize, value: &str) -> Result<Vec<String>> {
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .with_context(|| format!("line {line_number}: expected string list"))?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|value| quoted(line_number, value.trim()))
        .collect()
}

fn integer(line_number: usize, value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .with_context(|| format!("line {line_number}: expected unsigned integer"))
}

fn set_once<T>(slot: &mut Option<T>, value: T, line_number: usize, name: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        bail!("line {line_number}: duplicate field {name}");
    }
    Ok(())
}

fn resolve_required<F>(value: Option<ValueRef>, name: &str, resolver: &F) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    match value.context(format!("{name} is required"))? {
        ValueRef::Literal(value) => Ok(value),
        ValueRef::Environment(environment) => resolver(&environment)
            .filter(|value| !value.is_empty())
            .with_context(|| format!("environment variable {environment} for {name} is missing")),
    }
}

fn resolve_or<F>(value: Option<ValueRef>, default: &str, name: &str, resolver: &F) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    match value {
        Some(value) => resolve_required(Some(value), name, resolver),
        None => Ok(default.to_owned()),
    }
}

fn validate_identifier(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        bail!("{name} must contain 1-64 ASCII letters, digits, dots, dashes, or underscores");
    }
    Ok(())
}

fn validate_environment_name(value: &str) -> Result<()> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        bail!("environment names must contain uppercase ASCII letters, digits, or underscores");
    }
    Ok(())
}

fn unix_time() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> &'static str {
        r#"
infra 1

project "infra-agent" {
    runtime = "docker"
    service "agent" {
        source = "."
        build = "Dockerfile.agent"
        expose = 9090
        health = "/health"
    }
}

agent {
    poll_interval_seconds = 15
    health_bind = "0.0.0.0:9090"
    state_file = "/var/lib/infra/state.json"

    input "market-docker" {
        driver = "docker"
        addresses = ["http://docker-api:2375"]
        projects = ["zero-x-da-market-development", "zero-x-da-market-bot-development"]
        services = ["api", "bot"]
        fields = ["input", "docker_address", "container", "project", "service", "image", "state", "health"]
    }

    output "infra_services_bot" {
        driver = "infrabot"
        endpoint = "http://infra-bot:8787"
        source = "vps-spaceship-01"
        credential = env("INFRA_CREDENTIALS_FILE")
        events = ["service.failed", "service.recovered", "service.started", "service.removed"]
        fields = ["kind", "project", "service", "container", "image", "previous_status", "status"]
    }
}
"#
    }

    #[test]
    fn parses_input_and_output_contracts() {
        let config = parse_document(document(), &|name| {
            (name == "INFRA_CREDENTIALS_FILE").then(|| "/run/secrets/infrabot.json".to_owned())
        })
        .unwrap();
        assert_eq!(config.inputs.len(), 1);
        assert_eq!(config.inputs[0].id, "market-docker");
        assert_eq!(config.output.id, "infra_services_bot");
        assert_eq!(config.output.source, "vps-spaceship-01");
    }

    #[test]
    fn rejects_public_plain_http() {
        let input = document().replace("http://docker-api:2375", "http://docker.example:2375");
        assert!(parse_document(&input, &|_| Some("/run/credential.json".into())).is_err());
    }

    #[test]
    fn detects_failure_and_recovery() {
        let healthy = observed("healthy");
        let failed = observed("failed");
        let previous = BTreeMap::from([("docker:id".to_owned(), healthy.clone())]);
        let current = BTreeMap::from([("docker:id".to_owned(), failed.clone())]);
        assert_eq!(transitions(&previous, &current)[0].0, "service.failed");

        let previous = BTreeMap::from([("docker:id".to_owned(), failed)]);
        let current = BTreeMap::from([("docker:id".to_owned(), healthy)]);
        assert_eq!(transitions(&previous, &current)[0].0, "service.recovered");
    }

    fn observed(status: &str) -> ObservedContainer {
        ObservedContainer {
            input: "docker".into(),
            docker_address: "http://docker-api:2375".into(),
            container: "api".into(),
            container_id: "id".into(),
            project: "market".into(),
            service: "api".into(),
            image: "market:latest".into(),
            state: if status == "healthy" {
                "running"
            } else {
                "exited"
            }
            .into(),
            health: if status == "healthy" {
                "healthy"
            } else {
                "none"
            }
            .into(),
            status: status.into(),
        }
    }
}
