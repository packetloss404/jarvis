//! Command palette key handling and webview IPC bridge.

use jarvis_common::actions::Action;
use jarvis_platform::input::KeybindRegistry;
use jarvis_platform::input_processor::InputMode;
use jarvis_renderer::PaletteMode;

use super::core::JarvisApp;

impl JarvisApp {
    /// Handle key events for the command palette.
    pub(super) fn handle_palette_key(&mut self, key_name: &str, is_press: bool) -> bool {
        if !is_press || !self.command_palette_open {
            return false;
        }

        let palette = match self.command_palette.as_mut() {
            Some(p) => p,
            None => return false,
        };

        let mode = palette.mode();

        match key_name {
            "Escape" => {
                if mode == PaletteMode::UrlInput {
                    // Go back to action select mode
                    let registry = KeybindRegistry::from_config(&self.config.keybinds);
                    let mut new_palette = jarvis_renderer::CommandPalette::new(&registry);
                    self.inject_plugin_items(&mut new_palette);
                    self.command_palette = Some(new_palette);
                    self.send_palette_update();
                } else {
                    self.dispatch(Action::CloseOverlay);
                }
                true
            }
            "Enter" => {
                if let Some(action) = palette.confirm() {
                    if action == Action::OpenURLPrompt {
                        palette.enter_url_mode();
                        self.send_palette_update();
                    } else {
                        self.send_palette_hide();
                        self.command_palette_open = false;
                        self.command_palette = None;
                        self.input.set_mode(InputMode::Terminal);
                        self.notify_overlay_state();
                        self.dispatch(action);
                    }
                }
                true
            }
            "Up" => {
                palette.select_prev();
                self.send_palette_update();
                true
            }
            "Down" => {
                palette.select_next();
                self.send_palette_update();
                true
            }
            "Backspace" => {
                palette.backspace();
                self.send_palette_update();
                true
            }
            "Tab" => {
                palette.select_next();
                self.send_palette_update();
                true
            }
            _ => {
                if key_name.len() == 1 {
                    let ch = key_name.chars().next().unwrap();
                    if ch.is_ascii_graphic() || ch == ' ' {
                        let ch = if mode == PaletteMode::UrlInput {
                            ch // preserve case for URLs
                        } else {
                            ch.to_ascii_lowercase()
                        };
                        palette.append_char(ch);
                        self.send_palette_update();
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Send palette state to the focused webview.
    pub(super) fn send_palette_to_webview(&self, kind: &str) {
        let focused = self.tiling.focused_id();
        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(focused) {
                if let Some(ref palette) = self.command_palette {
                    let items: Vec<_> = palette
                        .visible_items()
                        .iter()
                        .map(|item| {
                            serde_json::json!({
                                "label": item.label,
                                "keybind": item.keybind_display,
                                "category": item.category
                            })
                        })
                        .collect();
                    let (mode_str, placeholder) = match palette.mode() {
                        PaletteMode::ActionSelect => ("action_select", ""),
                        PaletteMode::UrlInput => ("url_input", "Type a URL and press Enter"),
                    };
                    let payload = serde_json::json!({
                        "items": items,
                        "query": palette.query(),
                        "selectedIndex": palette.selected_index(),
                        "mode": mode_str,
                        "placeholder": placeholder
                    });
                    let _ = handle.send_ipc(kind, &payload);
                }
            }
        }
    }

    /// Send palette_hide to the focused webview.
    pub(super) fn send_palette_hide(&self) {
        let focused = self.tiling.focused_id();
        if let Some(ref registry) = self.webviews {
            if let Some(handle) = registry.get(focused) {
                let _ = handle.send_ipc("palette_hide", &serde_json::json!({}));
            }
        }
    }

    /// Convenience: send palette_update with current state.
    fn send_palette_update(&self) {
        self.send_palette_to_webview("palette_update");
    }

    /// Inject plugin items (bookmarks + local plugins) into the command palette.
    pub(super) fn inject_plugin_items(&self, palette: &mut jarvis_renderer::CommandPalette) {
        let mut items = Vec::new();

        for bm in &self.config.plugins.bookmarks {
            if bm.name.is_empty() || bm.url.is_empty() {
                continue;
            }
            items.push(jarvis_renderer::PaletteItem {
                action: jarvis_common::actions::Action::OpenURL(bm.url.clone()),
                label: bm.name.clone(),
                keybind_display: None,
                category: bm.category.clone(),
            });
        }

        for lp in &self.config.plugins.local {
            let url = format!(
                "jarvis://localhost/plugins/{}/{}",
                lp.id, lp.entry
            );
            items.push(jarvis_renderer::PaletteItem {
                action: jarvis_common::actions::Action::OpenURL(url),
                label: lp.name.clone(),
                keybind_display: None,
                category: lp.category.clone(),
            });
        }

        if !items.is_empty() {
            palette.add_items(items);
        }
    }

    /// Notify all webviews whether an overlay (palette/assistant) is active.
    /// This lets the JS keybind interceptor know to forward Cmd+V etc. to Rust.
    pub(super) fn notify_overlay_state(&self) {
        let active = self.command_palette_open || self.assistant_open;
        if let Some(ref registry) = self.webviews {
            let js = format!(
                "if(window.jarvis&&window.jarvis._setOverlayActive)window.jarvis._setOverlayActive({});",
                if active { "true" } else { "false" }
            );
            for pane_id in registry.active_panes() {
                if let Some(handle) = registry.get(pane_id) {
                    let _ = handle.evaluate_script(&js);
                }
            }
        }
    }
}
