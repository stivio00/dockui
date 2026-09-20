use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use bollard::container::Config as ContainerConfig;
use bollard::models::{
    DeviceRequest, HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use serde::Deserialize;

use crate::util::split_command;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub privileged: bool,
    #[serde(default)]
    pub gpus: Option<GpuSpec>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub pid: Option<String>,
    #[serde(default)]
    pub ipc: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<Cmd>,
    #[serde(default)]
    pub command: Option<Cmd>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub restart: Option<String>,
    #[serde(default)]
    pub shm_size: Option<ShmSize>,
    #[serde(default)]
    pub tty: bool,
    /// Run the op like `docker run -it`: create + attach + start, with the
    /// terminal handed to the container until it exits.
    #[serde(default)]
    pub attach: bool,
    #[serde(default)]
    pub rm: bool,
    #[serde(default = "default_true")]
    pub replace: bool,
    #[serde(default = "default_pull")]
    pub pull: PullPolicy,
    #[serde(default)]
    pub follow_logs: bool,
}

fn default_true() -> bool {
    true
}

fn default_pull() -> PullPolicy {
    PullPolicy::Missing
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    #[default]
    Missing,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum GpuSpec {
    Count(i64),
    Spec(String),
}

impl GpuSpec {
    pub fn to_request(&self) -> DeviceRequest {
        let mut req = DeviceRequest {
            driver: Some(String::new()),
            count: None,
            device_ids: None,
            capabilities: Some(vec![vec!["gpu".to_string()]]),
            options: None,
        };
        match self {
            GpuSpec::Spec(s) if s == "all" => req.count = Some(-1),
            GpuSpec::Count(n) => req.count = Some(*n),
            GpuSpec::Spec(s) => {
                req.device_ids = Some(s.split(',').map(|d| d.trim().to_string()).collect());
            }
        }
        req
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Cmd {
    Many(Vec<String>),
    One(String),
}

impl Cmd {
    fn to_args(&self) -> Vec<String> {
        match self {
            Cmd::Many(v) => v.clone(),
            Cmd::One(s) => split_command(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ShmSize {
    Bytes(i64),
    Spec(String),
}

impl ShmSize {
    fn to_bytes(&self) -> i64 {
        match self {
            ShmSize::Bytes(b) => *b,
            ShmSize::Spec(s) => parse_size(s).unwrap_or(64 * 1024 * 1024),
        }
    }
}

pub fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    const K: i64 = 1024;
    let (num, mult) = if let Some(n) = s.strip_suffix("kb") {
        (n, K)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, K)
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, K * K)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, K * K)
    } else if let Some(n) = s.strip_suffix("gb") {
        (n, K * K * K)
    } else if let Some(n) = s.strip_suffix('g') {
        (n, K * K * K)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 1)
    } else {
        (s.as_str(), 1)
    };
    num.trim().parse::<i64>().ok().map(|n| n * mult)
}

/// Expand `${VAR}` references (`$VAR` shorthand included) from the
/// environment; used for op names and env values.
pub fn expand_template(s: &str) -> String {
    expand_with(s, |k| std::env::var(k).ok())
}

/// Template expansion with an explicit variable lookup, so the logic can
/// be tested without touching the process environment.
pub fn expand_with<F: Fn(&str) -> Option<String>>(s: &str, lookup: F) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if after.starts_with('{') {
            let Some(end) = after.find('}') else {
                // unclosed `${` — emit the remainder verbatim and stop
                out.push('$');
                out.push_str(after);
                return out;
            };
            let var = &after[1..end];
            if let Some(v) = lookup(var) {
                out.push_str(&v);
            }
            rest = &after[end + 1..];
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            let var = &after[..end];
            if var.is_empty() {
                out.push('$');
                rest = after;
            } else {
                if let Some(v) = lookup(var) {
                    out.push_str(&v);
                }
                rest = &after[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn ops_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".dui").join("ops.yml"))
}

pub fn load() -> (Vec<(String, Op)>, Option<String>) {
    let Some(path) = ops_path() else {
        return (Vec::new(), Some("$HOME not set".to_string()));
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return (Vec::new(), None);
    };
    match parse_ops(&raw) {
        Ok(mut ops) => {
            ops.sort_by(|a, b| a.0.cmp(&b.0));
            (ops, None)
        }
        Err(e) => (Vec::new(), Some(format!("{}: {e:#}", path.display()))),
    }
}

/// Parse an ops document. Both a flat `name: {op}` mapping and one wrapped
/// in a top-level `ops:` key are accepted; a flat document wins when it
/// parses cleanly, so an op may legitimately be named `ops`.
pub fn parse_ops(raw: &str) -> anyhow::Result<Vec<(String, Op)>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(raw).context("invalid ops.yml")?;
    let map = match doc {
        serde_yaml::Value::Null => return Ok(Vec::new()),
        serde_yaml::Value::Mapping(m) => m,
        _ => bail!("ops.yml must be a mapping of op name -> definition"),
    };
    if let Ok(ops) = parse_op_entries(&map) {
        return Ok(ops);
    }
    if let Some(serde_yaml::Value::Mapping(inner)) =
        map.get(serde_yaml::Value::String("ops".into()))
        && let Ok(ops) = parse_op_entries(inner)
    {
        return Ok(ops);
    }
    // surface the flat-document error, which is the common case
    parse_op_entries(&map)
}

fn parse_op_entries(map: &serde_yaml::Mapping) -> anyhow::Result<Vec<(String, Op)>> {
    let mut out = Vec::new();
    for (k, v) in map {
        let serde_yaml::Value::String(name) = k else {
            bail!("op names must be strings");
        };
        if name.trim().is_empty() {
            bail!("op names must not be empty");
        }
        let op: Op =
            serde_yaml::from_value(v.clone()).with_context(|| format!("op '{name}' is invalid"))?;
        if op.image.as_deref().map(str::trim).unwrap_or("").is_empty() {
            bail!("op '{name}' is missing required field 'image'");
        }
        out.push((name.clone(), op));
    }
    Ok(out)
}

/// Everything needed to (re)create a container, captured from an inspect
/// response or an ops definition.
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub name: Option<String>,
    pub image: Option<String>,
    pub cmd: Option<Vec<String>>,
    pub entrypoint: Option<Vec<String>>,
    pub env: Vec<String>,
    pub labels: HashMap<String, String>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    pub tty: bool,
    pub host: HostConfig,
}

impl ContainerSpec {
    pub fn from_inspect(inspect: &bollard::models::ContainerInspectResponse) -> Self {
        let cfg = inspect.config.as_ref();
        ContainerSpec {
            name: inspect
                .name
                .as_deref()
                .map(|n| n.trim_start_matches('/').to_string()),
            image: cfg.and_then(|c| c.image.clone()),
            cmd: cfg.and_then(|c| c.cmd.clone()),
            entrypoint: cfg.and_then(|c| c.entrypoint.clone()),
            env: cfg.and_then(|c| c.env.clone()).unwrap_or_default(),
            labels: cfg.and_then(|c| c.labels.clone()).unwrap_or_default(),
            user: cfg.and_then(|c| c.user.clone()),
            working_dir: cfg.and_then(|c| c.working_dir.clone()),
            tty: cfg.and_then(|c| c.tty).unwrap_or(false),
            host: inspect.host_config.clone().unwrap_or_default(),
        }
    }

    pub fn to_config(&self) -> ContainerConfig<String> {
        let exposed: HashMap<String, HashMap<(), ()>> = self
            .host
            .port_bindings
            .as_ref()
            .map(|pm| pm.keys().map(|k| (k.clone(), HashMap::new())).collect())
            .unwrap_or_default();
        ContainerConfig {
            image: self.image.clone(),
            env: (!self.env.is_empty()).then(|| self.env.clone()),
            cmd: self.cmd.clone(),
            entrypoint: self.entrypoint.clone(),
            working_dir: self.working_dir.clone(),
            user: self.user.clone(),
            tty: Some(self.tty),
            labels: (!self.labels.is_empty()).then(|| self.labels.clone()),
            exposed_ports: (!exposed.is_empty()).then_some(exposed),
            host_config: Some(self.host.clone()),
            ..Default::default()
        }
    }
}

/// A request to recreate a container with modifications: create + start the
/// new one, then remove the old container.
#[derive(Debug, Clone)]
pub struct RecreateRequest {
    pub old_id: String,
    pub old_name: String,
    pub was_running: bool,
    pub spec: ContainerSpec,
}

impl Op {
    /// Container name with templates expanded.
    pub fn container_name(&self) -> Option<String> {
        self.name.as_deref().map(expand_template)
    }

    pub fn image(&self) -> &str {
        self.image.as_deref().unwrap_or_default()
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![self.image().to_string()];
        if self.privileged {
            parts.push("--privileged".into());
        }
        if let Some(g) = &self.gpus {
            parts.push(match g {
                GpuSpec::Spec(s) => format!("--gpus {s}"),
                GpuSpec::Count(n) => format!("--gpus {n}"),
            });
        }
        for (v, flag) in [
            (&self.network, "--net"),
            (&self.pid, "--pid"),
            (&self.ipc, "--ipc"),
        ] {
            if let Some(v) = v {
                parts.push(format!("{flag} {v}"));
            }
        }
        if self.attach {
            parts.push("-it".into());
        }
        parts.join(" ")
    }

    /// Build the create-container request: (name, config).
    pub fn to_create(&self) -> (Option<String>, ContainerConfig<String>) {
        let mut host = HostConfig {
            privileged: Some(self.privileged),
            ..Default::default()
        };
        if let Some(g) = &self.gpus {
            host.device_requests = Some(vec![g.to_request()]);
        }
        if !self.volumes.is_empty() {
            host.binds = Some(self.volumes.clone());
        }
        let mut exposed: HashMap<String, HashMap<(), ()>> = HashMap::new();
        let mut bindings: bollard::models::PortMap = HashMap::new();
        for spec in &self.ports {
            if let Some((key, binding)) = parse_port(spec) {
                exposed.insert(key.clone(), HashMap::new());
                if let Some(v) = bindings.entry(key).or_insert_with(|| Some(Vec::new())) {
                    v.push(binding);
                }
            }
        }
        if !bindings.is_empty() {
            host.port_bindings = Some(bindings);
        }
        host.network_mode = self.network.clone();
        host.pid_mode = self.pid.clone();
        host.ipc_mode = self.ipc.clone();
        host.auto_remove = Some(self.rm);
        if let Some(r) = self.restart_policy() {
            host.restart_policy = Some(r);
        }
        if let Some(shm) = &self.shm_size {
            host.shm_size = Some(shm.to_bytes());
        }

        let env: Vec<String> = self.env.iter().map(|e| expand_template(e)).collect();
        let config = ContainerConfig {
            image: Some(self.image().to_string()),
            env: (!env.is_empty()).then_some(env),
            cmd: self.command.as_ref().map(|c| c.to_args()),
            entrypoint: self.entrypoint.as_ref().map(|c| c.to_args()),
            working_dir: self.workdir.clone(),
            user: self.user.clone(),
            tty: Some(self.tty || self.attach),
            open_stdin: self.attach.then_some(true),
            attach_stdin: self.attach.then_some(true),
            attach_stdout: self.attach.then_some(true),
            attach_stderr: self.attach.then_some(true),
            labels: (!self.labels.is_empty()).then(|| self.labels.clone()),
            exposed_ports: (!exposed.is_empty()).then_some(exposed),
            host_config: Some(host),
            ..Default::default()
        };
        (self.container_name(), config)
    }

    fn restart_policy(&self) -> Option<RestartPolicy> {
        let name = self.restart.as_deref()?;
        let (policy, max_retry) = match name {
            "always" => (RestartPolicyNameEnum::ALWAYS, None),
            "unless-stopped" => (RestartPolicyNameEnum::UNLESS_STOPPED, None),
            "on-failure" => (RestartPolicyNameEnum::ON_FAILURE, None),
            other => {
                let n = other.strip_prefix("on-failure:")?;
                (
                    RestartPolicyNameEnum::ON_FAILURE,
                    n.parse::<i64>().ok().map(|x| x.clamp(0, 100)),
                )
            }
        };
        Some(RestartPolicy {
            name: Some(policy),
            maximum_retry_count: max_retry,
        })
    }
}

/// Parse a docker-style port spec `[host_ip:]host_port:container_port[/proto]`.
pub fn parse_port(spec: &str) -> Option<(String, PortBinding)> {
    let spec = spec.trim();
    let (container, proto) = match spec.rsplit_once('/') {
        Some((c, p)) if p.eq_ignore_ascii_case("tcp") || p.eq_ignore_ascii_case("udp") => {
            (c, p.to_ascii_lowercase())
        }
        Some(_) => return None,
        None => (spec, "tcp".to_string()),
    };
    // [host:]container | ip:host:container — anything else is invalid
    let parts: Vec<&str> = container.split(':').collect();
    let (host_ip, host_port, cport) = match parts.as_slice() {
        [hp, cp] => (None, *hp, *cp),
        [ip, hp, cp] => (Some(ip.to_string()), *hp, *cp),
        _ => return None,
    };
    if host_ip.as_deref() == Some("") {
        return None;
    }
    // single ports or simple ranges, numeric only
    let numeric = |p: &str| !p.is_empty() && p.split('-').all(|n| n.parse::<u16>().is_ok());
    if !numeric(host_port) || !numeric(cport) {
        return None;
    }
    Some((
        format!("{cport}/{proto}"),
        PortBinding {
            host_ip,
            host_port: Some(host_port.to_string()),
        },
    ))
}
