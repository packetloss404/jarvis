use jarvis_common::actions::Action;

/// The current mode of the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Filtering/selecting from the action list.
    ActionSelect,
    /// Typing a URL to navigate to.
    UrlInput,
}

/// A single item in the command palette.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    /// The action this item triggers.
    pub action: Action,
    /// Human-readable label.
    pub label: String,
    /// The keybind display string (e.g. "⌘T"), if one is bound.
    pub keybind_display: Option<String>,
    /// Category for grouping in the palette UI.
    pub category: String,
}
