//! Focus and zoom handling for TilingManager.

use crate::tree::Direction;

use super::TilingManager;

/// Maximum number of entries kept in the focus history (ISS-44).
const FOCUS_HISTORY_LIMIT: usize = 64;

impl TilingManager {
    /// Push the current focused pane onto the history stack, capping at
    /// `FOCUS_HISTORY_LIMIT` entries (ISS-44).
    fn push_focus_history(&mut self, prev_id: u32) {
        self.focus_history.push(prev_id);
        if self.focus_history.len() > FOCUS_HISTORY_LIMIT {
            self.focus_history.remove(0);
        }
    }

    /// Focus the next pane in order.
    pub fn focus_next(&mut self) -> bool {
        if let Some(next) = self.tree.next_pane(self.focused) {
            let prev = self.focused;
            self.focused = next;
            self.push_focus_history(prev);
            true
        } else {
            false
        }
    }

    /// Focus the previous pane in order.
    pub fn focus_prev(&mut self) -> bool {
        if let Some(prev) = self.tree.prev_pane(self.focused) {
            let old = self.focused;
            self.focused = prev;
            self.push_focus_history(old);
            true
        } else {
            false
        }
    }

    /// Focus the neighbor in a specific direction.
    pub fn focus_direction(&mut self, direction: Direction) -> bool {
        if let Some(neighbor) = self.tree.find_neighbor(self.focused, direction) {
            let prev = self.focused;
            self.focused = neighbor;
            self.push_focus_history(prev);
            true
        } else {
            false
        }
    }

    /// Set focus to a specific pane by ID.
    pub fn focus_pane(&mut self, id: u32) -> bool {
        if self.panes.contains_key(&id) {
            let prev = self.focused;
            self.focused = id;
            self.push_focus_history(prev);
            true
        } else {
            false
        }
    }

    /// Toggle zoom on the focused pane.
    pub fn zoom_toggle(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        if self.zoomed == Some(self.focused) {
            self.zoomed = None;
        } else {
            self.zoomed = Some(self.focused);
        }
        true
    }
}
