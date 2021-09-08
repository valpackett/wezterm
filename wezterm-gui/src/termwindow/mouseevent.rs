use crate::tabbar::TabBarItem;
use crate::termwindow::keyevent::window_mods_to_termwiz_mods;
use crate::termwindow::{PositionedSplit, ScrollHit, UIItem, UIItemType, TMB};
use ::window::{
    MouseButtons as WMB, MouseCursor, MouseEvent, MouseEventKind as WMEK, MousePress, WindowOps,
};
use config::keyassignment::{MouseEventTrigger, SpawnTabDomain};
use mux::pane::Pane;
use mux::tab::SplitDirection;
use mux::Mux;
use std::convert::TryInto;
use std::ops::Sub;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use wezterm_term::input::MouseEventKind as TMEK;
use wezterm_term::{LastMouseClick, StableRowIndex};

impl super::TermWindow {
    fn resolve_ui_item(&self, event: &MouseEvent) -> Option<UIItem> {
        let x = event.coords.x;
        let y = event.coords.y;
        self.ui_items
            .iter()
            .rev()
            .find(|item| {
                x >= item.x as isize
                    && x <= (item.x + item.width) as isize
                    && y >= item.y as isize
                    && y <= (item.y + item.height) as isize
            })
            .cloned()
    }

    fn leave_ui_item(&mut self, item: &UIItem) {
        match item.item_type {
            UIItemType::TabBar => {
                self.update_title_post_status();
            }
            UIItemType::AboveScrollThumb
            | UIItemType::BelowScrollThumb
            | UIItemType::ScrollThumb
            | UIItemType::Split(_) => {}
        }
    }

    fn enter_ui_item(&mut self, item: &UIItem) {
        match item.item_type {
            UIItemType::TabBar => {}
            UIItemType::AboveScrollThumb
            | UIItemType::BelowScrollThumb
            | UIItemType::ScrollThumb
            | UIItemType::Split(_) => {}
        }
    }

    pub fn mouse_event_impl(&mut self, event: MouseEvent, context: &dyn WindowOps) {
        let pane = match self.get_active_pane_or_overlay() {
            Some(pane) => pane,
            None => return,
        };

        self.current_mouse_event.replace(event.clone());

        let config = &self.config;
        let y = (event
            .coords
            .y
            .sub(config.window_padding.top as isize)
            .max(0)
            / self.render_metrics.cell_size.height) as i64;

        let first_line_offset = if self.show_tab_bar && !self.config.tab_bar_at_bottom {
            1
        } else {
            0
        };
        let tab_bar_y = if self.config.tab_bar_at_bottom {
            let num_rows = self
                .dimensions
                .pixel_height
                .sub((config.window_padding.top + config.window_padding.bottom) as usize)
                / self.render_metrics.cell_size.height as usize;
            num_rows - 1
        } else {
            0
        } as i64;
        let in_tab_bar = self.show_tab_bar && y == tab_bar_y && event.coords.y >= 0;

        let x = (event
            .coords
            .x
            .sub(config.window_padding.left as isize)
            .max(0) as f32)
            / self.render_metrics.cell_size.width as f32;
        let x = if !in_tab_bar && !pane.is_mouse_grabbed() {
            // Round the x coordinate so that we're a bit more forgiving of
            // the horizontal position when selecting cells
            x.round()
        } else {
            x
        }
        .trunc() as usize;

        self.last_mouse_coords = (x, y);

        // y position relative to top of viewport (not including tab bar)
        let term_y = y.saturating_sub(first_line_offset);

        match event.kind {
            WMEK::Release(ref press) => {
                self.current_mouse_buttons.retain(|p| p != press);
                if press == &MousePress::Left && self.window_drag_position.take().is_some() {
                    // Completed a window drag
                    return;
                }
                if press == &MousePress::Left && self.dragging.take().is_some() {
                    // Completed a drag
                    return;
                }
            }

            WMEK::Press(ref press) => {
                // Perform click counting
                let button = mouse_press_to_tmb(press);

                let click = match self.last_mouse_click.take() {
                    None => LastMouseClick::new(button),
                    Some(click) => click.add(button),
                };
                self.last_mouse_click = Some(click);
                self.current_mouse_buttons.retain(|p| p != press);
                self.current_mouse_buttons.push(*press);
            }

            WMEK::VertWheel(amount) if !pane.is_mouse_grabbed() && !pane.is_alt_screen_active() => {
                // adjust viewport
                let dims = pane.get_dimensions();
                let position = self
                    .get_viewport(pane.pane_id())
                    .unwrap_or(dims.physical_top)
                    .saturating_sub(amount.into());
                self.set_viewport(pane.pane_id(), Some(position), dims);
                context.invalidate();
                return;
            }

            WMEK::Move => {
                if let Some(start) = self.window_drag_position.as_ref() {
                    // Dragging the window
                    // Compute the distance since the initial event
                    let delta_x = start.screen_coords.x - event.screen_coords.x;
                    let delta_y = start.screen_coords.y - event.screen_coords.y;

                    // Now compute a new window position.
                    // We don't have a direct way to get the position,
                    // but we can infer it by comparing the mouse coords
                    // with the screen coords in the initial event.
                    // This computes the original top_left position,
                    // and applies the total drag delta to it.
                    let top_left = ::window::ScreenPoint::new(
                        (start.screen_coords.x - start.coords.x) - delta_x,
                        (start.screen_coords.y - start.coords.y) - delta_y,
                    );
                    // and now tell the window to go there
                    context.set_window_position(top_left);
                    return;
                }

                if let Some((item, start_event)) = self.dragging.take() {
                    self.drag_ui_item(item, start_event, x, term_y, event, context);
                    return;
                }
            }
            _ => {}
        }

        let ui_item = self.resolve_ui_item(&event);

        match (self.last_ui_item.take(), &ui_item) {
            (Some(prior), Some(item)) => {
                self.leave_ui_item(&prior);
                self.enter_ui_item(item);
            }
            (Some(prior), None) => {
                self.leave_ui_item(&prior);
            }
            (None, Some(item)) => {
                self.enter_ui_item(item);
            }
            (None, None) => {}
        }

        if let Some(item) = ui_item {
            self.mouse_event_ui_item(item, pane, x, term_y, event, context);
        } else {
            self.mouse_event_terminal(pane, x, term_y, event, context);
        }
    }

    fn drag_split(
        &mut self,
        mut item: UIItem,
        split: PositionedSplit,
        start_event: MouseEvent,
        x: usize,
        y: i64,
        context: &dyn WindowOps,
    ) {
        let mux = Mux::get().unwrap();
        let tab = match mux.get_active_tab_for_window(self.mux_window_id) {
            Some(tab) => tab,
            None => return,
        };
        let delta = match split.direction {
            SplitDirection::Horizontal => (x as isize).saturating_sub(split.left as isize),
            SplitDirection::Vertical => (y as isize).saturating_sub(split.top as isize),
        };

        if delta != 0 {
            tab.resize_split_by(split.index, delta);
            if let Some(split) = tab.iter_splits().into_iter().nth(split.index) {
                item.item_type = UIItemType::Split(split);
                context.invalidate();
            }
        }
        self.dragging.replace((item, start_event));
    }

    fn drag_scroll_thumb(
        &mut self,
        item: UIItem,
        start_event: MouseEvent,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        let pane = match self.get_active_pane_or_overlay() {
            Some(pane) => pane,
            None => return,
        };

        let dims = pane.get_dimensions();
        let current_viewport = self.get_viewport(pane.pane_id());

        let from_top = start_event.coords.y.saturating_sub(item.y as isize);
        let effective_thumb_top = event.coords.y.saturating_sub(from_top).max(0) as usize;

        // Convert thumb top into a row index by reversing the math
        // in ScrollHit::thumb
        let row = ScrollHit::thumb_top_to_scroll_top(
            effective_thumb_top,
            &*pane,
            current_viewport,
            &self.dimensions,
        );
        self.set_viewport(pane.pane_id(), Some(row), dims);
        context.invalidate();
        self.dragging.replace((item, start_event));
    }

    fn drag_ui_item(
        &mut self,
        item: UIItem,
        start_event: MouseEvent,
        x: usize,
        y: i64,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        match item.item_type {
            UIItemType::Split(split) => {
                self.drag_split(item, split, start_event, x, y, context);
            }
            UIItemType::ScrollThumb => {
                self.drag_scroll_thumb(item, start_event, event, context);
            }
            _ => {
                log::error!("drag not implemented for {:?}", item);
            }
        }
    }

    fn mouse_event_ui_item(
        &mut self,
        item: UIItem,
        pane: Rc<dyn Pane>,
        x: usize,
        _y: i64,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        self.last_ui_item.replace(item.clone());
        match item.item_type {
            UIItemType::TabBar => {
                self.mouse_event_tab_bar(x, event, context);
            }
            UIItemType::AboveScrollThumb => {
                self.mouse_event_above_scroll_thumb(item, pane, event, context);
            }
            UIItemType::ScrollThumb => {
                self.mouse_event_scroll_thumb(item, pane, event, context);
            }
            UIItemType::BelowScrollThumb => {
                self.mouse_event_below_scroll_thumb(item, pane, event, context);
            }
            UIItemType::Split(split) => {
                self.mouse_event_split(item, split, event, context);
            }
        }
    }

    pub fn mouse_event_tab_bar(&mut self, x: usize, event: MouseEvent, context: &dyn WindowOps) {
        match event.kind {
            WMEK::Press(MousePress::Left) => match self.tab_bar.hit_test(x) {
                TabBarItem::Tab(tab_idx) => {
                    self.activate_tab(tab_idx as isize).ok();
                }
                TabBarItem::NewTabButton => {
                    self.spawn_tab(&SpawnTabDomain::CurrentPaneDomain);
                }
                TabBarItem::None => {
                    // Potentially starting a drag by the tab bar
                    self.window_drag_position.replace(event.clone());
                    context.request_drag_move();
                }
            },
            WMEK::Press(MousePress::Middle) => match self.tab_bar.hit_test(x) {
                TabBarItem::Tab(tab_idx) => {
                    self.close_tab_idx(tab_idx).ok();
                }
                TabBarItem::NewTabButton | TabBarItem::None => {}
            },
            WMEK::Press(MousePress::Right) => match self.tab_bar.hit_test(x) {
                TabBarItem::Tab(_) => {
                    self.show_tab_navigator();
                }
                TabBarItem::NewTabButton => {
                    self.show_launcher();
                }
                TabBarItem::None => {}
            },
            _ => {}
        }
        self.update_title_post_status();
        context.set_cursor(Some(MouseCursor::Arrow));
    }

    pub fn mouse_event_above_scroll_thumb(
        &mut self,
        _item: UIItem,
        pane: Rc<dyn Pane>,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        if let WMEK::Press(MousePress::Left) = event.kind {
            let dims = pane.get_dimensions();
            let current_viewport = self.get_viewport(pane.pane_id());
            // Page up
            self.set_viewport(
                pane.pane_id(),
                Some(
                    current_viewport
                        .unwrap_or(dims.physical_top)
                        .saturating_sub(self.terminal_size.rows.try_into().unwrap()),
                ),
                dims,
            );
            context.invalidate();
        }
        context.set_cursor(Some(MouseCursor::Arrow));
    }

    pub fn mouse_event_below_scroll_thumb(
        &mut self,
        _item: UIItem,
        pane: Rc<dyn Pane>,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        if let WMEK::Press(MousePress::Left) = event.kind {
            let dims = pane.get_dimensions();
            let current_viewport = self.get_viewport(pane.pane_id());
            // Page down
            self.set_viewport(
                pane.pane_id(),
                Some(
                    current_viewport
                        .unwrap_or(dims.physical_top)
                        .saturating_add(self.terminal_size.rows.try_into().unwrap()),
                ),
                dims,
            );
            context.invalidate();
        }
        context.set_cursor(Some(MouseCursor::Arrow));
    }

    pub fn mouse_event_scroll_thumb(
        &mut self,
        item: UIItem,
        _pane: Rc<dyn Pane>,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        if let WMEK::Press(MousePress::Left) = event.kind {
            // Start a scroll drag
            // self.scroll_drag_start = Some(from_top);
            self.dragging = Some((item, event));
        }
        context.set_cursor(Some(MouseCursor::Arrow));
    }

    pub fn mouse_event_split(
        &mut self,
        item: UIItem,
        split: PositionedSplit,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        context.set_cursor(Some(match &split.direction {
            SplitDirection::Horizontal => MouseCursor::SizeLeftRight,
            SplitDirection::Vertical => MouseCursor::SizeUpDown,
        }));

        if event.kind == WMEK::Press(MousePress::Left) {
            self.dragging.replace((item, event));
        }
    }

    pub fn mouse_event_terminal(
        &mut self,
        mut pane: Rc<dyn Pane>,
        mut x: usize,
        mut y: i64,
        event: MouseEvent,
        context: &dyn WindowOps,
    ) {
        let mut is_click_to_focus = false;

        for pos in self.get_panes_to_render() {
            if y >= pos.top as i64
                && y <= (pos.top + pos.height) as i64
                && x >= pos.left
                && x <= pos.left + pos.width
            {
                if pane.pane_id() != pos.pane.pane_id() {
                    // We're over a pane that isn't active
                    match &event.kind {
                        WMEK::Press(_) => {
                            let mux = Mux::get().unwrap();
                            mux.get_active_tab_for_window(self.mux_window_id)
                                .map(|tab| tab.set_active_idx(pos.index));

                            pane = Rc::clone(&pos.pane);
                            is_click_to_focus = true;
                        }
                        WMEK::Move => {
                            if self.config.pane_focus_follows_mouse {
                                let mux = Mux::get().unwrap();
                                mux.get_active_tab_for_window(self.mux_window_id)
                                    .map(|tab| tab.set_active_idx(pos.index));

                                pane = Rc::clone(&pos.pane);
                                context.invalidate();
                            }
                        }
                        WMEK::Release(_) => {}
                        WMEK::VertWheel(_) => {}
                        WMEK::HorzWheel(_) => {}
                    }
                }
                x = x.saturating_sub(pos.left);
                y = y.saturating_sub(pos.top as i64);
                break;
            }
        }
        if let Some(focused) = self.focused.as_ref() {
            if focused.elapsed() <= Duration::from_millis(200) {
                if is_click_to_focus {
                    context.invalidate();
                }
                return;
            }
        }

        let dims = pane.get_dimensions();
        let stable_row = self
            .get_viewport(pane.pane_id())
            .unwrap_or(dims.physical_top)
            + y as StableRowIndex;

        self.last_mouse_terminal_coords = (x, stable_row); // FIXME: per-pane

        let (top, mut lines) = pane.get_lines_with_hyperlinks_applied(
            stable_row..stable_row + 1,
            &self.config.hyperlink_rules,
        );
        let new_highlight = if top == stable_row {
            if let Some(line) = lines.get_mut(0) {
                if let Some(cell) = line.cells().get(x) {
                    cell.attrs().hyperlink().cloned()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        match (self.current_highlight.as_ref(), new_highlight) {
            (Some(old_link), Some(new_link)) if Arc::ptr_eq(&old_link, &new_link) => {
                // Unchanged
            }
            (Some(old_link), Some(new_link)) if *old_link == new_link => {
                // Unchanged
                // Note: ideally this case wouldn't exist, as we *should*
                // only be matching distinct instances of the hyperlink.
                // However, for horrible reasons, we always end up with duplicated
                // hyperlink instances today, so we have to do a deeper compare.
            }
            (None, None) => {
                // Unchanged
            }
            (_, rhs) => {
                // We're hovering over a different URL, so invalidate and repaint
                // so that we render the underline correctly
                self.current_highlight = rhs;
                context.invalidate();
            }
        };

        context.set_cursor(Some(if self.current_highlight.is_some() {
            // When hovering over a hyperlink, show an appropriate
            // mouse cursor to give the cue that it is clickable
            MouseCursor::Hand
        } else if pane.is_mouse_grabbed() {
            MouseCursor::Arrow
        } else {
            MouseCursor::Text
        }));

        let event_trigger_type = match &event.kind {
            WMEK::Press(press) => {
                let press = mouse_press_to_tmb(press);
                match self.last_mouse_click.as_ref() {
                    Some(LastMouseClick { streak, button, .. }) if *button == press => {
                        Some(MouseEventTrigger::Down {
                            streak: *streak,
                            button: press,
                        })
                    }
                    _ => None,
                }
            }
            WMEK::Release(press) => {
                let press = mouse_press_to_tmb(press);
                match self.last_mouse_click.as_ref() {
                    Some(LastMouseClick { streak, button, .. }) if *button == press => {
                        Some(MouseEventTrigger::Up {
                            streak: *streak,
                            button: press,
                        })
                    }
                    _ => None,
                }
            }
            WMEK::Move => {
                if !self.current_mouse_buttons.is_empty() {
                    if let Some(LastMouseClick { streak, button, .. }) =
                        self.last_mouse_click.as_ref()
                    {
                        if Some(*button)
                            == self.current_mouse_buttons.last().map(mouse_press_to_tmb)
                        {
                            Some(MouseEventTrigger::Drag {
                                streak: *streak,
                                button: *button,
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            WMEK::VertWheel(_) | WMEK::HorzWheel(_) => None,
        };

        if !pane.is_mouse_grabbed()
            || event
                .modifiers
                .contains(self.config.bypass_mouse_reporting_modifiers)
        {
            if let Some(event_trigger_type) = event_trigger_type {
                let mut modifiers = event.modifiers;

                // Since we use shift to force assessing the mouse bindings, pretend
                // that shift is not one of the mods when the mouse is grabbed.
                if pane.is_mouse_grabbed() {
                    if modifiers.contains(self.config.bypass_mouse_reporting_modifiers) {
                        modifiers.remove(self.config.bypass_mouse_reporting_modifiers);
                    }
                }

                if let Some(action) = self
                    .input_map
                    .lookup_mouse(event_trigger_type.clone(), modifiers)
                {
                    self.perform_key_assignment(&pane, &action).ok();
                    return;
                }
            }
        }

        let mouse_event = wezterm_term::MouseEvent {
            kind: match event.kind {
                WMEK::Move => TMEK::Move,
                WMEK::VertWheel(_) | WMEK::HorzWheel(_) | WMEK::Press(_) => TMEK::Press,
                WMEK::Release(_) => TMEK::Release,
            },
            button: match event.kind {
                WMEK::Release(ref press) | WMEK::Press(ref press) => mouse_press_to_tmb(press),
                WMEK::Move => {
                    if event.mouse_buttons == WMB::LEFT {
                        TMB::Left
                    } else if event.mouse_buttons == WMB::RIGHT {
                        TMB::Right
                    } else if event.mouse_buttons == WMB::MIDDLE {
                        TMB::Middle
                    } else {
                        TMB::None
                    }
                }
                WMEK::VertWheel(amount) => {
                    if amount > 0 {
                        TMB::WheelUp(amount as usize)
                    } else {
                        TMB::WheelDown((-amount) as usize)
                    }
                }
                WMEK::HorzWheel(_) => TMB::None,
            },
            x,
            y,
            modifiers: window_mods_to_termwiz_mods(event.modifiers),
        };

        if !(self.config.swallow_mouse_click_on_pane_focus && is_click_to_focus) {
            pane.mouse_event(mouse_event).ok();
        }

        match event.kind {
            WMEK::Move => {}
            _ => {
                context.invalidate();
            }
        }
    }
}

fn mouse_press_to_tmb(press: &MousePress) -> TMB {
    match press {
        MousePress::Left => TMB::Left,
        MousePress::Right => TMB::Right,
        MousePress::Middle => TMB::Middle,
    }
}
