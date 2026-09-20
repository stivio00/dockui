use std::collections::HashMap;

pub const LABEL_PROJECT: &str = "com.docker.compose.project";
pub const LABEL_SERVICE: &str = "com.docker.compose.service";
pub const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
pub const LABEL_CONFIG_FILES: &str = "com.docker.compose.project.config_files";

#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub command: String,
    pub labels: HashMap<String, String>,
    pub ports: Vec<String>,
}

impl Container {
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
    pub fn short_id(&self) -> &str {
        let mut end = self.id.len().min(12);
        while end > 0 && !self.id.is_char_boundary(end) {
            end -= 1;
        }
        &self.id[..end]
    }
    pub fn compose_project(&self) -> Option<&str> {
        self.labels.get(LABEL_PROJECT).map(|s| s.as_str())
    }
    pub fn compose_service(&self) -> Option<&str> {
        self.labels.get(LABEL_SERVICE).map(|s| s.as_str())
    }
    pub fn compose_workdir(&self) -> Option<&str> {
        self.labels.get(LABEL_WORKING_DIR).map(|s| s.as_str())
    }
    pub fn compose_config_files(&self) -> Option<&str> {
        self.labels.get(LABEL_CONFIG_FILES).map(|s| s.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub mountpoint: String,
    pub labels: HashMap<String, String>,
    pub size: Option<u64>,
    pub ref_count: Option<u64>,
}

impl Volume {
    pub fn compose_project(&self) -> Option<&str> {
        self.labels.get(LABEL_PROJECT).map(|s| s.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DockerEvent {
    pub time: i64,
    pub typ: String,
    pub action: String,
    pub actor_id: String,
    pub actor_name: String,
    pub scope: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsSample {
    pub cpu_pct: f64,
    pub mem: u64,
    pub mem_limit: u64,
    pub mem_pct: f64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub blk_read: u64,
    pub blk_write: u64,
    pub pids: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct ContainerDetails {
    pub env: Vec<String>,
    pub ports: Vec<String>,
    pub mounts: Vec<String>,
    pub networks: Vec<String>,
    pub ip: Option<String>,
    pub cmd: String,
    pub entrypoint: String,
    pub working_dir: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub restart_count: Option<i64>,
    pub health: Option<String>,
    pub log_path: Option<String>,
    pub user: Option<String>,
    pub privileged: bool,
    pub gpus: Vec<String>,
    pub restart_policy: Option<String>,
    pub network_mode: Option<String>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub auto_remove: bool,
    pub shm_size: Option<i64>,
    pub spec: Option<crate::ops::ContainerSpec>,
}

impl ContainerDetails {
    /// Whether the container runs as root (user unset or root/0).
    pub fn runs_as_root(&self) -> bool {
        match self.user.as_deref() {
            None | Some("") | Some("root") | Some("0") | Some("0:0") => true,
            Some(_) => false,
        }
    }

    /// Host-namespace sharing summary, e.g. ["net:host", "pid:host"].
    pub fn host_namespaces(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (mode, tag) in [
            (&self.network_mode, "net"),
            (&self.pid_mode, "pid"),
            (&self.ipc_mode, "ipc"),
        ] {
            if let Some(m) = mode
                && m != "bridge"
                && m != "private"
                && m != "shareable"
                && !m.is_empty()
            {
                out.push(format!("{tag}:{m}"));
            }
        }
        out
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0}{}", UNITS[u])
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

pub fn fmt_created(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => unix.to_string(),
    }
}

pub fn fmt_hms(unix: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(unix, 0) {
        chrono::LocalResult::Single(t) => t.format("%H:%M:%S").to_string(),
        _ => unix.to_string(),
    }
}

pub fn fmt_percent(p: f64) -> String {
    format!("{p:.2}%")
}
