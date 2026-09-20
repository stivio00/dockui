use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, anyhow, bail};
use bollard::Docker;
use serde::Deserialize;

use crate::model::Container;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointKind {
    Unix,
    Tcp,
    Ssh,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct DockerContext {
    pub name: String,
    pub endpoint: String,
    pub kind: EndpointKind,
    pub current: bool,
}

impl DockerContext {
    pub fn kind_of(endpoint: &str) -> EndpointKind {
        if endpoint.starts_with("unix://") || endpoint.starts_with('/') {
            EndpointKind::Unix
        } else if endpoint.starts_with("tcp://") || endpoint.starts_with("http://") {
            EndpointKind::Tcp
        } else if endpoint.starts_with("ssh://") {
            EndpointKind::Ssh
        } else {
            EndpointKind::Unknown
        }
    }

    pub fn new(name: impl Into<String>, endpoint: impl Into<String>, current: bool) -> Self {
        let endpoint = endpoint.into();
        let kind = Self::kind_of(&endpoint);
        DockerContext {
            name: name.into(),
            endpoint,
            kind,
            current,
        }
    }
}

#[derive(Deserialize, Debug)]
struct ContextMeta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    endpoints: Option<HashMap<String, ContextEndpoint>>,
}

#[derive(Deserialize, Debug)]
struct ContextEndpoint {
    #[serde(default)]
    host: Option<String>,
}

#[derive(Deserialize, Debug)]
struct DockerConfig {
    #[serde(default)]
    current_context: Option<String>,
}

/// Discover docker contexts from `~/.docker`, honouring `DOCKER_HOST`.
pub fn list_contexts() -> anyhow::Result<Vec<DockerContext>> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let docker_dir = Path::new(&home).join(".docker");
    let current_from_cfg = read_current_context(&docker_dir);
    let env_host = std::env::var("DOCKER_HOST").ok().filter(|h| !h.is_empty());

    let mut found: Vec<DockerContext> = Vec::new();
    let meta_dir = docker_dir.join("contexts").join("meta");
    if let Ok(entries) = std::fs::read_dir(&meta_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            let Ok(raw) = std::fs::read_to_string(&meta_path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<ContextMeta>(&raw) else {
                continue;
            };
            let Some(name) = meta.name else { continue };
            let host = meta
                .endpoints
                .and_then(|e| e.get("docker").and_then(|d| d.host.clone()))
                .unwrap_or_else(|| "unix:///var/run/docker.sock".to_string());
            found.push(DockerContext::new(name, host, false));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    if found.is_empty() {
        found.push(DockerContext::new(
            "default",
            "unix:///var/run/docker.sock",
            false,
        ));
    }

    let effective = env_host
        .is_none()
        .then(|| current_from_cfg.unwrap_or_else(|| "default".into()));
    for ctx in &mut found {
        ctx.current = match (&effective, &env_host) {
            (Some(cur), _) => &ctx.name == cur,
            (None, Some(host)) => ctx.kind == EndpointKind::Unix && ctx.endpoint == *host,
            _ => false,
        };
    }
    if let Some(host) = env_host {
        let name = if host == "unix:///var/run/docker.sock" {
            "default".to_string()
        } else {
            "DOCKER_HOST".to_string()
        };
        if let Some(existing) = found.iter_mut().find(|c| c.name == name) {
            existing.endpoint = host.clone();
            existing.kind = DockerContext::kind_of(&host);
            existing.current = true;
        } else {
            found.push(DockerContext::new(name, host, true));
        }
    }
    // If nothing matched (stale config), mark the first as current.
    if !found.iter().any(|c| c.current)
        && let Some(first) = found.first_mut()
    {
        first.current = true;
    }
    Ok(found)
}

fn read_current_context(docker_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(docker_dir.join("config.json")).ok()?;
    let cfg: DockerConfig = serde_json::from_str(&raw).ok()?;
    cfg.current_context
}

pub struct Tunnel {
    socket_path: PathBuf,
    child: tokio::process::Child,
}

impl Tunnel {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

const CONNECT_TIMEOUT: u64 = 24 * 60 * 60;

pub struct Connection {
    pub docker: Docker,
    pub tunnel: Option<Tunnel>,
}

fn parse_ssh_endpoint(endpoint: &str) -> anyhow::Result<(String, Option<String>, String)> {
    // ssh://[user@]host[:port][/remote/socket]
    let rest = endpoint
        .strip_prefix("ssh://")
        .ok_or_else(|| anyhow!("not an ssh endpoint"))?;
    let (rest, remote_sock) = match rest.split_once('/') {
        Some((h, path)) => (h, format!("/{path}")),
        None => (rest, "/var/run/docker.sock".to_string()),
    };
    let (userhost, port) = match rest.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.to_string(), Some(p.to_string()))
        }
        _ => (rest.to_string(), None),
    };
    let user_host = userhost
        .split_once('@')
        .map(|(u, h)| (Some(u.to_string()), h.to_string()))
        .unwrap_or((None, userhost.clone()));
    let target = match user_host.0 {
        Some(ref u) => format!("{u}@{}", user_host.1),
        None => user_host.1.clone(),
    };
    if target.is_empty() {
        bail!("empty ssh target");
    }
    Ok((target, port, remote_sock))
}

async fn connect_ssh(endpoint: &str, ctx_name: &str) -> anyhow::Result<Tunnel> {
    let (target, port, remote_sock) = parse_ssh_endpoint(endpoint)?;
    let safe: String = ctx_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let socket_path = std::env::temp_dir().join(format!("dockui-docker-{safe}.sock"));
    let _ = std::fs::remove_file(&socket_path);

    let mut cmd = tokio::process::Command::new("ssh");
    cmd.arg("-N")
        .arg("-L")
        .arg(format!("{}:{}", socket_path.display(), remote_sock))
        .args([
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
        ]);
    if let Some(p) = &port {
        cmd.arg("-p").arg(p);
    }
    cmd.arg(&target);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.kill_on_drop(true);
    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn ssh tunnel to {target}"))?;

    let deadline = std::time::Duration::from_secs(12);
    let start = std::time::Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > deadline {
            bail!("ssh tunnel to {target} did not come up within 12s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(Tunnel { socket_path, child })
}

pub async fn connect(ctx: &DockerContext) -> anyhow::Result<Connection> {
    let docker = match ctx.kind {
        EndpointKind::Unix => {
            let path = ctx
                .endpoint
                .strip_prefix("unix://")
                .map(|p| p.to_string())
                .unwrap_or_else(|| ctx.endpoint.clone());
            if !Path::new(&path).exists() {
                bail!("docker socket not found: {path}");
            }
            Docker::connect_with_unix(&path, CONNECT_TIMEOUT, bollard::API_DEFAULT_VERSION)
                .map_err(|e| anyhow!("unix connect failed: {e}"))?
        }
        EndpointKind::Tcp => {
            Docker::connect_with_http(&ctx.endpoint, CONNECT_TIMEOUT, bollard::API_DEFAULT_VERSION)
                .map_err(|e| anyhow!("tcp connect failed: {e}"))?
        }
        EndpointKind::Ssh => {
            let tunnel = connect_ssh(&ctx.endpoint, &ctx.name).await?;
            let sock = tunnel.socket_path().display().to_string();
            let docker =
                Docker::connect_with_unix(&sock, CONNECT_TIMEOUT, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| anyhow!("tunnel connect failed: {e}"))?;
            let conn = Connection {
                docker,
                tunnel: Some(tunnel),
            };
            ping(&conn.docker).await?;
            return Ok(conn);
        }
        EndpointKind::Unknown => bail!("unsupported endpoint: {}", ctx.endpoint),
    };
    ping(&docker).await?;
    Ok(Connection {
        docker,
        tunnel: None,
    })
}

pub async fn ping(docker: &Docker) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), docker.ping())
        .await
        .map_err(|_| anyhow!("ping timed out"))?
        .map_err(|e| anyhow!("docker daemon unreachable: {e}"))?;
    Ok(())
}

pub async fn list_containers(docker: &Docker) -> anyhow::Result<Vec<Container>> {
    use bollard::container::ListContainersOptions;
    let opts = ListContainersOptions::<String> {
        all: true,
        limit: None,
        size: false,
        filters: HashMap::new(),
    };
    let list = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        docker.list_containers(Some(opts)),
    )
    .await
    .map_err(|_| anyhow!("container list timed out"))??;

    Ok(list
        .into_iter()
        .map(|c| {
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();
            let ports = c
                .ports
                .unwrap_or_default()
                .iter()
                .filter_map(|p| {
                    p.public_port.map(|pubp| {
                        format!(
                            "{}:{}->{}:{}",
                            p.ip.as_deref().unwrap_or("0.0.0.0"),
                            pubp,
                            p.private_port,
                            p.typ.map(|t| t.to_string()).unwrap_or_default()
                        )
                    })
                })
                .collect();
            Container {
                id: c.id.unwrap_or_default(),
                name,
                image: c.image.unwrap_or_default(),
                state: c.state.unwrap_or_else(|| "unknown".into()),
                status: c.status.unwrap_or_default(),
                created: c.created.unwrap_or_default(),
                command: c.command.unwrap_or_default(),
                labels: c.labels.unwrap_or_default(),
                ports,
            }
        })
        .collect())
}
