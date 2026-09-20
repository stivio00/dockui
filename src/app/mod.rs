use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bollard::Docker;
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use tokio::sync::Notify;
use tokio::sync::mpsc::Sender;

use crate::actions::{EditForm, EnvEditor, PendingAction, TextInput};
use crate::docker::{DockerContext, Tunnel};
use crate::exec::TerminalRequest;
use crate::files::FsEntry;
use crate::model::*;
use crate::ops::{self, Op};
use crate::workers::{self, LogTarget, Msg, Workers};

mod keys;
mod mouse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Tree,
    Stats,
    Logs,
    Events,
    Files,
}

impl View {
    pub fn title(&self) -> &'static str {
        match self {
            View::Tree => "TREE",
            View::Stats => "STATS",
            View::Logs => "LOGS",
            View::Events => "EVENTS",
            View::Files => "FILES",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    None,
    Context,
    ContainerSelect,
    Help,
    Ops,
    Confirm,
    Edit,
    Exec,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting,
    Connected,
    Error(String),
    Mock,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Section(&'static str),
    Project(String),
    Service(String, String),
    Container(String),
    Volume(String),
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub key: String,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub label: String,
    pub sub: String,
    pub state: Option<String>,
}

pub struct Toast {
    pub ok: bool,
    pub text: String,
    pub at: Instant,
}

const TOAST_TTL: Duration = Duration::from_secs(4);

/// Regex filter state for the vim-style `/` search.
#[derive(Default)]
pub struct Search {
    pub active: Option<regex::Regex>,
    pub input: TextInput,
    pub prompting: bool,
    pub last: String,
    pub error: Option<String>,
}

/// Filesystem explorer state: which container it is browsing, where it
/// is, and what came back for the current directory request.
pub struct FilesState {
    pub id: String,
    pub name: String,
    pub path: String,
    pub entries: Vec<FsEntry>,
    pub sel: usize,
    pub state: ListState,
    pub loading: bool,
    pub error: Option<String>,
    pub req: u64,
}

/// Rects of the regions rendered in the last frame, used for mouse targeting.
#[derive(Clone, Copy, Debug, Default)]
pub struct Areas {
    pub tree: Rect,
    pub detail: Rect,
    pub stats_table: Rect,
    pub stats_charts: Rect,
    pub logs: Rect,
    pub events: Rect,
    pub files: Rect,
    pub popup: Rect,
    pub popup_list: Rect,
    /// x where the env editor's VALUE column starts (set during draw)
    pub env_split_x: u16,
}

fn contains(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

pub struct App {
    pub tx: Sender<Msg>,
    pub mock: bool,
    pub view: View,
    pub focus: Focus,
    pub popup: Popup,
    pub popup_sel: usize,
    pub popup_state: ListState,

    pub contexts: Vec<DockerContext>,
    pub current_ctx: usize,
    pub docker: Option<Docker>,
    pub tunnel_slot: Arc<Mutex<Option<Tunnel>>>,
    pub conn: ConnState,
    pub conn_err: String,

    pub containers: Vec<Container>,
    pub volumes: Vec<Volume>,
    pub events: VecDeque<DockerEvent>,
    pub events_follow: bool,
    pub events_off: usize,

    pub stats: HashMap<String, StatsSample>,
    pub cpu_hist: HashMap<String, VecDeque<u64>>,
    pub mem_hist: HashMap<String, VecDeque<u64>>,

    pub details: HashMap<String, ContainerDetails>,
    inspect_sent: HashMap<String, Instant>,
    pub detail_scroll: u16,

    pub tree_rows: Vec<Row>,
    pub tree_sel: usize,
    pub tree_state: ListState,
    pub expanded: HashSet<String>,
    seen_nodes: HashSet<String>,

    pub log_targets: Vec<LogTarget>,
    pub log_names: HashSet<String>,
    pub log_lines: VecDeque<String>,
    pub log_follow: bool,
    pub log_off: usize,

    pub files: Option<FilesState>,
    pub files_req: u64,

    pub stats_sel: usize,
    pub stats_state: TableState,
    stats_spawned: Vec<String>,

    pub workers: Workers,
    pub stats_workers: Workers,
    pub log_workers: Workers,
    pub refresh_notify: Arc<Notify>,

    pub ops: Vec<(String, Op)>,
    pub ops_error: Option<String>,
    pub pending: Option<PendingAction>,
    pub edit: Option<EditForm>,
    edit_pending: Option<String>,
    pub env_editor: Option<EnvEditor>,
    pub exec_tx: Option<tokio::sync::mpsc::UnboundedSender<TerminalRequest>>,
    pub exec_user_input: Option<TextInput>,
    pub toast: Option<Toast>,
    pub search: Search,
    pub areas: Areas,
    /// tree/detail width split in percent (Tree view)
    pub split_pct: u16,
    dragging_split: bool,
}

const HIST_LEN: usize = 120;
const LOG_CAP: usize = 10_000;
const EVENT_CAP: usize = 500;

impl App {
    pub fn new(
        tx: Sender<Msg>,
        mock: bool,
        exec_tx: Option<tokio::sync::mpsc::UnboundedSender<TerminalRequest>>,
    ) -> anyhow::Result<Self> {
        let contexts = if mock {
            vec![
                DockerContext::new("mock-local", "mock://local", true),
                DockerContext::new("desktop-linux", "unix:///var/run/docker.sock", false),
                DockerContext::new("t480 (ssh)", "ssh://stephen@stephen-t480.local", false),
            ]
        } else {
            crate::docker::list_contexts()?
        };
        let current_ctx = contexts.iter().position(|c| c.current).unwrap_or(0);

        let mut expanded: HashSet<String> = HashSet::new();
        expanded.insert("sec:projects".into());
        expanded.insert("sec:containers".into());
        expanded.insert("sec:volumes".into());

        let (ops, ops_error) = ops::load();
        let mut app = App {
            tx,
            mock,
            view: View::Tree,
            focus: Focus::Tree,
            popup: Popup::None,
            popup_sel: 0,
            popup_state: ListState::default(),
            contexts,
            current_ctx,
            docker: None,
            tunnel_slot: Arc::new(Mutex::new(None)),
            conn: ConnState::Connecting,
            conn_err: String::new(),
            containers: Vec::new(),
            volumes: Vec::new(),
            events: VecDeque::new(),
            events_follow: true,
            events_off: 0,
            stats: HashMap::new(),
            cpu_hist: HashMap::new(),
            mem_hist: HashMap::new(),
            details: HashMap::new(),
            inspect_sent: HashMap::new(),
            detail_scroll: 0,
            tree_rows: Vec::new(),
            tree_sel: 0,
            tree_state: ListState::default(),
            expanded,
            seen_nodes: HashSet::new(),
            log_targets: Vec::new(),
            log_names: HashSet::new(),
            log_lines: VecDeque::new(),
            log_follow: true,
            log_off: 0,
            files: None,
            files_req: 0,
            stats_sel: 0,
            stats_state: TableState::default(),
            stats_spawned: Vec::new(),
            workers: Workers::default(),
            stats_workers: Workers::default(),
            log_workers: Workers::default(),
            refresh_notify: Arc::new(Notify::new()),
            ops,
            ops_error,
            pending: None,
            edit: None,
            edit_pending: None,
            env_editor: None,
            exec_tx,
            exec_user_input: None,
            toast: None,
            search: Search::default(),
            areas: Areas::default(),
            split_pct: 58,
            dragging_split: false,
        };
        app.rebuild_tree();
        Ok(app)
    }

    pub fn current_context(&self) -> &DockerContext {
        &self.contexts[self.current_ctx]
    }

    pub fn clear_data(&mut self) {
        self.containers.clear();
        self.volumes.clear();
        self.events.clear();
        self.stats.clear();
        self.cpu_hist.clear();
        self.mem_hist.clear();
        self.details.clear();
        self.inspect_sent.clear();
        self.log_lines.clear();
        self.log_targets.clear();
        self.log_names.clear();
        self.stats_spawned.clear();
        self.seen_nodes.clear();
        self.rebuild_tree();
    }

    pub fn bootstrap(&mut self) {
        self.workers.abort_all();
        self.stats_workers.abort_all();
        self.log_workers.abort_all();
        self.docker = None;
        self.stats_spawned.clear();
        self.pending = None;
        self.edit = None;
        self.edit_pending = None;
        self.env_editor = None;
        self.exec_user_input = None;
        if self.files.is_some() {
            self.files = None;
            if self.view == View::Files {
                self.view = View::Tree;
            }
        }
        if let Ok(mut slot) = self.tunnel_slot.lock() {
            *slot = None;
        }
        self.clear_data();
        self.conn = ConnState::Connecting;
        self.conn_err.clear();

        let ctx = self.current_context().clone();
        let tx = self.tx.clone();
        let base = self.workers.shared();
        let notify = self.refresh_notify.clone();
        let mock = self.mock;
        let tunnel_slot = self.tunnel_slot.clone();
        let j = tokio::spawn(async move {
            if mock {
                let _ = tx.send(Msg::ConnMock).await;
                base.add(crate::mock::spawn_mock(tx.clone()));
                return;
            }
            match crate::docker::connect(&ctx).await {
                Ok(conn) => {
                    if let Some(t) = conn.tunnel
                        && let Ok(mut slot) = tunnel_slot.lock()
                    {
                        *slot = Some(t);
                    }
                    let _ = tx
                        .send(Msg::ConnOk {
                            docker: conn.docker.clone(),
                        })
                        .await;
                    base.add(workers::spawn_container_poll(
                        conn.docker.clone(),
                        tx.clone(),
                        notify,
                    ));
                    base.add(workers::spawn_volume_poll(conn.docker.clone(), tx.clone()));
                    base.add(workers::spawn_events(conn.docker, tx));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnErr(e.to_string())).await;
                }
            }
        });
        self.workers.add(j);
    }

    pub fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::ConnMock => {
                self.conn = ConnState::Mock;
                self.conn_err.clear();
            }
            Msg::ConnOk { docker } => {
                self.conn = ConnState::Connected;
                self.conn_err.clear();
                self.docker = Some(docker);
                self.sync_stats_tasks();
                self.request_inspect_for_selection();
            }
            Msg::ConnErr(e) => {
                if self.conn != ConnState::Connecting {
                    self.conn = ConnState::Error(e.clone());
                }
                self.conn_err = e;
            }
            Msg::Containers(list) => {
                if self.conn != ConnState::Mock {
                    self.conn = ConnState::Connected;
                }
                self.prune_stale(&list);
                self.containers = list;
                self.rebuild_tree();
                self.sync_stats_tasks();
                self.request_inspect_for_selection();
            }
            Msg::Volumes(list) => {
                self.volumes = list;
                self.rebuild_tree();
            }
            Msg::Event(ev) => {
                self.events.push_back(ev);
                while self.events.len() > EVENT_CAP {
                    self.events.pop_front();
                }
            }
            Msg::Stats { id, sample } => {
                let cpu = sample.cpu_pct.clamp(0.0, 400.0) as u64;
                let mem = sample.mem;
                self.cpu_hist.entry(id.clone()).or_default().push_back(cpu);
                self.mem_hist.entry(id.clone()).or_default().push_back(mem);
                let h = self.cpu_hist.get_mut(&id).unwrap();
                while h.len() > HIST_LEN {
                    h.pop_front();
                }
                let h = self.mem_hist.get_mut(&id).unwrap();
                while h.len() > HIST_LEN {
                    h.pop_front();
                }
                self.stats.insert(id, sample);
            }
            Msg::LogLine { name, line } => {
                if !self.log_names.contains(&name) {
                    return;
                }
                let line = if self.log_names.len() > 1 {
                    format!("[{name}] {line}")
                } else {
                    line
                };
                self.log_lines.push_back(line);
                while self.log_lines.len() > LOG_CAP {
                    self.log_lines.pop_front();
                }
            }
            Msg::Inspected { id, details } => {
                let open_edit = self.edit_pending.as_deref() == Some(id.as_str());
                self.details.insert(id.clone(), details);
                if open_edit {
                    self.edit_pending = None;
                    self.open_edit_form(&id);
                }
            }
            Msg::FilesListed { req, result } => {
                if let Some(f) = &mut self.files
                    && f.req == req
                {
                    f.loading = false;
                    match result {
                        Ok(entries) => {
                            f.entries = entries;
                            f.sel = f.sel.min(f.entries.len().saturating_sub(1));
                            f.error = None;
                        }
                        Err(e) => {
                            f.entries.clear();
                            f.error = Some(e);
                        }
                    }
                }
            }
            Msg::ActionDone {
                label,
                ok,
                message,
                open_logs,
            } => {
                let text = if ok {
                    format!("{label} done")
                } else {
                    format!("{label} failed: {message}")
                };
                self.toast(ok, text);
                if let Some(t) = open_logs {
                    self.open_logs(vec![t]);
                }
            }
        }
    }

    fn prune_stale(&mut self, list: &[Container]) {
        let ids: HashSet<&str> = list.iter().map(|c| c.id.as_str()).collect();
        self.stats.retain(|id, _| ids.contains(id.as_str()));
        self.cpu_hist.retain(|id, _| ids.contains(id.as_str()));
        self.mem_hist.retain(|id, _| ids.contains(id.as_str()));
        self.details.retain(|id, _| ids.contains(id.as_str()));
        self.inspect_sent.retain(|id, _| ids.contains(id.as_str()));
    }

    pub fn toast(&mut self, ok: bool, text: String) {
        self.toast = Some(Toast {
            ok,
            text,
            at: Instant::now(),
        });
    }

    pub fn toast_expired(&self) -> bool {
        self.toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > TOAST_TTL)
    }
}

impl App {
    fn running_ids(&self) -> Vec<String> {
        self.containers
            .iter()
            .filter(|c| c.is_running())
            .map(|c| c.id.clone())
            .collect()
    }

    pub fn running_containers(&self) -> Vec<&Container> {
        let mut v: Vec<&Container> = self.containers.iter().filter(|c| c.is_running()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Running containers visible under the active `/` filter.
    pub fn stats_rows(&self) -> Vec<&Container> {
        let mut v = self.running_containers();
        if let Some(re) = &self.search.active {
            v.retain(|c| re.is_match(&c.name) || re.is_match(&c.image) || re.is_match(&c.state));
        }
        v
    }

    /// Events visible under the active `/` filter.
    pub fn visible_events(&self) -> Vec<&DockerEvent> {
        match &self.search.active {
            None => self.events.iter().collect(),
            Some(re) => self
                .events
                .iter()
                .filter(|ev| {
                    re.is_match(&ev.typ)
                        || re.is_match(&ev.action)
                        || re.is_match(&ev.actor_name)
                        || re.is_match(&ev.actor_id)
                        || re.is_match(&ev.scope)
                })
                .collect(),
        }
    }

    /// Log lines visible under the active `/` filter.
    pub fn visible_log_lines(&self) -> Vec<&String> {
        match &self.search.active {
            None => self.log_lines.iter().collect(),
            Some(re) => self
                .log_lines
                .iter()
                .filter(|l| re.is_match(l.as_str()))
                .collect(),
        }
    }

    /// Filesystem entries visible under the active `/` filter.
    pub fn visible_files(&self) -> Vec<&FsEntry> {
        let Some(f) = &self.files else {
            return Vec::new();
        };
        match &self.search.active {
            None => f.entries.iter().collect(),
            Some(re) => f.entries.iter().filter(|e| re.is_match(&e.name)).collect(),
        }
    }

    pub fn search_active(&self) -> bool {
        self.search.active.is_some()
    }

    fn clear_filter(&mut self) {
        self.search.active = None;
        self.search.error = None;
        self.rebuild_tree();
    }

    fn sync_stats_tasks(&mut self) {
        if self.view != View::Stats || self.mock {
            return;
        }
        let ids = self.running_ids();
        if ids == self.stats_spawned {
            return;
        }
        self.stats_workers.abort_all();
        self.stats_spawned = ids.clone();
        if let Some(docker) = &self.docker {
            for h in workers::spawn_stats_streams(docker.clone(), self.tx.clone(), ids) {
                self.stats_workers.add(h);
            }
        }
    }

    /// Abort background streams that only make sense in the current view
    /// (stats streams in Stats, log streams in Logs) and reset their state.
    fn leave_view_streams(&mut self) {
        match self.view {
            View::Stats => {
                self.stats_workers.abort_all();
                self.stats_spawned.clear();
            }
            View::Logs => {
                self.log_workers.abort_all();
                self.log_targets.clear();
                self.log_names.clear();
            }
            _ => {}
        }
    }

    fn request_inspect_for_selection(&mut self) {
        if self.mock {
            return;
        }
        let Some(row) = self.tree_rows.get(self.tree_sel) else {
            return;
        };
        let RowKind::Container(id) = &row.kind else {
            return;
        };
        let id = id.clone();
        self.request_inspect(&id);
    }

    fn request_inspect(&mut self, id: &str) {
        let fresh = self
            .inspect_sent
            .get(id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if self.details.contains_key(id) || fresh {
            return;
        }
        if let Some(docker) = &self.docker {
            self.inspect_sent.insert(id.to_string(), Instant::now());
            let h = workers::spawn_inspect(docker.clone(), self.tx.clone(), id.to_string());
            self.workers.add(h);
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.tree_rows.get(self.tree_sel)
    }

    pub fn container_by_id(&self, id: &str) -> Option<&Container> {
        self.containers.iter().find(|c| c.id == id)
    }

    pub fn selected_container(&self) -> Option<&Container> {
        let row = self.selected_row()?;
        match &row.kind {
            RowKind::Container(id) => self.container_by_id(id),
            _ => None,
        }
    }

    pub fn containers_for_row(&self, row: &Row) -> Vec<&Container> {
        match &row.kind {
            RowKind::Container(id) => self.container_by_id(id).into_iter().collect(),
            RowKind::Service(p, s) => self
                .containers
                .iter()
                .filter(|c| {
                    c.compose_project() == Some(p.as_str())
                        && c.compose_service() == Some(s.as_str())
                })
                .collect(),
            RowKind::Project(p) => self
                .containers
                .iter()
                .filter(|c| c.compose_project() == Some(p.as_str()))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn rebuild_tree(&mut self) {
        let sel_key = self
            .tree_rows
            .get(self.tree_sel)
            .map(|r| r.key.clone())
            .unwrap_or_default();

        let mut projects: Vec<String> = self
            .containers
            .iter()
            .filter_map(|c| c.compose_project().map(|p| p.to_string()))
            .collect();
        projects.sort();
        projects.dedup();

        // auto-expand projects and services the first time they appear
        for p in &projects {
            let key = format!("p:{p}");
            if self.seen_nodes.insert(key.clone()) {
                self.expanded.insert(key);
            }
            let mut services: Vec<String> = self
                .containers
                .iter()
                .filter(|c| c.compose_project() == Some(p.as_str()))
                .map(|c| c.compose_service().unwrap_or("—").to_string())
                .collect();
            services.sort();
            services.dedup();
            for s in &services {
                let skey = format!("p:{p}/{s}");
                if self.seen_nodes.insert(skey.clone()) {
                    self.expanded.insert(skey);
                }
            }
        }

        // while a filter is active every parent is expanded so all matches
        // are visible
        let show_all = self.search.active.is_some();
        let expanded = |key: &str| show_all || self.expanded.contains(key);

        let mut rows: Vec<Row> = Vec::new();

        let (proj_run, proj_tot): (usize, usize) = self
            .containers
            .iter()
            .filter(|c| c.compose_project().is_some())
            .map(|c| c.is_running())
            .fold((0, 0), |(r, t), run| (r + run as usize, t + 1));

        rows.push(Row {
            kind: RowKind::Section("PROJECTS (COMPOSE)"),
            key: "sec:projects".into(),
            depth: 0,
            has_children: !projects.is_empty(),
            expanded: expanded("sec:projects"),
            label: "Projects".into(),
            sub: if projects.is_empty() {
                "0".into()
            } else {
                format!("{} · {}/{} up", projects.len(), proj_run, proj_tot)
            },
            state: None,
        });

        if expanded("sec:projects") {
            for p in &projects {
                let cs: Vec<&Container> = self
                    .containers
                    .iter()
                    .filter(|c| c.compose_project() == Some(p.as_str()))
                    .collect();
                let running = cs.iter().filter(|c| c.is_running()).count();
                let key = format!("p:{p}");
                let pexp = expanded(&key);
                rows.push(Row {
                    kind: RowKind::Project(p.clone()),
                    key: key.clone(),
                    depth: 1,
                    has_children: true,
                    expanded: pexp,
                    label: p.clone(),
                    sub: format!("{}/{} up", running, cs.len()),
                    state: if running == cs.len() {
                        Some("running".into())
                    } else if running == 0 {
                        Some("exited".into())
                    } else {
                        Some("partial".into())
                    },
                });
                if pexp {
                    let mut services: Vec<String> = cs
                        .iter()
                        .map(|c| c.compose_service().unwrap_or("—").to_string())
                        .collect();
                    services.sort();
                    services.dedup();
                    for s in &services {
                        let scs: Vec<&Container> = cs
                            .iter()
                            .copied()
                            .filter(|c| {
                                c.compose_service()
                                    .map(|x| x == s.as_str())
                                    .unwrap_or(s == "—")
                            })
                            .collect();
                        let srunning = scs.iter().filter(|c| c.is_running()).count();
                        let skey = format!("{key}/{s}");
                        let sexp = expanded(&skey);
                        rows.push(Row {
                            kind: RowKind::Service(p.clone(), s.clone()),
                            key: skey.clone(),
                            depth: 2,
                            has_children: true,
                            expanded: sexp,
                            label: s.clone(),
                            sub: format!("{}/{}", srunning, scs.len()),
                            state: if srunning == scs.len() {
                                Some("running".into())
                            } else if srunning == 0 {
                                Some("exited".into())
                            } else {
                                Some("partial".into())
                            },
                        });
                        if sexp {
                            for c in crate::util::sort_containers(scs) {
                                rows.push(Row {
                                    kind: RowKind::Container(c.id.clone()),
                                    key: format!("c:{}", c.id),
                                    depth: 3,
                                    has_children: false,
                                    expanded: false,
                                    label: c.name.clone(),
                                    sub: c.image.clone(),
                                    state: Some(c.state.clone()),
                                });
                            }
                        }
                    }
                }
            }
        }

        let standalone: Vec<&Container> = self
            .containers
            .iter()
            .filter(|c| c.compose_project().is_none())
            .collect();
        let sa_run = standalone.iter().filter(|c| c.is_running()).count();
        rows.push(Row {
            kind: RowKind::Section("CONTAINERS (STANDALONE)"),
            key: "sec:containers".into(),
            depth: 0,
            has_children: !standalone.is_empty(),
            expanded: expanded("sec:containers"),
            label: "Containers".into(),
            sub: if standalone.is_empty() {
                "0".into()
            } else {
                format!("{} · {}/{} up", standalone.len(), sa_run, standalone.len())
            },
            state: None,
        });
        if expanded("sec:containers") {
            for c in crate::util::sort_containers(standalone) {
                rows.push(Row {
                    kind: RowKind::Container(c.id.clone()),
                    key: format!("c:{}", c.id),
                    depth: 1,
                    has_children: false,
                    expanded: false,
                    label: c.name.clone(),
                    sub: c.image.clone(),
                    state: Some(c.state.clone()),
                });
            }
        }

        rows.push(Row {
            kind: RowKind::Section("VOLUMES"),
            key: "sec:volumes".into(),
            depth: 0,
            has_children: !self.volumes.is_empty(),
            expanded: expanded("sec:volumes"),
            label: "Volumes".into(),
            sub: self.volumes.len().to_string(),
            state: None,
        });
        if expanded("sec:volumes") {
            for v in &self.volumes {
                rows.push(Row {
                    kind: RowKind::Volume(v.name.clone()),
                    key: format!("v:{}", v.name),
                    depth: 1,
                    has_children: false,
                    expanded: false,
                    label: v.name.clone(),
                    sub: match (v.size, v.compose_project()) {
                        (Some(s), Some(p)) => format!("{} · {p}", human_bytes(s)),
                        (Some(s), None) => human_bytes(s),
                        (None, Some(p)) => p.to_string(),
                        (None, None) => String::new(),
                    },
                    state: None,
                });
            }
        }

        if let Some(re) = &self.search.active {
            rows = filter_rows(rows, re);
        }

        self.tree_rows = rows;
        self.tree_sel = self
            .tree_rows
            .iter()
            .position(|r| r.key == sel_key)
            .unwrap_or(self.tree_sel.min(self.tree_rows.len().saturating_sub(1)));
        if self.tree_rows.is_empty() {
            self.tree_sel = 0;
        } else if self.search.active.is_some()
            && !matches!(self.tree_rows[self.tree_sel].kind, RowKind::Container(_))
            && let Some(i) = self
                .tree_rows
                .iter()
                .position(|r| matches!(r.kind, RowKind::Container(_)))
        {
            // with a filter active, land the selection on the first
            // matching container so action keys work immediately
            self.tree_sel = i;
        }
    }

    pub fn open_logs(&mut self, targets: Vec<LogTarget>) {
        self.leave_view_streams();
        self.log_workers.abort_all();
        self.log_lines.clear();
        self.log_off = 0;
        self.log_follow = true;
        self.log_names = targets.iter().map(|t| t.name.clone()).collect();
        self.log_targets = targets.clone();
        if !self.mock
            && let Some(docker) = &self.docker
        {
            for h in workers::spawn_logs(docker.clone(), self.tx.clone(), targets, 200) {
                self.log_workers.add(h);
            }
        }
        self.view = View::Logs;
    }

    pub fn open_logs_for_row(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let cs = self.containers_for_row(&row);
        if cs.is_empty() {
            return;
        }
        let targets: Vec<LogTarget> = cs
            .iter()
            .map(|c| LogTarget {
                id: c.id.clone(),
                name: c.name.clone(),
            })
            .collect();
        self.open_logs(targets);
    }

    pub fn enter_view(&mut self, view: View) {
        match view {
            View::Logs => {
                if self.log_targets.is_empty() {
                    if self.selected_container().is_some() {
                        self.open_logs_for_row();
                    } else if let Some(c) = self.running_containers().first() {
                        let t = LogTarget {
                            id: c.id.clone(),
                            name: c.name.clone(),
                        };
                        self.open_logs(vec![t]);
                    } else {
                        self.view = View::Logs;
                    }
                } else {
                    self.view = View::Logs;
                }
            }
            View::Stats => {
                self.view = View::Stats;
                self.stats_spawned.clear();
                self.sync_stats_tasks();
            }
            _ => self.view = view,
        }
    }

    // ---- filesystem explorer ----

    /// Open the explorer on the selected tree row if it is a container.
    pub fn open_files_for_row(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        match &row.kind {
            RowKind::Container(id) => {
                let name = self
                    .container_by_id(id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| id.chars().take(12).collect());
                self.open_files(id.clone(), name);
            }
            _ => self.toast(false, "files: only containers can be browsed".into()),
        }
    }

    fn open_files(&mut self, id: String, name: String) {
        self.leave_view_streams();
        self.view = View::Files;
        self.files = Some(FilesState {
            id,
            name,
            path: "/".into(),
            entries: Vec::new(),
            sel: 0,
            state: ListState::default(),
            loading: false,
            error: None,
            req: 0,
        });
        self.request_files();
    }

    fn files_enter(&mut self, name: &str) {
        let Some(f) = &self.files else {
            return;
        };
        let path = if f.path == "/" {
            format!("/{name}")
        } else {
            format!("{}/{}", f.path, name)
        };
        self.files_navigate(&path);
    }

    fn files_navigate(&mut self, path: &str) {
        if let Some(f) = &mut self.files {
            f.path = path.to_string();
            f.sel = 0;
            f.state.select(Some(0));
        }
        self.request_files();
    }

    fn request_files(&mut self) {
        let Some(f) = &mut self.files else {
            return;
        };
        self.files_req += 1;
        f.req = self.files_req;
        f.loading = true;
        f.error = None;
        let req = f.req;
        let path = f.path.clone();
        let id = f.id.clone();
        if self.mock {
            f.entries = crate::files::mock_listing(&path);
            f.loading = false;
            f.sel = 0;
            return;
        }
        let Some(docker) = self.docker.clone() else {
            f.loading = false;
            f.error = Some("not connected".into());
            return;
        };
        let h = workers::spawn_files_list(docker, self.tx.clone(), req, id, path);
        self.workers.add(h);
    }
}

/// Keep rows that match the regex, plus their ancestors; if a parent row
/// itself matches, its whole subtree is kept.
fn filter_rows(rows: Vec<Row>, re: &regex::Regex) -> Vec<Row> {
    let mut keep = vec![false; rows.len()];
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for i in 0..rows.len() {
        while stack
            .last()
            .is_some_and(|&(top, _)| rows[top].depth >= rows[i].depth)
        {
            stack.pop();
        }
        if stack.iter().any(|&(_, matched)| matched) {
            keep[i] = true;
        }
        let r = &rows[i];
        let text = format!(
            "{} {} {}",
            r.label,
            r.sub,
            r.state.clone().unwrap_or_default()
        );
        let matched = re.is_match(&text);
        if matched {
            keep[i] = true;
            for &(a, _) in &stack {
                keep[a] = true;
            }
        }
        stack.push((i, matched));
    }
    rows.into_iter()
        .zip(keep)
        .filter_map(|(r, k)| k.then_some(r))
        .collect()
}
