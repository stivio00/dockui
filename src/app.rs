use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bollard::Docker;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use tokio::sync::Notify;
use tokio::sync::mpsc::Sender;

use crate::actions::{ContainerAction, EditForm, EnvCol, EnvEditor, PendingAction, TextInput};
use crate::docker::{DockerContext, Tunnel};
use crate::exec::{ExecRequest, TerminalRequest};
use crate::model::*;
use crate::ops::{self, Op};
use crate::workers::{self, LogTarget, Msg, Workers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Tree,
    Stats,
    Logs,
    Events,
}

impl View {
    pub fn title(&self) -> &'static str {
        match self {
            View::Tree => "TREE",
            View::Stats => "STATS",
            View::Logs => "LOGS",
            View::Events => "EVENTS",
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

/// Rects of the regions rendered in the last frame, used for mouse targeting.
#[derive(Clone, Copy, Debug, Default)]
pub struct Areas {
    pub tree: Rect,
    pub detail: Rect,
    pub stats_table: Rect,
    pub stats_charts: Rect,
    pub logs: Rect,
    pub events: Rect,
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

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return true;
        }
        let code = key.code;
        if key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
            && code == KeyCode::Char('c')
        {
            return false;
        }
        if self.search.prompting {
            return self.handle_search_key(key);
        }
        if self.popup != Popup::None {
            return self.handle_popup_key(code);
        }
        match self.view {
            View::Tree => self.handle_tree_key(code),
            View::Stats => self.handle_stats_key(code),
            View::Logs => self.handle_logs_key(code),
            View::Events => self.handle_events_key(code),
        }
    }

    fn refresh(&mut self) {
        self.inspect_sent.clear();
        self.refresh_notify.notify_one();
        if self.view == View::Stats {
            self.stats_spawned.clear();
            self.sync_stats_tasks();
        }
        self.request_inspect_for_selection();
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        let code = key.code;
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        let input = &mut self.search.input;
        match code {
            KeyCode::Esc => {
                self.search.prompting = false;
                self.search.error = None;
            }
            KeyCode::Enter => {
                let pattern = input.value.clone();
                match regex::Regex::new(&pattern) {
                    Ok(re) => {
                        self.search.last = pattern;
                        self.search.active = Some(re);
                        self.search.prompting = false;
                        self.search.error = None;
                        self.rebuild_tree();
                    }
                    Err(e) => {
                        self.search.error = Some(e.to_string());
                    }
                }
            }
            KeyCode::Char('u') if ctrl => {
                input.value.clear();
                input.cursor = 0;
            }
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Left => input.left(),
            KeyCode::Right => input.right(),
            KeyCode::Home => input.home(),
            KeyCode::End => input.end(),
            KeyCode::Char(c) => input.insert(c),
            _ => {}
        }
        true
    }

    fn handle_global_key(&mut self, code: KeyCode) -> Option<bool> {
        match code {
            KeyCode::Char('q') => Some(false),
            KeyCode::Char('c') => {
                self.popup = Popup::Context;
                self.popup_sel = self.current_ctx;
                Some(true)
            }
            KeyCode::Char('?') => {
                self.popup = Popup::Help;
                Some(true)
            }
            KeyCode::Char('/') => {
                self.search.prompting = true;
                self.search.input = TextInput::new(self.search.last.clone());
                self.search.error = None;
                Some(true)
            }
            KeyCode::Char('o') => {
                self.popup = Popup::Ops;
                self.popup_sel = 0;
                Some(true)
            }
            KeyCode::Char('1') => {
                self.leave_view_streams();
                self.view = View::Tree;
                Some(true)
            }
            KeyCode::Char('2') => {
                self.leave_view_streams();
                self.enter_view(View::Stats);
                Some(true)
            }
            KeyCode::Char('3') => {
                self.leave_view_streams();
                self.view = View::Events;
                Some(true)
            }
            KeyCode::Char('4') => {
                if self.view != View::Logs {
                    self.leave_view_streams();
                }
                self.enter_view(View::Logs);
                Some(true)
            }
            KeyCode::Char('r') => {
                self.refresh();
                Some(true)
            }
            _ => None,
        }
    }

    fn toggle_expand(&mut self) {
        if let Some(row) = self.selected_row().cloned()
            && row.has_children
        {
            if self.expanded.contains(&row.key) {
                self.expanded.remove(&row.key);
            } else {
                self.expanded.insert(row.key.clone());
            }
            self.rebuild_tree();
        }
    }

    fn collapse_or_up(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.has_children && row.expanded {
            self.expanded.remove(&row.key);
            self.rebuild_tree();
            return;
        }
        // jump to parent row
        for i in (0..self.tree_sel).rev() {
            if self.tree_rows[i].depth < row.depth {
                self.tree_sel = i;
                self.detail_scroll = 0;
                self.request_inspect_for_selection();
                return;
            }
        }
    }

    fn tree_move(&mut self, delta: i32) {
        if self.tree_rows.is_empty() {
            return;
        }
        let new = (self.tree_sel as i32 + delta).clamp(0, self.tree_rows.len() as i32 - 1);
        if new as usize != self.tree_sel {
            self.tree_sel = new as usize;
            self.detail_scroll = 0;
            self.request_inspect_for_selection();
        }
    }

    fn nudge_split(&mut self, delta: i16) {
        self.split_pct = (self.split_pct as i16 + delta).clamp(20, 80) as u16;
    }

    fn set_split_from_col(&mut self, col: u16) {
        let t = self.areas.tree;
        let d = self.areas.detail;
        let full = (t.width + d.width).max(1) as u32;
        let rel = col.saturating_sub(t.x) as u32;
        self.split_pct = ((rel * 100) / full).clamp(20, 80) as u16;
    }

    fn handle_tree_key(&mut self, code: KeyCode) -> bool {
        if let Some(res) = self.handle_global_key(code) {
            return res;
        }
        if self.focus == Focus::Detail {
            match code {
                KeyCode::Tab => self.focus = Focus::Tree,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = crate::ui::detail::detail_line_count(self).saturating_sub(1);
                    if (self.detail_scroll as usize) < max {
                        self.detail_scroll += 1;
                    }
                }
                KeyCode::Char('g') => self.detail_scroll = 0,
                KeyCode::Char('G') => {
                    self.detail_scroll = crate::ui::detail::detail_line_count(self)
                        .saturating_sub(1)
                        .min(u16::MAX as usize) as u16;
                }
                KeyCode::Char('L') => self.open_logs_for_row(),
                KeyCode::Char('<') => self.nudge_split(-3),
                KeyCode::Char('>') => self.nudge_split(3),
                _ => {}
            }
            return true;
        }
        match code {
            KeyCode::Esc if self.search_active() => self.clear_filter(),
            KeyCode::Up | KeyCode::Char('k') => self.tree_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.tree_move(1),
            KeyCode::Char('g') | KeyCode::Home => {
                self.tree_sel = 0;
                self.detail_scroll = 0;
                self.request_inspect_for_selection();
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.tree_sel = self.tree_rows.len().saturating_sub(1);
                self.detail_scroll = 0;
                self.request_inspect_for_selection();
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => {
                let leaf = self
                    .selected_row()
                    .map(|r| {
                        !r.has_children
                            && matches!(r.kind, RowKind::Container(_) | RowKind::Volume(_))
                    })
                    .unwrap_or(false);
                if leaf {
                    self.focus = Focus::Detail;
                } else {
                    self.toggle_expand();
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.collapse_or_up(),
            KeyCode::Tab => self.focus = Focus::Detail,
            KeyCode::Char('<') => self.nudge_split(-3),
            KeyCode::Char('>') => self.nudge_split(3),
            KeyCode::Char('L') => self.open_logs_for_row(),
            KeyCode::Char('S') => self.container_action(ContainerAction::Start),
            KeyCode::Char('K') => self.container_action(ContainerAction::Stop),
            KeyCode::Char('R') => self.container_action(ContainerAction::Restart),
            KeyCode::Char('D') => self.confirm_container_action(ContainerAction::Remove),
            KeyCode::Char('E') => self.open_edit(),
            KeyCode::Char('t') => self.open_exec_popup(),
            _ => {}
        }
        true
    }

    fn handle_stats_key(&mut self, code: KeyCode) -> bool {
        if let Some(res) = self.handle_global_key(code) {
            return res;
        }
        let len = self.stats_rows().len();
        match code {
            KeyCode::Esc if self.search_active() => self.clear_filter(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.stats_sel = self.stats_sel.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    self.stats_sel = (self.stats_sel + 1).min(len - 1);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => self.stats_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.stats_sel = len.saturating_sub(1),
            KeyCode::Char('L') => {
                if let Some(c) = self.stats_rows().get(self.stats_sel) {
                    let t = LogTarget {
                        id: c.id.clone(),
                        name: c.name.clone(),
                    };
                    self.open_logs(vec![t]);
                }
            }
            _ => {}
        }
        true
    }

    fn handle_logs_key(&mut self, code: KeyCode) -> bool {
        if let Some(res) = self.handle_global_key(code) {
            return res;
        }
        let len = self.visible_log_lines().len();
        match code {
            KeyCode::Esc if self.search_active() => self.clear_filter(),
            KeyCode::Esc => {
                self.leave_view_streams();
                self.view = View::Tree;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.log_follow = false;
                self.log_off = (self.log_off + 1).min(len.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.log_off > 0 {
                    self.log_off -= 1;
                }
                if self.log_off == 0 {
                    self.log_follow = true;
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.log_follow = false;
                self.log_off = len.saturating_sub(1);
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.log_off = 0;
                self.log_follow = true;
            }
            KeyCode::PageUp => {
                self.log_follow = false;
                self.log_off = (self.log_off + 10).min(len.saturating_sub(1));
            }
            KeyCode::PageDown => {
                self.log_off = self.log_off.saturating_sub(10);
                if self.log_off == 0 {
                    self.log_follow = true;
                }
            }
            KeyCode::Char('f') => {
                self.log_follow = !self.log_follow;
                if self.log_follow {
                    self.log_off = 0;
                }
            }
            KeyCode::Char('s') => {
                self.popup = Popup::ContainerSelect;
                self.popup_sel = 0;
            }
            _ => {}
        }
        true
    }

    fn handle_events_key(&mut self, code: KeyCode) -> bool {
        if let Some(res) = self.handle_global_key(code) {
            return res;
        }
        let len = self.visible_events().len();
        match code {
            KeyCode::Esc if self.search_active() => self.clear_filter(),
            KeyCode::Esc => self.view = View::Tree,
            KeyCode::Up | KeyCode::Char('k') => {
                self.events_follow = false;
                self.events_off = (self.events_off + 1).min(len.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.events_off > 0 {
                    self.events_off -= 1;
                }
                if self.events_off == 0 {
                    self.events_follow = true;
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.events_follow = false;
                self.events_off = len.saturating_sub(1);
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.events_off = 0;
                self.events_follow = true;
            }
            KeyCode::Char('f') => {
                self.events_follow = !self.events_follow;
                if self.events_follow {
                    self.events_off = 0;
                }
            }
            _ => {}
        }
        true
    }
}

impl App {
    // ---- container actions ----

    fn selected_running_or_any_container(&self) -> Option<&Container> {
        self.selected_container()
    }

    fn container_action(&mut self, action: ContainerAction) {
        let Some(c) = self.selected_running_or_any_container().cloned() else {
            return;
        };
        if action == ContainerAction::Remove {
            // remove is destructive: route through confirmation
            self.confirm_container_action(action);
            return;
        }
        self.apply_action(PendingAction::Container {
            id: c.id,
            name: c.name,
            action,
        });
    }

    fn confirm_container_action(&mut self, action: ContainerAction) {
        let Some(c) = self.selected_container().cloned() else {
            return;
        };
        self.pending = Some(PendingAction::Container {
            id: c.id,
            name: c.name,
            action,
        });
        self.popup = Popup::Confirm;
    }

    fn apply_action(&mut self, action: PendingAction) {
        if self.mock {
            self.apply_action_mock(&action);
            return;
        }
        let Some(docker) = self.docker.clone() else {
            self.toast(false, "not connected".into());
            return;
        };
        match action {
            PendingAction::Container { id, name, action } => {
                let h = workers::spawn_container_action(docker, self.tx.clone(), id, name, action);
                self.workers.add(h);
                self.refresh_notify.notify_one();
            }
            PendingAction::Recreate(req) => {
                let h = workers::spawn_recreate(docker, self.tx.clone(), *req);
                self.workers.add(h);
                self.refresh_notify.notify_one();
            }
        }
    }

    fn apply_action_mock(&mut self, action: &PendingAction) {
        match action {
            PendingAction::Container { id, name, action } => {
                let Some(c) = self.containers.iter_mut().find(|c| c.id == *id) else {
                    self.toast(false, format!("{name}: not found"));
                    return;
                };
                let label = format!("{} {}", action.label(), name);
                match action {
                    ContainerAction::Start => {
                        c.state = "running".into();
                        c.status = "Up (mock)".into();
                    }
                    ContainerAction::Stop => {
                        c.state = "exited".into();
                        c.status = "Exited (0) (mock)".into();
                    }
                    ContainerAction::Restart => {
                        c.state = "running".into();
                        c.status = "Up (mock, restarted)".into();
                    }
                    ContainerAction::Remove => {
                        self.containers.retain(|x| x.id != *id);
                        self.details.remove(id);
                        self.stats.remove(id);
                        self.cpu_hist.remove(id);
                        self.mem_hist.remove(id);
                        self.rebuild_tree();
                    }
                }
                self.toast(true, format!("{label} done (mock)"));
            }
            PendingAction::Recreate(req) => {
                let Some(c) = self.containers.iter_mut().find(|c| c.id == req.old_id) else {
                    self.toast(false, "recreate: container not found".into());
                    return;
                };
                if let Some(name) = req.spec.name.clone() {
                    c.name = name;
                }
                if let Some(image) = req.spec.image.clone() {
                    c.image = image;
                }
                c.state = "running".into();
                c.status = "Up (mock, recreated)".into();
                c.ports = req
                    .spec
                    .host
                    .port_bindings
                    .as_ref()
                    .map(|pm| {
                        let mut out = Vec::new();
                        for (cport, bs) in pm {
                            for b in bs.iter().flatten() {
                                out.push(format!(
                                    "{}:{} -> {cport}",
                                    b.host_ip.as_deref().unwrap_or("0.0.0.0"),
                                    b.host_port.as_deref().unwrap_or("?")
                                ));
                            }
                        }
                        out
                    })
                    .unwrap_or_default();
                if let Some(d) = self.details.get_mut(&req.old_id) {
                    d.env = req.spec.env.clone();
                    d.privileged = req.spec.host.privileged.unwrap_or(false);
                    d.gpus = crate::actions::gpu_summary(&req.spec.host);
                    d.network_mode = req.spec.host.network_mode.clone();
                    d.pid_mode = req.spec.host.pid_mode.clone();
                    d.ipc_mode = req.spec.host.ipc_mode.clone();
                    if let Some(user) = req.spec.user.clone() {
                        d.user = Some(user);
                    }
                    d.spec = Some(req.spec.clone());
                }
                self.rebuild_tree();
                self.toast(
                    true,
                    format!(
                        "recreate {} done (mock)",
                        req.spec.name.as_deref().unwrap_or(&req.old_name)
                    ),
                );
            }
        }
    }

    fn open_edit(&mut self) {
        let Some(c) = self.selected_container().cloned() else {
            return;
        };
        if self.mock {
            if let Some(d) = self.details.get(&c.id)
                && let Some(spec) = &d.spec
            {
                self.edit = Some(EditForm::from_spec(&c.id, spec, c.is_running()));
                self.popup = Popup::Edit;
            } else {
                self.toast(false, "no inspect data for edit (mock)".into());
            }
            return;
        }
        if let Some(d) = self.details.get(&c.id)
            && let Some(spec) = &d.spec
        {
            self.edit = Some(EditForm::from_spec(&c.id, spec, c.is_running()));
            self.popup = Popup::Edit;
        } else if self.docker.is_some() {
            self.edit_pending = Some(c.id.clone());
            self.request_inspect(&c.id);
            self.toast(true, format!("inspecting {}…", c.name));
        }
    }

    fn open_edit_form(&mut self, id: &str) {
        let Some(c) = self.container_by_id(id).cloned() else {
            return;
        };
        if let Some(d) = self.details.get(id)
            && let Some(spec) = &d.spec
        {
            self.edit = Some(EditForm::from_spec(id, spec, c.is_running()));
            self.popup = Popup::Edit;
        } else {
            self.toast(false, "no inspect data for edit".into());
        }
    }

    // ---- exec ----

    fn open_exec_popup(&mut self) {
        let Some(c) = self.selected_container() else {
            return;
        };
        if !c.is_running() {
            self.toast(false, format!("{} is not running", c.name));
            return;
        }
        self.popup = Popup::Exec;
        self.popup_sel = 0;
        self.exec_user_input = None;
    }

    fn launch_exec(&mut self, user: Option<String>) {
        let Some(c) = self.selected_container().cloned() else {
            return;
        };
        if self.mock || self.docker.is_none() {
            self.toast(false, "exec needs a real docker connection".into());
            return;
        }
        let Some(tx) = self.exec_tx.clone() else {
            self.toast(false, "exec unavailable".into());
            return;
        };
        let _ = tx.send(TerminalRequest::Exec(ExecRequest {
            id: c.id,
            name: c.name.clone(),
            user,
        }));
    }

    // ---- ops ----

    fn run_op(&mut self, name: String, op: Op) {
        if self.mock {
            let cname = op.container_name().unwrap_or_else(|| "unnamed".into());
            let id = format!(
                "mock{:x}",
                (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64)
                    .unwrap_or(0))
                    & 0xffff_ffff
            );
            let ports = op
                .ports
                .iter()
                .filter_map(|p| {
                    ops::parse_port(p).map(|(k, b)| {
                        format!(
                            "{}:{} -> {k}",
                            b.host_ip.as_deref().unwrap_or("0.0.0.0"),
                            b.host_port.as_deref().unwrap_or("?")
                        )
                    })
                })
                .collect();
            self.containers.push(Container {
                id: id.clone(),
                name: cname.clone(),
                image: op.image().to_string(),
                state: "running".into(),
                status: "Up (mock op)".into(),
                created: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                command: op
                    .command
                    .as_ref()
                    .map(|c| match c {
                        ops::Cmd::Many(v) => v.join(" "),
                        ops::Cmd::One(s) => s.clone(),
                    })
                    .unwrap_or_default(),
                labels: op.labels.clone(),
                ports,
            });
            self.rebuild_tree();
            self.toast(true, format!("op {name} started {cname} (mock)"));
            if op.attach {
                self.toast(false, "attach needs a real docker connection".into());
            }
            if op.follow_logs {
                let t = LogTarget { id, name: cname };
                self.open_logs(vec![t]);
            }
            return;
        }
        if op.attach {
            let Some(tx) = self.exec_tx.clone() else {
                self.toast(false, "attach unavailable".into());
                return;
            };
            let (cname, config) = op.to_create();
            let _ = tx.send(TerminalRequest::AttachRun {
                name: cname.unwrap_or_default(),
                config: Box::new(config),
            });
            return;
        }
        let Some(docker) = self.docker.clone() else {
            self.toast(false, "not connected".into());
            return;
        };
        let h = workers::spawn_run_op(docker, self.tx.clone(), name.clone(), op);
        self.workers.add(h);
        self.toast(true, format!("running op {name}…"));
    }

    // ---- popups ----

    fn close_popup(&mut self) {
        self.popup = Popup::None;
        self.pending = None;
        self.edit = None;
        self.env_editor = None;
        self.exec_user_input = None;
    }

    fn handle_popup_key(&mut self, code: KeyCode) -> bool {
        // typing a custom exec user name
        if self.popup == Popup::Exec
            && let Some(input) = &mut self.exec_user_input
        {
            match code {
                KeyCode::Esc => self.exec_user_input = None,
                KeyCode::Enter => {
                    let user = input.value.trim().to_string();
                    self.popup = Popup::None;
                    self.exec_user_input = None;
                    self.launch_exec((!user.is_empty()).then_some(user));
                }
                KeyCode::Backspace => input.backspace(),
                KeyCode::Delete => input.delete(),
                KeyCode::Left => input.left(),
                KeyCode::Right => input.right(),
                KeyCode::Home => input.home(),
                KeyCode::End => input.end(),
                KeyCode::Char(c) => input.insert(c),
                _ => {}
            }
            return true;
        }

        if self.popup == Popup::Edit {
            return self.handle_edit_key(code);
        }

        if self.popup == Popup::Confirm {
            match code {
                KeyCode::Esc | KeyCode::Char('n') => self.close_popup(),
                KeyCode::Enter | KeyCode::Char('y') => {
                    if let Some(action) = self.pending.take() {
                        self.popup = Popup::None;
                        self.apply_action(action);
                    } else {
                        self.close_popup();
                    }
                }
                _ => {}
            }
            return true;
        }

        let list_len = match self.popup {
            Popup::Context => self.contexts.len(),
            Popup::ContainerSelect => self.containers.len(),
            Popup::Ops => self.ops.len(),
            Popup::Exec => 3,
            Popup::Help | Popup::Confirm | Popup::Edit | Popup::None => 0,
        };
        match code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_sel = self.popup_sel.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if list_len > 0 {
                    self.popup_sel = (self.popup_sel + 1).min(list_len - 1);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => self.popup_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.popup_sel = list_len.saturating_sub(1),
            KeyCode::Enter => self.activate_popup(),
            _ => {}
        }
        true
    }

    fn handle_edit_key(&mut self, code: KeyCode) -> bool {
        if self.env_editor.is_some() {
            return self.handle_env_key(code);
        }
        let Some(form) = &mut self.edit else {
            self.close_popup();
            return true;
        };
        let n_fields = form.fields.len();
        let selected_label = form.fields.get(form.sel).map(|f| f.label);
        let is_text = form
            .fields
            .get(form.sel)
            .is_some_and(|f| matches!(f.value, crate::actions::FormValue::Text(_)));
        match code {
            KeyCode::Esc => {
                self.close_popup();
                return true;
            }
            KeyCode::Up => {
                if form.sel > 0 {
                    form.sel -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if form.sel < n_fields {
                    form.sel += 1;
                }
            }
            KeyCode::Enter => {
                if form.sel == n_fields {
                    // APPLY row
                    let req = form.to_request();
                    self.close_popup();
                    self.apply_action(PendingAction::Recreate(Box::new(req)));
                    return true;
                }
                if selected_label == Some("Env") {
                    let text = form.text("Env").to_string();
                    self.env_editor = Some(EnvEditor::from_text(&text));
                    return true;
                }
                if is_text {
                    form.sel += 1;
                } else {
                    form.cycle_selected();
                }
            }
            KeyCode::Char(' ') if !is_text => form.cycle_selected(),
            KeyCode::Backspace if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.backspace();
                }
            }
            KeyCode::Delete if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.delete();
                }
            }
            KeyCode::Left if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.left();
                }
            }
            KeyCode::Right if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.right();
                }
            }
            KeyCode::Home if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.home();
                }
            }
            KeyCode::End if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.end();
                }
            }
            KeyCode::Char(c) if is_text => {
                if let Some(crate::actions::FormValue::Text(t)) =
                    form.fields.get_mut(form.sel).map(|f| &mut f.value)
                {
                    t.insert(c);
                }
            }
            _ => {}
        }
        true
    }

    /// Commit the env editor back into the form's Env field and close it.
    fn close_env_editor(&mut self) {
        let text = self.env_editor.take().map(|ed| ed.to_text());
        if let Some(text) = text
            && let Some(form) = &mut self.edit
            && let Some(f) = form.fields.iter_mut().find(|f| f.label == "Env")
        {
            f.value = crate::actions::FormValue::Text(TextInput::new(text));
        }
    }

    fn handle_env_key(&mut self, code: KeyCode) -> bool {
        if code == KeyCode::Esc {
            if self
                .env_editor
                .as_ref()
                .is_some_and(|e| e.editing.is_some())
            {
                if let Some(e) = &mut self.env_editor {
                    e.editing = None;
                }
            } else {
                self.close_env_editor();
            }
            return true;
        }
        let Some(ed) = &mut self.env_editor else {
            return true;
        };
        if let Some(col) = ed.editing {
            let cell = match col {
                EnvCol::Key => &mut ed.rows[ed.sel].key,
                EnvCol::Value => &mut ed.rows[ed.sel].value,
            };
            match code {
                KeyCode::Enter | KeyCode::Tab => {
                    ed.editing = match col {
                        EnvCol::Key => Some(EnvCol::Value),
                        EnvCol::Value => None,
                    };
                }
                KeyCode::Up => {
                    ed.editing = None;
                    ed.sel = ed.sel.saturating_sub(1);
                }
                KeyCode::Down => {
                    ed.editing = None;
                    if ed.sel + 1 < ed.rows.len() {
                        ed.sel += 1;
                    }
                }
                KeyCode::Backspace => cell.backspace(),
                KeyCode::Delete => cell.delete(),
                KeyCode::Left => cell.left(),
                KeyCode::Right => cell.right(),
                KeyCode::Home => cell.home(),
                KeyCode::End => cell.end(),
                KeyCode::Char(c) => cell.insert(c),
                _ => {}
            }
            return true;
        }
        let len = ed.rows.len();
        match code {
            KeyCode::Up | KeyCode::Char('k') => ed.sel = ed.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 1 {
                    ed.sel = (ed.sel + 1).min(len - 1);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => ed.sel = 0,
            KeyCode::Char('G') | KeyCode::End => ed.sel = len.saturating_sub(1),
            KeyCode::Enter => ed.editing = Some(EnvCol::Key),
            KeyCode::Tab => ed.editing = Some(EnvCol::Value),
            KeyCode::Char('a') => ed.add_row(),
            KeyCode::Char('d') | KeyCode::Delete => ed.delete_row(),
            _ => {}
        }
        true
    }

    fn activate_popup(&mut self) {
        match self.popup {
            Popup::Context => {
                let sel = self.popup_sel;
                self.close_popup();
                if sel != self.current_ctx {
                    self.current_ctx = sel;
                    self.bootstrap();
                }
            }
            Popup::ContainerSelect => {
                let sel = self.popup_sel;
                self.close_popup();
                if let Some(c) = self.containers.get(sel) {
                    let t = LogTarget {
                        id: c.id.clone(),
                        name: c.name.clone(),
                    };
                    self.open_logs(vec![t]);
                }
            }
            Popup::Help => self.close_popup(),
            Popup::Ops => {
                let sel = self.popup_sel;
                self.close_popup();
                if let Some((name, op)) = self.ops.get(sel).cloned() {
                    self.run_op(name, op);
                }
            }
            Popup::Exec => match self.popup_sel {
                0 => {
                    self.close_popup();
                    self.launch_exec(None);
                }
                1 => {
                    self.close_popup();
                    self.launch_exec(Some("root".into()));
                }
                _ => {
                    self.exec_user_input = Some(TextInput::new(String::new()));
                }
            },
            Popup::Confirm | Popup::Edit | Popup::None => {}
        }
    }

    // ---- mouse ----

    pub fn handle_mouse(&mut self, ev: MouseEvent) {
        if self.search.prompting {
            return;
        }
        match ev.kind {
            MouseEventKind::ScrollUp => self.mouse_scroll(ev.column, ev.row, -1),
            MouseEventKind::ScrollDown => self.mouse_scroll(ev.column, ev.row, 1),
            MouseEventKind::Down(MouseButton::Left) => self.mouse_left(ev.column, ev.row, true),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_left(ev.column, ev.row, false),
            MouseEventKind::Up(MouseButton::Left) => self.dragging_split = false,
            _ => {}
        }
    }

    fn mouse_scroll(&mut self, col: u16, row: u16, dir: i32) {
        if self.popup != Popup::None {
            if self.popup == Popup::Edit && self.env_editor.is_some() {
                if contains(self.areas.popup_list, col, row)
                    && let Some(ed) = &mut self.env_editor
                {
                    let len = ed.rows.len();
                    if len > 1 {
                        ed.sel = (ed.sel as i32 + dir).clamp(0, len as i32 - 1) as usize;
                        ed.editing = None;
                    }
                }
                return;
            }
            if contains(self.areas.popup_list, col, row) {
                if self.popup == Popup::Edit {
                    if let Some(form) = &mut self.edit {
                        let new = (form.sel as i32 + dir).clamp(0, form.fields.len() as i32);
                        form.sel = new as usize;
                    }
                } else {
                    let len = match self.popup {
                        Popup::Context => self.contexts.len(),
                        Popup::ContainerSelect => self.containers.len(),
                        Popup::Ops => self.ops.len(),
                        Popup::Exec => 3,
                        _ => 0,
                    };
                    if len > 0 {
                        self.popup_sel =
                            (self.popup_sel as i32 + dir).clamp(0, len as i32 - 1) as usize;
                    }
                }
            }
            return;
        }
        match self.view {
            View::Tree => {
                if contains(self.areas.detail, col, row) {
                    if dir < 0 {
                        self.detail_scroll = self.detail_scroll.saturating_sub(1);
                    } else {
                        let max = crate::ui::detail::detail_line_count(self).saturating_sub(1);
                        if (self.detail_scroll as usize) < max {
                            self.detail_scroll += 1;
                        }
                    }
                } else if contains(self.areas.tree, col, row) {
                    self.tree_move(dir);
                }
            }
            View::Stats => {
                if contains(self.areas.stats_table, col, row) {
                    let len = self.stats_rows().len();
                    if len > 0 {
                        self.stats_sel =
                            (self.stats_sel as i32 + dir).clamp(0, len as i32 - 1) as usize;
                    }
                }
            }
            View::Logs => {
                let len = self.visible_log_lines().len();
                if len > 0 {
                    if dir < 0 {
                        self.log_follow = false;
                        self.log_off = (self.log_off + 1).min(len - 1);
                    } else if self.log_off > 0 {
                        self.log_off -= 1;
                        if self.log_off == 0 {
                            self.log_follow = true;
                        }
                    }
                }
            }
            View::Events => {
                let len = self.visible_events().len();
                if len > 0 {
                    if dir < 0 {
                        self.events_follow = false;
                        self.events_off = (self.events_off + 1).min(len - 1);
                    } else if self.events_off > 0 {
                        self.events_off -= 1;
                        if self.events_off == 0 {
                            self.events_follow = true;
                        }
                    }
                }
            }
        }
    }

    fn mouse_left(&mut self, col: u16, row: u16, is_down: bool) {
        if self.popup != Popup::None {
            if self.popup == Popup::Edit && self.env_editor.is_some() {
                if contains(self.areas.popup, col, row) {
                    self.mouse_env_select(col, row);
                } else if is_down {
                    self.close_env_editor();
                }
                return;
            }
            if contains(self.areas.popup, col, row) {
                self.mouse_popup_select(col, row, is_down);
            } else if is_down {
                self.close_popup();
            }
            return;
        }
        match self.view {
            View::Tree => {
                if self.dragging_split {
                    self.set_split_from_col(col);
                    return;
                }
                let a = self.areas.tree;
                let div = a.x + a.width;
                if is_down && col + 1 >= div && col <= div + 1 && row >= a.y && row < a.y + a.height
                {
                    self.dragging_split = true;
                    self.set_split_from_col(col);
                    return;
                }
                if contains(a, col, row) && row > a.y && row < a.y + a.height.saturating_sub(1) {
                    let offset = self.tree_state.offset();
                    let idx = offset + (row - a.y - 1) as usize;
                    if idx < self.tree_rows.len() {
                        if is_down && idx == self.tree_sel {
                            // second click on the selected row acts as enter
                            let leaf = {
                                let r = &self.tree_rows[idx];
                                !r.has_children
                                    && matches!(r.kind, RowKind::Container(_) | RowKind::Volume(_))
                            };
                            if leaf {
                                self.focus = Focus::Detail;
                            } else {
                                self.toggle_expand();
                            }
                        } else {
                            self.tree_sel = idx;
                            self.detail_scroll = 0;
                            self.request_inspect_for_selection();
                        }
                    }
                } else if contains(self.areas.detail, col, row) && is_down {
                    self.focus = Focus::Detail;
                }
            }
            View::Stats => {
                let a = self.areas.stats_table;
                if contains(a, col, row) && row > a.y + 1 && row < a.y + a.height.saturating_sub(1)
                {
                    let offset = self.stats_state.offset();
                    let idx = offset + (row - a.y - 2) as usize;
                    let len = self.stats_rows().len();
                    if idx < len {
                        self.stats_sel = idx;
                    }
                }
            }
            View::Logs => {
                let a = self.areas.logs;
                if contains(a, col, row) && row > a.y && row < a.y + a.height.saturating_sub(1) {
                    let len = self.visible_log_lines().len();
                    let h = a.height.saturating_sub(2) as usize;
                    let end = len.saturating_sub(self.log_off);
                    let start = end.saturating_sub(h);
                    let p = (row - a.y - 1) as usize;
                    let abs = start + p;
                    if abs < len {
                        self.log_follow = false;
                        self.log_off = len - 1 - abs;
                        if self.log_off == 0 {
                            self.log_follow = true;
                        }
                    }
                }
            }
            View::Events => {
                let a = self.areas.events;
                if contains(a, col, row) && row > a.y && row < a.y + a.height.saturating_sub(1) {
                    let len = self.visible_events().len();
                    let h = a.height.saturating_sub(2) as usize;
                    let end = len.saturating_sub(self.events_off);
                    let start = end.saturating_sub(h);
                    let p = (row - a.y - 1) as usize;
                    let abs = start + p;
                    if abs < len {
                        self.events_follow = false;
                        self.events_off = len - 1 - abs;
                        if self.events_off == 0 {
                            self.events_follow = true;
                        }
                    }
                }
            }
        }
    }

    fn mouse_popup_select(&mut self, col: u16, row: u16, is_down: bool) {
        let a = self.areas.popup_list;
        if !contains(a, col, row) {
            return;
        }
        let offset = self.popup_state.offset();
        let idx = offset + (row - a.y) as usize;
        if self.popup == Popup::Edit {
            let Some(form) = &mut self.edit else {
                return;
            };
            let p = (row - a.y) as usize;
            let field = p.saturating_sub(2);
            if field > form.fields.len() {
                return;
            }
            let is_env = field < form.fields.len() && form.fields[field].label == "Env";
            form.sel = field;
            if is_env && is_down {
                let text = form.text("Env").to_string();
                self.env_editor = Some(EnvEditor::from_text(&text));
            }
            return;
        }
        let len = match self.popup {
            Popup::Context => self.contexts.len(),
            Popup::ContainerSelect => self.containers.len(),
            Popup::Ops => self.ops.len(),
            Popup::Exec => 3,
            Popup::Help | Popup::Confirm | Popup::Edit | Popup::None => 0,
        };
        if idx < len {
            self.popup_sel = idx;
        }
    }

    /// Click inside the env editor table: select the clicked row and start
    /// editing the cell (key vs value) that was clicked.
    fn mouse_env_select(&mut self, col: u16, row: u16) {
        let a = self.areas.popup_list;
        if !contains(a, col, row) {
            return;
        }
        let idx = (row - a.y) as usize;
        let cell = if col < self.areas.env_split_x {
            EnvCol::Key
        } else {
            EnvCol::Value
        };
        if let Some(ed) = &mut self.env_editor
            && idx < ed.rows.len()
        {
            ed.sel = idx;
            ed.editing = Some(cell);
        }
    }
}
