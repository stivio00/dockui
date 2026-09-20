use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::Sender;

use crate::model::*;
use crate::workers::Msg;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn container(
    id: &str,
    name: &str,
    image: &str,
    state: &str,
    status: &str,
    project: Option<&str>,
    service: Option<&str>,
) -> Container {
    let mut labels = HashMap::new();
    if let Some(p) = project {
        labels.insert(LABEL_PROJECT.to_string(), p.to_string());
        labels.insert(
            LABEL_WORKING_DIR.to_string(),
            format!("/home/stephen/projects/{p}"),
        );
        labels.insert(
            LABEL_CONFIG_FILES.to_string(),
            format!("/home/stephen/projects/{p}/docker-compose.yml"),
        );
    }
    if let Some(s) = service {
        labels.insert(LABEL_SERVICE.to_string(), s.to_string());
    }
    Container {
        id: id.to_string(),
        name: name.to_string(),
        image: image.to_string(),
        state: state.to_string(),
        status: status.to_string(),
        created: 1761300000,
        command: format!("{image} serve"),
        labels,
        ports: if state == "running" {
            vec![format!("0.0.0.0:808{}->80/tcp", id.len() % 9)]
        } else {
            vec![]
        },
    }
}

pub fn sample_containers() -> Vec<Container> {
    vec![
        container(
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0",
            "webshop-web-1",
            "nginx:1.27",
            "running",
            "Up 2 hours (healthy)",
            Some("webshop"),
            Some("web"),
        ),
        container(
            "b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c",
            "webshop-api-1",
            "ghcr.io/webshop/api:v2.4.1",
            "running",
            "Up 2 hours",
            Some("webshop"),
            Some("api"),
        ),
        container(
            "c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1",
            "webshop-db-1",
            "postgres:16.2",
            "running",
            "Up 2 hours (healthy)",
            Some("webshop"),
            Some("db"),
        ),
        container(
            "d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d",
            "webshop-redis-1",
            "redis:7.2-alpine",
            "exited",
            "Exited (0) 26 minutes ago",
            Some("webshop"),
            Some("redis"),
        ),
        container(
            "e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2",
            "obs-prometheus-1",
            "prom/prometheus:v2.51.0",
            "running",
            "Up 5 days",
            Some("observability"),
            Some("prometheus"),
        ),
        container(
            "f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e",
            "obs-grafana-1",
            "grafana/grafana:10.4.2",
            "running",
            "Up 5 days (healthy)",
            Some("observability"),
            Some("grafana"),
        ),
        container(
            "0a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8",
            "registry",
            "registry:2.8.3",
            "running",
            "Up 5 days",
            None,
            None,
        ),
        container(
            "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9",
            "sandbox",
            "alpine:3.20",
            "exited",
            "Exited (137) 3 hours ago",
            None,
            None,
        ),
    ]
}

pub fn sample_volumes() -> Vec<Volume> {
    let mk = |name: &str, size: u64, rc: u64, project: Option<&str>| {
        let mut labels = HashMap::new();
        if let Some(p) = project {
            labels.insert(LABEL_PROJECT.to_string(), p.to_string());
        }
        Volume {
            name: name.to_string(),
            driver: "local".into(),
            scope: "local".into(),
            mountpoint: format!("/var/lib/docker/volumes/{name}/_data"),
            labels,
            size: Some(size),
            ref_count: Some(rc),
        }
    };
    vec![
        mk("webshop_pgdata", 483_183_820, 1, Some("webshop")),
        mk("webshop_redis", 4_194_304, 1, Some("webshop")),
        mk(
            "obs_prometheus_data",
            2_147_483_648,
            1,
            Some("observability"),
        ),
        mk("obs_grafana_data", 314_572_800, 1, Some("observability")),
        mk("registry_data", 7_516_192_768, 1, None),
    ]
}

pub fn sample_details(id: &str) -> ContainerDetails {
    let env = match id.chars().next() {
        Some('a') => vec![
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
            "NGINX_VERSION=1.27.0".into(),
            "NJS_VERSION=0.8.4".into(),
            "PKG_RELEASE=2~bookworm".into(),
        ],
        Some('c') => vec![
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
            "PGDATA=/var/lib/postgresql/data".into(),
            "POSTGRES_DB=webshop".into(),
            "POSTGRES_USER=webshop".into(),
            "POSTGRES_PASSWORD=hunter2".into(),
        ],
        _ => vec![
            "PATH=/usr/local/sbin:/usr/local/bin".into(),
            "APP_ENV=production".into(),
            "LOG_LEVEL=info".into(),
        ],
    };
    // the api container demonstrates privileged + GPU + host namespaces
    let demo_gpu = id.starts_with('b');
    let mut host = bollard::models::HostConfig::default();
    if demo_gpu {
        host.privileged = Some(true);
        host.device_requests = Some(vec![bollard::models::DeviceRequest {
            driver: Some(String::new()),
            count: Some(-1),
            device_ids: None,
            capabilities: Some(vec![vec!["gpu".to_string()]]),
            options: None,
        }]);
        host.network_mode = Some("host".into());
        host.pid_mode = Some("host".into());
        host.ipc_mode = Some("host".into());
    }
    let name = match id.chars().next() {
        Some('a') => "webshop-web-1",
        Some('b') => "webshop-api-1",
        Some('c') => "webshop-db-1",
        Some('d') => "webshop-redis-1",
        Some('e') => "obs-prometheus-1",
        Some('f') => "obs-grafana-1",
        Some('0') => "registry",
        _ => "sandbox",
    };
    let spec = crate::ops::ContainerSpec {
        name: Some(name.to_string()),
        image: match id.chars().next() {
            Some('a') => Some("nginx:1.27".into()),
            Some('b') => Some("ghcr.io/webshop/api:v2.4.1".into()),
            Some('c') => Some("postgres:16.2".into()),
            _ => Some("alpine:3.20".into()),
        },
        cmd: Some(vec!["serve".into()]),
        entrypoint: Some(vec!["/docker-entrypoint.sh".into()]),
        env: env.clone(),
        labels: HashMap::new(),
        user: None,
        working_dir: Some("/app".into()),
        tty: false,
        host: host.clone(),
    };
    ContainerDetails {
        env,
        ports: vec!["0.0.0.0:8080 -> 80/tcp".into()],
        mounts: vec![
            "webshop_site -> /usr/share/nginx/html (volume, rw)".into(),
            "/home/stephen/projects/webshop/nginx.conf -> /etc/nginx/conf.d/default.conf (bind, ro)".into(),
        ],
        networks: vec![format!("{}_default", id.chars().take(4).collect::<String>()),
            "bridge".into()],
        ip: (id.starts_with('a')).then(|| "172.18.0.2".into()),
        cmd: "serve".into(),
        entrypoint: "/docker-entrypoint.sh".into(),
        working_dir: Some("/app".into()),
        started_at: Some("2026-09-20T06:31:12.441Z".into()),
        finished_at: None,
        exit_code: Some(0),
        restart_count: Some(0),
        health: Some("healthy".into()),
        log_path: Some(format!(
            "/var/lib/docker/containers/{}/{}-json.log",
            id,
            id.chars().take(6).collect::<String>()
        )),
        user: None,
        privileged: host.privileged.unwrap_or(false),
        gpus: crate::actions::gpu_summary(&host),
        restart_policy: None,
        network_mode: host.network_mode.clone(),
        pid_mode: host.pid_mode.clone(),
        ipc_mode: host.ipc_mode.clone(),
        auto_remove: false,
        shm_size: None,
        spec: Some(spec),
    }
}

const LOG_LINES: [&str; 12] = [
    "GET /api/products 200 12ms",
    "GET /api/cart 200 3ms",
    "POST /api/checkout 201 145ms",
    "GET /healthz 200 1ms",
    "cache miss for key=products:page:3",
    "connection accepted from 172.18.0.5:52314",
    "WARN: slow query took 212ms (SELECT * FROM orders)",
    "database connection pool at 80% capacity",
    "reload requested via SIGHUP, reloading configuration",
    "GET /metrics 200 2ms",
    "INFO: worker process started with pid 97",
    "level=info msg=\"serving prometheus metrics\" addr=:9090",
];

pub fn spawn_mock(tx: Sender<Msg>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let containers = sample_containers();
        let _ = tx.send(Msg::Containers(containers.clone())).await;
        let _ = tx.send(Msg::Volumes(sample_volumes())).await;
        for c in &containers {
            let _ = tx
                .send(Msg::Inspected {
                    id: c.id.clone(),
                    details: sample_details(&c.id),
                })
                .await;
        }

        let mut rng = Lcg(42);
        let mut tick: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tick += 1;
            for c in &containers {
                if !c.is_running() {
                    continue;
                }
                let phase = tick as f64 / 8.0 + (c.id.len() % 7) as f64;
                let base = match c.name.as_str() {
                    n if n.contains("db") => 22.0,
                    n if n.contains("api") => 38.0,
                    n if n.contains("grafana") => 6.0,
                    n if n.contains("prometheus") => 11.0,
                    _ => 3.0,
                };
                let cpu = (base + (phase.sin() * 14.0) + (rng.next() % 100) as f64 / 10.0).max(0.2);
                let mem = match c.name.as_str() {
                    n if n.contains("db") => 412_000_000,
                    n if n.contains("api") => 168_000_000,
                    n if n.contains("grafana") => 234_000_000,
                    n if n.contains("prometheus") => 389_000_000,
                    _ => 12_000_000,
                } + (rng.next() % 8_000_000);
                let _ = tx
                    .send(Msg::Stats {
                        id: c.id.clone(),
                        sample: StatsSample {
                            cpu_pct: cpu,
                            mem,
                            mem_limit: 4_294_967_296,
                            mem_pct: mem as f64 / 4_294_967_296.0 * 100.0,
                            net_rx: 1_048_576 * (tick % 97) + rng.next() % 900_000,
                            net_tx: 524_288 * (tick % 61) + rng.next() % 300_000,
                            blk_read: 1_048_576 * (tick % 43),
                            blk_write: 524_288 * (tick % 29),
                            pids: Some(8 + tick % 40),
                        },
                    })
                    .await;
            }
            // occasional event
            if tick % 4 == 0 {
                let idx = (tick as usize / 4) % containers.len();
                let c = &containers[idx];
                let _ = tx
                    .send(Msg::Event(DockerEvent {
                        time: chrono::Utc::now().timestamp(),
                        typ: "container".into(),
                        action: if tick % 8 == 0 {
                            "die"
                        } else {
                            "health_status: healthy"
                        }
                        .into(),
                        actor_id: c.id.clone(),
                        actor_name: c.name.clone(),
                        scope: "local".into(),
                    }))
                    .await;
            }
            // log line per running container every 2 ticks
            if tick % 2 == 0 {
                for c in &containers {
                    if !c.is_running() {
                        continue;
                    }
                    let line = LOG_LINES[(rng.next() as usize) % LOG_LINES.len()];
                    let _ = tx
                        .send(Msg::LogLine {
                            name: c.name.clone(),
                            line: format!(
                                "2026-09-20T08:{:02}:{:02}.{:03}Z {line}",
                                tick % 60,
                                (tick * 7) % 60,
                                rng.next() % 999
                            ),
                        })
                        .await;
                }
            }
        }
    })
}
