use std::collections::HashMap;
use std::time::Duration;

use bollard::Docker;
use bollard::container::{
    LogOutput, LogsOptions, RemoveContainerOptions, RestartContainerOptions, Stats, StatsOptions,
    StopContainerOptions,
};
use bollard::models::{EventMessage, EventMessageTypeEnum, VolumeScopeEnum};
use bollard::system::EventsOptions;
use bollard::volume::ListVolumesOptions;
use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use crate::actions::ContainerAction;
use crate::model::{Container, ContainerDetails, DockerEvent, StatsSample, Volume};
use crate::ops::{Op, PullPolicy, RecreateRequest};

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    ConnOk {
        docker: Docker,
    },
    ConnMock,
    ConnErr(String),
    Containers(Vec<Container>),
    Volumes(Vec<Volume>),
    Event(DockerEvent),
    Stats {
        id: String,
        sample: StatsSample,
    },
    LogLine {
        name: String,
        line: String,
    },
    Inspected {
        id: String,
        details: ContainerDetails,
    },
    ActionDone {
        label: String,
        ok: bool,
        message: String,
        open_logs: Option<LogTarget>,
    },
    FilesListed {
        req: u64,
        result: Result<Vec<crate::files::FsEntry>, String>,
    },
}

#[derive(Default)]
pub struct Workers {
    handles: std::sync::Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
}

impl Workers {
    pub fn shared(&self) -> Workers {
        Workers {
            handles: self.handles.clone(),
        }
    }
    pub fn add(&self, h: JoinHandle<()>) {
        self.handles.lock().unwrap().push(h);
    }
    pub fn abort_all(&self) {
        let mut hs = self.handles.lock().unwrap();
        for h in hs.drain(..) {
            h.abort();
        }
    }
}

pub fn spawn_container_poll(
    docker: Docker,
    tx: Sender<Msg>,
    notify: std::sync::Arc<tokio::sync::Notify>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match crate::docker::list_containers(&docker).await {
                Ok(list) => {
                    if tx.send(Msg::Containers(list)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    if tx.send(Msg::ConnErr(e.to_string())).await.is_err() {
                        return;
                    }
                }
            }
            tokio::select! {
                _ = notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    })
}

pub fn spawn_volume_poll(docker: Docker, tx: Sender<Msg>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let opts = ListVolumesOptions::<String> {
                filters: HashMap::new(),
            };
            let res =
                tokio::time::timeout(Duration::from_secs(10), docker.list_volumes(Some(opts)))
                    .await;
            match res {
                Ok(Ok(vols)) => {
                    let vols: Vec<Volume> = vols
                        .volumes
                        .unwrap_or_default()
                        .into_iter()
                        .map(|v| {
                            let usage = v.usage_data.as_ref();
                            Volume {
                                name: v.name,
                                driver: v.driver,
                                scope: v
                                    .scope
                                    .map(|s| match s {
                                        VolumeScopeEnum::LOCAL => "local",
                                        VolumeScopeEnum::GLOBAL => "global",
                                        VolumeScopeEnum::EMPTY => "",
                                    })
                                    .unwrap_or("local")
                                    .to_string(),
                                mountpoint: v.mountpoint,
                                labels: v.labels,
                                size: usage.map(|u| u.size.max(0) as u64),
                                ref_count: usage.map(|u| u.ref_count.max(0) as u64),
                            }
                        })
                        .collect();
                    if tx.send(Msg::Volumes(vols)).await.is_err() {
                        return;
                    }
                }
                Ok(Err(e)) => {
                    if tx.send(Msg::ConnErr(e.to_string())).await.is_err() {
                        return;
                    }
                }
                Err(_) => {
                    if tx
                        .send(Msg::ConnErr("volume list timed out".into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}

pub fn spawn_events(docker: Docker, tx: Sender<Msg>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut since = chrono::Utc::now().timestamp() - 60;
        loop {
            let opts = EventsOptions::<String> {
                since: Some(since.to_string()),
                until: None,
                filters: HashMap::new(),
            };
            let mut stream = docker.events(Some(opts));
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ev) => {
                        since = chrono::Utc::now().timestamp();
                        if tx.send(Msg::Event(convert_event(&ev))).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if tx.send(Msg::ConnErr(e.to_string())).await.is_err() {
                            return;
                        }
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

fn convert_event(ev: &EventMessage) -> DockerEvent {
    let typ = match &ev.typ {
        Some(EventMessageTypeEnum::CONTAINER) => "container",
        Some(EventMessageTypeEnum::IMAGE) => "image",
        Some(EventMessageTypeEnum::VOLUME) => "volume",
        Some(EventMessageTypeEnum::NETWORK) => "network",
        Some(EventMessageTypeEnum::DAEMON) => "daemon",
        Some(EventMessageTypeEnum::PLUGIN) => "plugin",
        Some(EventMessageTypeEnum::BUILDER) => "builder",
        Some(EventMessageTypeEnum::CONFIG) => "config",
        Some(EventMessageTypeEnum::NODE) => "node",
        Some(EventMessageTypeEnum::SECRET) => "secret",
        Some(EventMessageTypeEnum::SERVICE) => "service",
        Some(EventMessageTypeEnum::EMPTY) | None => "?",
    };
    let (actor_id, actor_name) = ev
        .actor
        .as_ref()
        .map(|a| {
            let id = a.id.clone().unwrap_or_default();
            let name = a
                .attributes
                .as_ref()
                .and_then(|attrs| {
                    attrs
                        .get("name")
                        .or_else(|| attrs.get("container"))
                        .cloned()
                })
                .unwrap_or_else(|| id.chars().take(12).collect());
            (id, name)
        })
        .unwrap_or_default();
    DockerEvent {
        time: ev.time.unwrap_or_default(),
        typ: typ.to_string(),
        action: ev.action.clone().unwrap_or_default(),
        actor_id,
        actor_name,
        scope: ev.scope.as_ref().map(|s| s.to_string()).unwrap_or_default(),
    }
}

impl From<&Stats> for StatsSample {
    fn from(s: &Stats) -> StatsSample {
        let cpu_delta =
            s.cpu_stats.cpu_usage.total_usage as f64 - s.precpu_stats.cpu_usage.total_usage as f64;
        let sys_delta = s.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
            - s.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
        let ncpu = s
            .cpu_stats
            .online_cpus
            .map(|c| c.max(1) as f64)
            .or_else(|| {
                s.cpu_stats
                    .cpu_usage
                    .percpu_usage
                    .as_ref()
                    .map(|v| v.len().max(1) as f64)
            })
            .unwrap_or(1.0);
        let cpu_pct = if sys_delta > 0.0 && cpu_delta >= 0.0 {
            (cpu_delta / sys_delta) * ncpu * 100.0
        } else {
            0.0
        };
        let mem = s.memory_stats.usage.unwrap_or(0);
        let mem_limit = s.memory_stats.limit.unwrap_or(0);
        let mem_pct = if mem_limit > 0 {
            mem as f64 / mem_limit as f64 * 100.0
        } else {
            0.0
        };
        let mut net_rx = 0u64;
        let mut net_tx = 0u64;
        if let Some(nets) = &s.networks {
            for n in nets.values() {
                net_rx += n.rx_bytes;
                net_tx += n.tx_bytes;
            }
        } else if let Some(n) = &s.network {
            net_rx = n.rx_bytes;
            net_tx = n.tx_bytes;
        }
        let mut blk_read = 0u64;
        let mut blk_write = 0u64;
        if let Some(entries) = &s.blkio_stats.io_service_bytes_recursive {
            for e in entries {
                match e.op.as_str() {
                    "read" => blk_read += e.value,
                    "write" => blk_write += e.value,
                    _ => {}
                }
            }
        }
        StatsSample {
            cpu_pct,
            mem,
            mem_limit,
            mem_pct,
            net_rx,
            net_tx,
            blk_read,
            blk_write,
            pids: s.pids_stats.current,
        }
    }
}

pub fn spawn_stats_streams(
    docker: Docker,
    tx: Sender<Msg>,
    ids: Vec<String>,
) -> Vec<JoinHandle<()>> {
    ids.into_iter()
        .map(|id| {
            let docker = docker.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let opts = StatsOptions {
                    stream: true,
                    one_shot: false,
                };
                let mut stream = docker.stats(&id, Some(opts));
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(stats) => {
                            let sample = StatsSample::from(&stats);
                            if tx
                                .send(Msg::Stats {
                                    id: id.clone(),
                                    sample,
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct LogTarget {
    pub id: String,
    pub name: String,
}

pub fn spawn_logs(
    docker: Docker,
    tx: Sender<Msg>,
    targets: Vec<LogTarget>,
    tail: usize,
) -> Vec<JoinHandle<()>> {
    targets
        .into_iter()
        .map(|t| {
            let docker = docker.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let opts = LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    since: 0,
                    until: 0,
                    timestamps: true,
                    tail: tail.to_string(),
                };
                let mut stream = docker.logs(&t.id, Some(opts));
                // docker multiplexes frames; a single line may be split
                // across frames, so buffer until a newline arrives
                let mut pending = String::new();
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(LogOutput::StdOut { message }) | Ok(LogOutput::StdErr { message }) => {
                            pending.push_str(&String::from_utf8_lossy(&message));
                            while let Some(nl) = pending.find('\n') {
                                let line: String = pending.drain(..=nl).collect();
                                let line = line.trim_end_matches('\r');
                                if line.is_empty() {
                                    continue;
                                }
                                if tx
                                    .send(Msg::LogLine {
                                        name: t.name.clone(),
                                        line: line.to_string(),
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(_) => return,
                    }
                }
                let rest = pending.trim_end_matches('\r');
                if !rest.is_empty() {
                    let _ = tx
                        .send(Msg::LogLine {
                            name: t.name.clone(),
                            line: rest.to_string(),
                        })
                        .await;
                }
            })
        })
        .collect()
}

/// List one directory of a container's filesystem via the docker archive
/// API (works on stopped containers too).
pub fn spawn_files_list(
    docker: Docker,
    tx: Sender<Msg>,
    req: u64,
    id: String,
    path: String,
) -> JoinHandle<()> {
    use bollard::container::DownloadFromContainerOptions;

    tokio::spawn(async move {
        use futures_util::StreamExt;
        let stream = docker.download_from_container(
            &id,
            Some(DownloadFromContainerOptions::<String> {
                path: path.to_string(),
            }),
        );
        let mut data: Vec<u8> = Vec::new();
        let mut stream = std::pin::pin!(stream);
        let mut result = Ok(Vec::new());
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => {
                    result = Err(e.to_string());
                    break;
                }
            }
        }
        if result.is_ok() {
            result = crate::files::parse_tar_listing(&data);
        }
        let _ = tx.send(Msg::FilesListed { req, result }).await;
    })
}

pub fn spawn_inspect(docker: Docker, tx: Sender<Msg>, id: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let res =
            tokio::time::timeout(Duration::from_secs(10), docker.inspect_container(&id, None))
                .await;
        let details = match res {
            Ok(Ok(inspect)) => convert_inspect(&inspect),
            Ok(Err(e)) => {
                let _ = tx
                    .send(Msg::ConnErr(format!("inspect {id:.12}: {e}")))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx
                    .send(Msg::ConnErr(format!("inspect {id:.12}: timed out")))
                    .await;
                return;
            }
        };
        let _ = tx.send(Msg::Inspected { id, details }).await;
    })
}

fn restart_policy_text(p: &Option<bollard::models::RestartPolicy>) -> Option<String> {
    use bollard::models::RestartPolicyNameEnum;
    let p = p.as_ref()?;
    let name = p
        .name
        .map(|n| match n {
            RestartPolicyNameEnum::ALWAYS => "always",
            RestartPolicyNameEnum::UNLESS_STOPPED => "unless-stopped",
            RestartPolicyNameEnum::ON_FAILURE => "on-failure",
            RestartPolicyNameEnum::NO => "no",
            RestartPolicyNameEnum::EMPTY => "",
        })
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    Some(match p.maximum_retry_count {
        Some(n) if n > 0 => format!("{name}:{n}"),
        _ => name.to_string(),
    })
}

fn convert_inspect(inspect: &bollard::models::ContainerInspectResponse) -> ContainerDetails {
    use bollard::models::MountPointTypeEnum;

    let cfg = inspect.config.as_ref();
    let nets = inspect.network_settings.as_ref();
    let host = inspect.host_config.clone().unwrap_or_default();
    let mut ports: Vec<String> = Vec::new();
    if let Some(pm) = nets.and_then(|n| n.ports.as_ref()) {
        for (container_port, bindings) in pm {
            if let Some(bindings) = bindings {
                for b in bindings {
                    ports.push(format!(
                        "{}:{} -> {container_port}",
                        b.host_ip.as_deref().unwrap_or("0.0.0.0"),
                        b.host_port.as_deref().unwrap_or("?")
                    ));
                }
            } else {
                ports.push(format!("{container_port} (unpublished)"));
            }
        }
    }
    let mounts = inspect
        .mounts
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| {
                    let typ = match m.typ {
                        Some(MountPointTypeEnum::BIND) => "bind",
                        Some(MountPointTypeEnum::VOLUME) => "volume",
                        Some(MountPointTypeEnum::TMPFS) => "tmpfs",
                        Some(MountPointTypeEnum::NPIPE) => "npipe",
                        Some(MountPointTypeEnum::CLUSTER)
                        | Some(MountPointTypeEnum::EMPTY)
                        | None => "?",
                    };
                    let rw = if m.rw.unwrap_or(false) { "rw" } else { "ro" };
                    let src = match (m.name.as_deref(), m.source.as_deref()) {
                        (Some(n), Some(s)) => format!("{s} [{n}]"),
                        (Some(n), None) => n.to_string(),
                        _ => m.source.clone().unwrap_or_else(|| "?".into()),
                    };
                    format!(
                        "{} -> {} ({typ}, {rw})",
                        src,
                        m.destination.as_deref().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let networks = nets
        .and_then(|n| n.networks.as_ref())
        .map(|ns| ns.keys().cloned().collect())
        .unwrap_or_default();
    ContainerDetails {
        env: cfg.and_then(|c| c.env.clone()).unwrap_or_default(),
        ports,
        mounts,
        networks,
        ip: nets
            .and_then(|n| n.ip_address.clone())
            .filter(|ip| !ip.is_empty()),
        cmd: cfg
            .and_then(|c| c.cmd.clone())
            .map(|c| c.join(" "))
            .unwrap_or_default(),
        entrypoint: cfg
            .and_then(|c| c.entrypoint.clone())
            .map(|c| c.join(" "))
            .unwrap_or_default(),
        working_dir: cfg.and_then(|c| c.working_dir.clone()),
        started_at: inspect.state.as_ref().and_then(|s| s.started_at.clone()),
        finished_at: inspect.state.as_ref().and_then(|s| s.finished_at.clone()),
        exit_code: inspect.state.as_ref().and_then(|s| s.exit_code),
        restart_count: inspect.restart_count,
        health: inspect.state.as_ref().and_then(|s| {
            s.health
                .as_ref()
                .and_then(|h| h.status.map(|st| st.to_string()))
        }),
        log_path: inspect.log_path.clone(),
        user: cfg.and_then(|c| c.user.clone()),
        privileged: host.privileged.unwrap_or(false),
        gpus: crate::actions::gpu_summary(&host),
        restart_policy: restart_policy_text(&host.restart_policy),
        network_mode: host.network_mode,
        pid_mode: host.pid_mode,
        ipc_mode: host.ipc_mode,
        auto_remove: host.auto_remove.unwrap_or(false),
        shm_size: host.shm_size,
        spec: Some(crate::ops::ContainerSpec::from_inspect(inspect)),
    }
}

fn is_not_found(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

async fn pull_image(docker: &Docker, image: &str) -> Result<(), String> {
    let opts = bollard::image::CreateImageOptions {
        from_image: image.to_string(),
        ..Default::default()
    };
    let mut stream = docker.create_image(Some(opts), None, None);
    while let Some(item) = stream.next().await {
        match item {
            Ok(info) => {
                if let Some(err) = info.error
                    && !err.is_empty()
                {
                    return Err(err);
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

async fn create_with_pull(
    docker: &Docker,
    name: Option<String>,
    config: bollard::container::Config<String>,
    pull: PullPolicy,
) -> Result<(String, Option<String>), String> {
    let create = || async {
        let opts = bollard::container::CreateContainerOptions {
            name: name.clone().unwrap_or_default(),
            platform: None,
        };
        docker
            .create_container(Some(opts), config.clone())
            .await
            .map(|r| (r.id, name.clone()))
    };
    match create().await {
        Ok(res) => Ok(res),
        Err(e) if is_not_found(&e) && pull != PullPolicy::Never => {
            let image = config.image.clone().unwrap_or_default();
            pull_image(docker, &image).await?;
            create()
                .await
                .map_err(|e| format!("create after pull failed: {e}"))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Run a one-shot lifecycle action (start/stop/restart/remove) on a container.
pub fn spawn_container_action(
    docker: Docker,
    tx: Sender<Msg>,
    id: String,
    name: String,
    action: ContainerAction,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let res = match action {
            ContainerAction::Start => {
                docker
                    .start_container(
                        &id,
                        None::<bollard::container::StartContainerOptions<String>>,
                    )
                    .await
            }
            ContainerAction::Stop => {
                docker
                    .stop_container(&id, Some(StopContainerOptions { t: 5 }))
                    .await
            }
            ContainerAction::Restart => {
                docker
                    .restart_container(&id, Some(RestartContainerOptions { t: 5 }))
                    .await
            }
            ContainerAction::Remove => {
                docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            v: false,
                            force: true,
                            link: false,
                        }),
                    )
                    .await
            }
        };
        let (ok, message) = match res {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e.to_string()),
        };
        let _ = tx
            .send(Msg::ActionDone {
                label: format!("{} {}", action.label(), name),
                ok,
                message,
                open_logs: None,
            })
            .await;
    })
}

/// Recreate a container with modifications: create + start the new container,
/// then remove the old one (removed first if the name is unchanged).
pub fn spawn_recreate(docker: Docker, tx: Sender<Msg>, req: RecreateRequest) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = recreate(&docker, &req).await;
        let (ok, message, created) = match result {
            Ok(created) => (true, String::new(), created),
            Err(e) => (false, e, None),
        };
        let open_logs = req
            .was_running
            .then(|| created.map(|(id, name)| LogTarget { id, name }))
            .flatten();
        let _ = tx
            .send(Msg::ActionDone {
                label: format!(
                    "recreate {}",
                    req.spec.name.as_deref().unwrap_or(&req.old_name)
                ),
                ok,
                message,
                open_logs,
            })
            .await;
    })
}

async fn recreate(
    docker: &Docker,
    req: &RecreateRequest,
) -> Result<Option<(String, String)>, String> {
    let same_name = req.spec.name.as_deref().is_some_and(|n| n == req.old_name);
    if same_name {
        docker
            .remove_container(
                &req.old_id,
                Some(RemoveContainerOptions {
                    v: false,
                    force: true,
                    link: false,
                }),
            )
            .await
            .map_err(|e| format!("remove old: {e}"))?;
    }
    let (id, name) = create_with_pull(
        docker,
        req.spec.name.clone(),
        req.spec.to_config(),
        PullPolicy::Missing,
    )
    .await?;
    docker
        .start_container(
            &id,
            None::<bollard::container::StartContainerOptions<String>>,
        )
        .await
        .map_err(|e| format!("start: {e}"))?;
    if !same_name {
        docker
            .remove_container(
                &req.old_id,
                Some(RemoveContainerOptions {
                    v: false,
                    force: true,
                    link: false,
                }),
            )
            .await
            .map_err(|e| format!("remove old: {e}"))?;
    }
    Ok(Some((
        id.clone(),
        name.unwrap_or_else(|| id.chars().take(12).collect()),
    )))
}

/// Run a user op from ~/.dockui/ops.yml: replace if requested, create (pulling
/// the image on 404), then start.
pub fn spawn_run_op(docker: Docker, tx: Sender<Msg>, op_name: String, op: Op) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (name, config) = op.to_create();
        let result = run_op(&docker, &op, name, config).await;
        let (ok, message, open_logs) = match result {
            Ok(target) => (true, String::new(), target),
            Err(e) => (false, e, None),
        };
        let _ = tx
            .send(Msg::ActionDone {
                label: format!("op {op_name}"),
                ok,
                message,
                open_logs,
            })
            .await;
    })
}

async fn run_op(
    docker: &Docker,
    op: &Op,
    name: Option<String>,
    config: bollard::container::Config<String>,
) -> Result<Option<LogTarget>, String> {
    if op.replace
        && let Some(name) = &name
    {
        let _ = docker
            .remove_container(
                name.as_str(),
                Some(RemoveContainerOptions {
                    v: false,
                    force: true,
                    link: false,
                }),
            )
            .await;
    }
    if op.pull == PullPolicy::Always {
        let image = op.image().to_string();
        pull_image(docker, &image).await?;
    }
    let (id, name) = create_with_pull(docker, name, config, op.pull).await?;
    docker
        .start_container(
            &id,
            None::<bollard::container::StartContainerOptions<String>>,
        )
        .await
        .map_err(|e| format!("start: {e}"))?;
    Ok(op.follow_logs.then(|| LogTarget {
        name: name.unwrap_or_else(|| id.chars().take(12).collect()),
        id,
    }))
}
