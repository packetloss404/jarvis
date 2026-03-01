//! Native menu bar setup and event handling.

use jarvis_common::actions::Action;
use muda::accelerator::{Accelerator, Code, Modifiers, CMD_OR_CTRL};
use muda::{Menu, MenuEvent, MenuItem, MenuId, PredefinedMenuItem, Submenu};

use super::core::JarvisApp;

/// Stored menu item IDs so we can map click events back to [`Action`] variants.
pub(super) struct MenuIds {
    settings: MenuId,
    reload_config: MenuId,
    command_palette: MenuId,
    toggle_fullscreen: MenuId,
    open_chat: MenuId,
    new_pane: MenuId,
    close_pane: MenuId,
    split_horizontal: MenuId,
    split_vertical: MenuId,
}

impl JarvisApp {
    /// Build the native menu bar and attach it to the application.
    pub(super) fn initialize_menu(&mut self) {
        let menu = Menu::new();

        // -- App menu (jarvis) --
        let about = PredefinedMenuItem::about(None, None);
        let settings = MenuItem::new(
            "Settings",
            true,
            Some(Accelerator::new(Some(CMD_OR_CTRL), Code::Comma)),
        );
        let reload_config = MenuItem::new("Reload Config", true, None);
        let quit = PredefinedMenuItem::quit(None);

        let app_menu = Submenu::with_items(
            "jarvis",
            true,
            &[
                &about,
                &PredefinedMenuItem::separator(),
                &settings,
                &reload_config,
                &PredefinedMenuItem::separator(),
                &quit,
            ],
        )
        .expect("failed to build app menu");

        // -- Edit menu --
        let edit_menu = Submenu::with_items(
            "Edit",
            true,
            &[
                &PredefinedMenuItem::copy(None),
                &PredefinedMenuItem::paste(None),
                &PredefinedMenuItem::select_all(None),
            ],
        )
        .expect("failed to build edit menu");

        // -- View menu --
        let command_palette = MenuItem::new(
            "Command Palette",
            true,
            Some(Accelerator::new(
                Some(CMD_OR_CTRL | Modifiers::SHIFT),
                Code::KeyP,
            )),
        );
        let toggle_fullscreen = MenuItem::new("Toggle Fullscreen", true, None);
        let open_chat = MenuItem::new("Open Chat", true, None);

        let view_menu = Submenu::with_items(
            "View",
            true,
            &[
                &command_palette,
                &toggle_fullscreen,
                &PredefinedMenuItem::separator(),
                &open_chat,
            ],
        )
        .expect("failed to build view menu");

        // -- Window menu --
        let new_pane = MenuItem::new("New Pane", true, None);
        let close_pane = MenuItem::new("Close Pane", true, None);
        let split_horizontal = MenuItem::new("Split Horizontal", true, None);
        let split_vertical = MenuItem::new("Split Vertical", true, None);

        let window_menu = Submenu::with_items(
            "Window",
            true,
            &[
                &new_pane,
                &close_pane,
                &PredefinedMenuItem::separator(),
                &split_horizontal,
                &split_vertical,
            ],
        )
        .expect("failed to build window menu");

        // Assemble the menu bar
        menu.append_items(&[&app_menu, &edit_menu, &view_menu, &window_menu])
            .expect("failed to assemble menu bar");

        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();

        // Store IDs for event mapping
        self.menu_ids = Some(MenuIds {
            settings: settings.into_id(),
            reload_config: reload_config.into_id(),
            command_palette: command_palette.into_id(),
            toggle_fullscreen: toggle_fullscreen.into_id(),
            open_chat: open_chat.into_id(),
            new_pane: new_pane.into_id(),
            close_pane: close_pane.into_id(),
            split_horizontal: split_horizontal.into_id(),
            split_vertical: split_vertical.into_id(),
        });

        // Keep the menu alive — dropping it removes it from the menu bar.
        self._menu = Some(menu);

        tracing::info!("Native menu bar initialized");
    }

    /// Poll for native menu click events and dispatch the corresponding action.
    pub(super) fn poll_menu_events(&mut self) {
        // Collect actions first to avoid borrow conflict with self.dispatch().
        let actions: Vec<Action> = {
            let ids = match self.menu_ids {
                Some(ref ids) => ids,
                None => return,
            };

            let mut actions = Vec::new();
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                let action = if event.id == ids.settings {
                    Action::OpenSettings
                } else if event.id == ids.reload_config {
                    Action::ReloadConfig
                } else if event.id == ids.command_palette {
                    Action::OpenCommandPalette
                } else if event.id == ids.toggle_fullscreen {
                    Action::ToggleFullscreen
                } else if event.id == ids.open_chat {
                    Action::OpenChat
                } else if event.id == ids.new_pane {
                    Action::NewPane
                } else if event.id == ids.close_pane {
                    Action::ClosePane
                } else if event.id == ids.split_horizontal {
                    Action::SplitHorizontal
                } else if event.id == ids.split_vertical {
                    Action::SplitVertical
                } else {
                    continue;
                };
                actions.push(action);
            }
            actions
        };

        for action in actions {
            tracing::debug!(action = ?action, "Menu item clicked");
            self.dispatch(action);
        }
    }
}
