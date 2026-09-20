use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::actions::{ContainerAction, EnvCol, EnvEditor, TextInput};
use crate::exec::{ExecRequest, TerminalRequest};
use crate::ops::{self, Op};
use crate::workers::LogTarget;

use super::*;

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
            View::Files => self.handle_files_key(code),
        }
    }

    pub(crate) fn refresh(&mut self) {
        self.inspect_sent.clear();
        self.refresh_notify.notify_one();
        if self.view == View::Stats {
            self.stats_spawned.clear();
            self.sync_stats_tasks();
        }
        if self.view == View::Files {
            self.request_files();
        }
        self.request_inspect_for_selection();
    }

    pub(crate) fn handle_search_key(&mut self, key: KeyEvent) -> bool {
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

    pub(crate) fn handle_global_key(&mut self, code: KeyCode) -> Option<bool> {
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

    pub(crate) fn toggle_expand(&mut self) {
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

    pub(crate) fn collapse_or_up(&mut self) {
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

    pub(crate) fn tree_move(&mut self, delta: i32) {
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

    pub(crate) fn nudge_split(&mut self, delta: i16) {
        self.split_pct = (self.split_pct as i16 + delta).clamp(20, 80) as u16;
    }

    pub(crate) fn set_split_from_col(&mut self, col: u16) {
        let t = self.areas.tree;
        let d = self.areas.detail;
        let full = (t.width + d.width).max(1) as u32;
        let rel = col.saturating_sub(t.x) as u32;
        self.split_pct = ((rel * 100) / full).clamp(20, 80) as u16;
    }

    pub(crate) fn handle_tree_key(&mut self, code: KeyCode) -> bool {
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
                KeyCode::Char('f') => self.open_files_for_row(),
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
            KeyCode::Char('f') => self.open_files_for_row(),
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

    pub(crate) fn handle_stats_key(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn handle_logs_key(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn handle_events_key(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn handle_files_key(&mut self, code: KeyCode) -> bool {
        if let Some(res) = self.handle_global_key(code) {
            return res;
        }
        let len = self.visible_files().len();
        match code {
            KeyCode::Esc if self.search_active() => self.clear_filter(),
            KeyCode::Esc => {
                self.leave_view_streams();
                self.view = View::Tree;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(f) = &mut self.files {
                    f.sel = f.sel.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0
                    && let Some(f) = &mut self.files
                {
                    f.sel = (f.sel + 1).min(len - 1);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if let Some(f) = &mut self.files {
                    f.sel = 0;
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if let Some(f) = &mut self.files {
                    f.sel = len.saturating_sub(1);
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let sel = self.files.as_ref().map(|f| f.sel).unwrap_or(0);
                if let Some(e) = self.visible_files().get(sel) {
                    let name = e.name.clone();
                    if e.dir {
                        self.files_enter(&name);
                    }
                }
            }
            KeyCode::Char('r') => self.request_files(),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
                let parent = self
                    .files
                    .as_ref()
                    .filter(|f| f.path != "/")
                    .map(|f| parent_path(&f.path));
                if let Some(parent) = parent {
                    self.files_navigate(&parent);
                }
            }
            _ => {}
        }
        true
    }
    // ---- container actions ----

    pub(crate) fn selected_running_or_any_container(&self) -> Option<&Container> {
        self.selected_container()
    }

    pub(crate) fn container_action(&mut self, action: ContainerAction) {
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

    pub(crate) fn confirm_container_action(&mut self, action: ContainerAction) {
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

    pub(crate) fn apply_action(&mut self, action: PendingAction) {
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

    pub(crate) fn apply_action_mock(&mut self, action: &PendingAction) {
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

    pub(crate) fn open_edit(&mut self) {
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

    pub(crate) fn open_edit_form(&mut self, id: &str) {
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

    pub(crate) fn open_exec_popup(&mut self) {
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

    pub(crate) fn launch_exec(&mut self, user: Option<String>) {
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

    pub(crate) fn run_op(&mut self, name: String, op: Op) {
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

    pub(crate) fn close_popup(&mut self) {
        self.popup = Popup::None;
        self.pending = None;
        self.edit = None;
        self.env_editor = None;
        self.exec_user_input = None;
    }

    pub(crate) fn handle_popup_key(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn handle_edit_key(&mut self, code: KeyCode) -> bool {
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
    pub(crate) fn close_env_editor(&mut self) {
        let text = self.env_editor.take().map(|ed| ed.to_text());
        if let Some(text) = text
            && let Some(form) = &mut self.edit
            && let Some(f) = form.fields.iter_mut().find(|f| f.label == "Env")
        {
            f.value = crate::actions::FormValue::Text(TextInput::new(text));
        }
    }

    pub(crate) fn handle_env_key(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn activate_popup(&mut self) {
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
}

/// Parent directory of a files-view path: "/" -> "/", "/etc" -> "/",
/// "/etc/ssl" -> "/etc" (trailing slashes tolerated).
fn parent_path(p: &str) -> String {
    let trimmed = p.strip_suffix('/').unwrap_or(p);
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => trimmed[..i].into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parent_path;

    #[test]
    fn parent_paths() {
        assert_eq!(parent_path("/"), "/");
        assert_eq!(parent_path("/etc"), "/");
        assert_eq!(parent_path("/etc/ssl"), "/etc");
        assert_eq!(parent_path("/var/log/nginx/"), "/var/log");
    }
}
