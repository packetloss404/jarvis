use super::Action;

impl Action {
    /// Human-readable label for display in the command palette.
    pub fn label(&self) -> &'static str {
        match self {
            Action::NewPane => "New Pane",
            Action::ClosePane => "Close Pane",
            Action::SplitHorizontal => "Split Horizontal",
            Action::SplitVertical => "Split Vertical",
            Action::FocusPane(1) => "Focus Pane 1",
            Action::FocusPane(2) => "Focus Pane 2",
            Action::FocusPane(3) => "Focus Pane 3",
            Action::FocusPane(4) => "Focus Pane 4",
            Action::FocusPane(5) => "Focus Pane 5",
            Action::FocusPane(_) => "Focus Pane",
            Action::FocusNextPane => "Focus Next Pane",
            Action::FocusPrevPane => "Focus Previous Pane",
            Action::ZoomPane => "Zoom Pane",
            Action::ResizePane { .. } => "Resize Pane",
            Action::SwapPane(_) => "Swap Pane",
            Action::ToggleFullscreen => "Toggle Fullscreen",
            Action::ToggleBlankPane => "Toggle Blank Window",
            Action::Quit => "Quit",
            Action::OpenCommandPalette => "Command Palette",
            Action::OpenSettings => "Open Settings",
            Action::CloseOverlay => "Close Overlay",
            Action::OpenURLPrompt => "Open URL",
            Action::OpenAssistant => "Open Assistant",
            Action::OpenChat => "Open Chat",
            Action::PushToTalk => "Push to Talk",
            Action::ReleasePushToTalk => "Release Push to Talk",
            Action::ScrollUp(_) => "Scroll Up",
            Action::ScrollDown(_) => "Scroll Down",
            Action::ScrollToTop => "Scroll to Top",
            Action::ScrollToBottom => "Scroll to Bottom",
            Action::Copy => "Copy",
            Action::Paste => "Paste",
            Action::SelectAll => "Select All",
            Action::SearchOpen => "Find",
            Action::SearchClose => "Close Find",
            Action::SearchNext => "Find Next",
            Action::SearchPrev => "Find Previous",
            Action::ClearTerminal => "Clear Terminal",
            // Bookmarks carry their own label via config; this is the generic fallback.
            Action::OpenURL(_) => "Open Website",
            Action::PairMobile => "Pair Mobile Device",
            Action::RevokeMobilePairing => "Revoke Mobile Pairing",
            Action::ReloadConfig => "Reload Config",
            Action::None => "None",
        }
    }

    /// Category for grouping in the command palette.
    pub fn category(&self) -> &'static str {
        match self {
            Action::NewPane
            | Action::ClosePane
            | Action::SplitHorizontal
            | Action::SplitVertical
            | Action::FocusPane(_)
            | Action::FocusNextPane
            | Action::FocusPrevPane
            | Action::ZoomPane
            | Action::SwapPane(_)
            | Action::ResizePane { .. } => "Panes",

            Action::ToggleFullscreen | Action::ToggleBlankPane | Action::Quit => "Window",

            Action::OpenSettings
            | Action::OpenAssistant
            | Action::OpenChat
            | Action::OpenURLPrompt
            | Action::OpenCommandPalette
            | Action::CloseOverlay => "Apps",

            Action::Copy
            | Action::Paste
            | Action::SelectAll
            | Action::SearchOpen
            | Action::SearchClose
            | Action::SearchNext
            | Action::SearchPrev
            | Action::ScrollUp(_)
            | Action::ScrollDown(_)
            | Action::ScrollToTop
            | Action::ScrollToBottom
            | Action::ClearTerminal => "Terminal",

            // Bookmarks carry their own category via config; this is the fallback.
            Action::OpenURL(_) => "Web",

            Action::PairMobile | Action::RevokeMobilePairing | Action::ReloadConfig => "System",

            Action::PushToTalk | Action::ReleasePushToTalk | Action::None => "System",
        }
    }

    /// All actions that should appear in the command palette.
    pub fn palette_actions() -> Vec<Action> {
        vec![
            Action::NewPane,
            Action::ClosePane,
            Action::SplitHorizontal,
            Action::SplitVertical,
            Action::ToggleFullscreen,
            Action::ToggleBlankPane,
            Action::OpenSettings,
            Action::OpenChat,
            Action::OpenURLPrompt,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
            Action::ScrollToTop,
            Action::ScrollToBottom,
            Action::ClearTerminal,
            Action::PairMobile,
            Action::RevokeMobilePairing,
            Action::ReloadConfig,
            Action::Quit,
        ]
    }
}
