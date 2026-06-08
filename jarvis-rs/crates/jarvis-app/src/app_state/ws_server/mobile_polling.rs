//! Polling mobile WebSocket commands from the main thread.

use crate::app_state::core::JarvisApp;

use super::relay_client::ClientCommand;

impl JarvisApp {
    /// Process commands from connected mobile clients (non-blocking).
    ///
    /// Called from the main poll loop alongside `poll_pty_output()`.
    pub(in crate::app_state) fn poll_mobile_commands(&mut self) {
        if let Some(ref rx) = self.mobile_cmd_rx {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    ClientCommand::PtyInput { pane_id, data } => {
                        // Block writes to the pair host pane — mobile is not the driver
                        if self.pair_host_pane_id == Some(pane_id) {
                            tracing::warn!(pane_id, "Mobile PtyInput to pair host pane rejected");
                            continue;
                        }
                        if let Err(e) = self.ptys.write_input(pane_id, data.as_bytes()) {
                            tracing::warn!(pane_id, error = %e, "Mobile input write failed");
                        }
                    }
                    ClientCommand::PtyResize {
                        pane_id,
                        cols,
                        rows,
                    } => {
                        if cols == 0 || rows == 0 || cols > 500 || rows > 500 {
                            continue;
                        }
                        if let Err(e) = self.ptys.resize(pane_id, cols, rows) {
                            tracing::warn!(pane_id, error = %e, "Mobile resize failed");
                        }
                    }
                }
            }
        }
    }
}
