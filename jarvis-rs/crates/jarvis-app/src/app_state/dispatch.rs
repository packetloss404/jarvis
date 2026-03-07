//! Action dispatch: routes resolved actions to the appropriate subsystem.

use jarvis_common::actions::{Action, ResizeDirection};
use jarvis_common::events::Event;
use jarvis_common::notifications::Notification;
use jarvis_platform::input_processor::InputMode;
use jarvis_renderer::AssistantPanel;
use jarvis_tiling::commands::TilingCommand;
use jarvis_tiling::tree::Direction;

use super::core::JarvisApp;

impl JarvisApp {
    /// Dispatch a resolved [`Action`] to the appropriate subsystem.
    pub(super) fn dispatch(&mut self, action: Action) {
        match action {
            Action::NewPane => {
                let max = self.config.layout.max_panels as usize;
                if self.tiling.pane_count() >= max {
                    tracing::warn!(max, "NewPane rejected: at panel limit");
                    return;
                }
                let viewport = self.viewport();
                let content = self
                    .chrome
                    .content_rect(viewport.width as f32, viewport.height as f32);
                let dir = self.tiling.auto_split_direction(content);
                self.tiling.split(dir);
                let new_id = self.tiling.focused_id();
                self.create_webview_for_pane(new_id);
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::ClosePane => {
                let closing_id = self.tiling.focused_id();
                if self.tiling.close_focused() {
                    self.destroy_webview_for_pane(closing_id);
                    self.sync_webview_bounds();
                    self.needs_redraw = true;
                }
            }
            Action::SplitHorizontal => {
                self.tiling.execute(TilingCommand::SplitHorizontal);
                let new_id = self.tiling.focused_id();
                self.create_webview_for_pane(new_id);
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::SplitVertical => {
                self.tiling.execute(TilingCommand::SplitVertical);
                let new_id = self.tiling.focused_id();
                self.create_webview_for_pane(new_id);
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::FocusPane(n) => {
                self.tiling.focus_pane(n);
                self.notify_focus_changed();
                self.needs_redraw = true;
            }
            Action::FocusNextPane => {
                self.tiling.execute(TilingCommand::FocusNext);
                self.notify_focus_changed();
                self.needs_redraw = true;
            }
            Action::FocusPrevPane => {
                self.tiling.execute(TilingCommand::FocusPrev);
                self.notify_focus_changed();
                self.needs_redraw = true;
            }

            Action::ZoomPane => {
                self.tiling.execute(TilingCommand::Zoom);
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::ResizePane { direction, delta } => {
                let tiling_dir = match direction {
                    ResizeDirection::Left | ResizeDirection::Right => Direction::Horizontal,
                    ResizeDirection::Up | ResizeDirection::Down => Direction::Vertical,
                };
                let signed_delta = match direction {
                    ResizeDirection::Right | ResizeDirection::Down => delta,
                    ResizeDirection::Left | ResizeDirection::Up => -delta,
                };
                self.tiling
                    .execute(TilingCommand::Resize(tiling_dir, signed_delta));
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::SwapPane(direction) => {
                let tiling_dir = match direction {
                    ResizeDirection::Left | ResizeDirection::Right => Direction::Horizontal,
                    ResizeDirection::Up | ResizeDirection::Down => Direction::Vertical,
                };
                self.tiling.execute(TilingCommand::Swap(tiling_dir));
                self.sync_webview_bounds();
                self.needs_redraw = true;
            }
            Action::ToggleFullscreen => {
                if let Some(ref w) = self.window {
                    if w.fullscreen().is_some() {
                        w.set_fullscreen(None);
                    } else {
                        w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                    }
                }
            }
            Action::ToggleBlankPane => {
                self.toggle_blank_for_focused_pane();
            }
            Action::OpenCommandPalette => {
                self.command_palette_open = true;
                let mut palette = jarvis_renderer::CommandPalette::new(&self.registry);
                self.inject_plugin_items(&mut palette);
                self.command_palette = Some(palette);
                self.input.set_mode(InputMode::CommandPalette);
                self.send_palette_to_webview("palette_show");
                self.notify_overlay_state();
                self.needs_redraw = true;
            }
            Action::OpenAssistant => {
                if self.assistant_open {
                    self.assistant_open = false;
                    self.assistant_panel = None;
                    self.input.set_mode(InputMode::Terminal);
                } else {
                    self.assistant_open = true;
                    self.assistant_panel = Some(AssistantPanel::new());
                    self.input.set_mode(InputMode::Assistant);
                    self.ensure_assistant_runtime();
                }
                self.notify_overlay_state();
                self.needs_redraw = true;
            }
            Action::CloseOverlay => {
                if self.assistant_open {
                    self.assistant_open = false;
                    self.assistant_panel = None;
                } else {
                    self.send_palette_hide();
                    self.command_palette_open = false;
                    self.command_palette = None;
                }
                self.input.set_mode(InputMode::Terminal);
                self.notify_overlay_state();
            }
            Action::OpenSettings => {
                self.input.set_mode(InputMode::Settings);
                // Open a settings webview panel
                let kind = jarvis_common::types::PaneKind::WebView;
                if let Some(new_id) = self.tiling.split_with(
                    jarvis_tiling::tree::Direction::Horizontal,
                    kind,
                    "Settings",
                ) {
                    self.create_webview_for_pane_with_url(
                        new_id,
                        "jarvis://localhost/settings/index.html",
                    );
                    self.sync_webview_bounds();
                    self.needs_redraw = true;
                }
            }
            Action::OpenChat => {
                let kind = jarvis_common::types::PaneKind::Chat;
                if let Some(new_id) =
                    self.tiling
                        .split_with(jarvis_tiling::tree::Direction::Horizontal, kind, "Chat")
                {
                    self.create_webview_for_pane_with_url(
                        new_id,
                        "jarvis://localhost/chat/index.html",
                    );
                    self.sync_webview_bounds();
                    self.needs_redraw = true;
                }
            }
            Action::Copy => {
                // Ask the focused webview to grab its selection and send it
                // back via clipboard_copy IPC (handled in ipc_dispatch.rs).
                let focused = self.tiling.focused_id();
                if let Some(ref registry) = self.webviews {
                    if let Some(handle) = registry.get(focused) {
                        let _ = handle.evaluate_script(
                            "(function(){var t='';if(window._xtermInstance&&window._xtermInstance.getSelection){t=window._xtermInstance.getSelection();}if(!t){t=(window.getSelection()||'').toString();}if(t&&window.jarvis&&window.jarvis.ipc){window.jarvis.ipc.send('clipboard_copy',{text:t});}})()"
                        );
                    }
                }
            }
            Action::Paste => {
                // Read clipboard on the Rust side (no WebView permission needed)
                let text = match jarvis_platform::Clipboard::new() {
                    Ok(mut clip) => clip.get_text().ok(),
                    Err(_) => None,
                };
                if let Some(text) = text {
                    if !text.is_empty() {
                        let focused = self.tiling.focused_id();
                        if let Some(ref registry) = self.webviews {
                            if let Some(handle) = registry.get(focused) {
                                // Escape text for JS string literal
                                let escaped = text
                                    .replace('\\', "\\\\")
                                    .replace('\'', "\\'")
                                    .replace('\n', "\\n")
                                    .replace('\r', "\\r");
                                let js = format!(
                                    concat!(
                                        "(function(){{",
                                        "var t='{}';",
                                        "var a=document.activeElement;",
                                        "if(a&&(a.tagName==='INPUT'||a.tagName==='TEXTAREA')){{",
                                        "var s=a.selectionStart||0,e=a.selectionEnd||0;",
                                        "a.value=a.value.slice(0,s)+t+a.value.slice(e);",
                                        "a.selectionStart=a.selectionEnd=s+t.length;",
                                        "a.dispatchEvent(new Event('input',{{bubbles:true}}));",
                                        "}}else if(window.jarvis&&window.jarvis.ipc){{",
                                        "window.jarvis.ipc.send('pty_input',{{data:t}});",
                                        "}}",
                                        "}})()"
                                    ),
                                    escaped
                                );
                                let _ = handle.evaluate_script(&js);
                            }
                        }
                    }
                }
            }
            Action::LaunchGame(ref game) => {
                let pane_id = self.tiling.focused_id();
                let game_url = format!("jarvis://localhost/games/{}.html", game);

                // Save original URL before navigating
                if let Some(ref registry) = self.webviews {
                    if let Some(handle) = registry.get(pane_id) {
                        let original_url = handle.current_url().to_string();
                        self.game_active.insert(pane_id, original_url);
                    }
                }

                if game == "emulator" {
                    // Emulator uses WebGL which requires a non-transparent WebView.
                    // Destroy the existing transparent one and recreate as opaque.
                    if let Some(ref mut registry) = self.webviews {
                        registry.destroy(pane_id);
                    }
                    self.create_webview_for_pane_opaque(pane_id, &game_url);
                    tracing::info!(pane_id, game = %game, "Emulator launched (opaque WebView)");
                } else {
                    if let Some(ref mut registry) = self.webviews {
                        if let Some(handle) = registry.get_mut(pane_id) {
                            if let Err(e) = handle.load_url(&game_url) {
                                tracing::warn!(error = %e, "Failed to launch game");
                                self.game_active.remove(&pane_id);
                            } else {
                                tracing::info!(pane_id, game = %game, "Game launched");
                            }
                        }
                    }
                }
            }
            Action::OpenURL(ref url) => {
                // Normalize: auto-prepend https:// if no scheme is provided
                let normalized = if !url.contains("://") {
                    format!("https://{}", url)
                } else {
                    url.clone()
                };
                let pane_id = self.tiling.focused_id();
                if let Some(ref mut registry) = self.webviews {
                    if let Some(handle) = registry.get_mut(pane_id) {
                        let original_url = handle.current_url().to_string();
                        if let Err(e) = handle.load_url(&normalized) {
                            tracing::warn!(error = %e, url = %normalized, "Failed to open URL");
                        } else {
                            tracing::info!(pane_id, url = %normalized, "URL navigation requested");
                            self.game_active.insert(pane_id, original_url);
                        }
                    }
                }
            }
            Action::PairMobile => {
                self.show_pair_code();
            }
            Action::RevokeMobilePairing => {
                self.revoke_mobile_pairing();
                tracing::info!("Mobile pairing revoked — new session ID generated");
            }
            Action::ReloadConfig => match jarvis_config::load_config() {
                Ok(c) => {
                    self.registry =
                        jarvis_platform::input::KeybindRegistry::from_config(&c.keybinds);
                    self.chrome = jarvis_renderer::UiChrome::from_config(&c.layout);

                    // Re-register plugin directories
                    if let Some(ref dirs_handle) = self.plugin_dirs {
                        if let Ok(mut dirs) = dirs_handle.write() {
                            dirs.clear();
                            if let Some(plugins_base) =
                                jarvis_config::toml_loader::plugins::plugins_dir()
                            {
                                for lp in &c.plugins.local {
                                    dirs.insert(lp.id.clone(), plugins_base.join(&lp.id));
                                }
                            }
                        }
                    }

                    self.config = c;
                    self.inject_theme_into_all_webviews();
                    self.event_bus.publish(Event::ConfigReloaded);
                    tracing::info!("Config reloaded");
                }
                Err(e) => {
                    tracing::warn!("Config reload failed: {e}");
                    self.notifications.push(Notification::error(
                        "Config Error",
                        format!("Reload failed: {e}"),
                    ));
                }
            },
            Action::ClearTerminal => {
                let focused = self.tiling.focused_id();
                if let Some(ref registry) = self.webviews {
                    if let Some(handle) = registry.get(focused) {
                        let _ = handle.evaluate_script(
                            "(function(){if(window._xtermInstance&&window._xtermInstance.clear){window._xtermInstance.clear();}})()"
                        );
                    }
                }
            }
            Action::ScrollUp(_) | Action::ScrollDown(_) => {
                tracing::debug!("scroll action: will be handled by webview");
            }
            Action::Quit => {
                self.event_bus.publish(Event::Shutdown);
                self.shutdown();
                self.should_exit = true;
            }
            _ => {
                tracing::debug!("unhandled action: {:?}", action);
            }
        }

        self.update_window_title();
    }
}
