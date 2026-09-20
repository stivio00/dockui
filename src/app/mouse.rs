use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::actions::EnvCol;

use super::*;

impl App {
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

    pub(crate) fn mouse_scroll(&mut self, col: u16, row: u16, dir: i32) {
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
            View::Files => {
                let len = self.visible_files().len();
                if len > 0
                    && let Some(f) = &mut self.files
                {
                    f.sel = (f.sel as i32 + dir).clamp(0, len as i32 - 1) as usize;
                }
            }
        }
    }

    pub(crate) fn mouse_left(&mut self, col: u16, row: u16, is_down: bool) {
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
            View::Files => {
                let a = self.areas.files;
                if contains(a, col, row) && row > a.y + 1 && row < a.y + a.height.saturating_sub(1)
                {
                    let idx = {
                        let Some(f) = &mut self.files else {
                            return;
                        };
                        f.state.offset() + (row - a.y - 2) as usize
                    };
                    let len = self.visible_files().len();
                    if idx >= len {
                        return;
                    }
                    let already = self.files.as_ref().is_some_and(|f| f.sel == idx);
                    if let Some(f) = &mut self.files {
                        f.sel = idx;
                    }
                    if already && is_down {
                        let e = self.visible_files()[idx].clone();
                        if e.dir {
                            self.files_enter(&e.name);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn mouse_popup_select(&mut self, col: u16, row: u16, is_down: bool) {
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
    pub(crate) fn mouse_env_select(&mut self, col: u16, row: u16) {
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
