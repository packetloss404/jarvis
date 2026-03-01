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
            Action::LaunchGame(ref name) => match name.as_str() {
                "tetris" => "Play Tetris",
                "asteroids" => "Play Asteroids",
                "minesweeper" => "Play Minesweeper",
                "pinball" => "Play Pinball",
                "doodlejump" => "Play Doodle Jump",
                "draw" => "Open Draw",
                "subway" => "Play Subway Surfers",
                "videoplayer" => "Open Video Player",
                _ => "Launch Game",
            },
            Action::OpenURL(ref url) => {
                if url.contains("kartbros") {
                    "Play KartBros"
                } else if url.contains("basketbros") {
                    "Play Basket Bros"
                } else if url.contains("footballbros") {
                    "Play Football Bros"
                } else if url.contains("soccerbros") {
                    "Play Soccer Bros"
                } else if url.contains("wrestlebros") {
                    "Play Wrestle Bros"
                } else if url.contains("baseballbros") {
                    "Play Baseball Bros"
                } else if url.contains("lichess") {
                    "Play Lichess"
                } else if url.contains("monkeytype") {
                    "Open Monkeytype"
                } else if url.contains("excalidraw") {
                    "Open Excalidraw"
                } else if url.contains("desmos") {
                    "Open Desmos"
                } else if url.contains("news.ycombinator") {
                    "Open Hacker News"
                } else if url.contains("spotify") {
                    "Open Spotify"
                } else {
                    "Open Website"
                }
            }
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

            Action::ToggleFullscreen | Action::Quit => "Window",

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

            Action::LaunchGame(_) => "Games",

            Action::OpenURL(ref url) => {
                if url.contains("kartbros")
                    || url.contains("basketbros")
                    || url.contains("footballbros")
                    || url.contains("soccerbros")
                    || url.contains("wrestlebros")
                    || url.contains("baseballbros")
                    || url.contains("lichess")
                {
                    "Games"
                } else {
                    "Web"
                }
            }

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
            Action::OpenSettings,
            Action::OpenChat,
            Action::OpenURLPrompt,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
            Action::ScrollToTop,
            Action::ScrollToBottom,
            Action::ClearTerminal,
            Action::LaunchGame("tetris".into()),
            Action::LaunchGame("asteroids".into()),
            Action::LaunchGame("minesweeper".into()),
            Action::LaunchGame("pinball".into()),
            Action::LaunchGame("doodlejump".into()),
            Action::LaunchGame("draw".into()),
            Action::LaunchGame("subway".into()),
            Action::OpenURL("https://kartbros.io".into()),
            Action::OpenURL("https://basketbros.io".into()),
            Action::OpenURL("https://footballbros.io".into()),
            Action::OpenURL("https://soccerbros.gg".into()),
            Action::OpenURL("https://wrestlebros.io".into()),
            Action::OpenURL("https://baseballbros.io".into()),
            Action::OpenURL("https://lichess.org".into()),
            Action::OpenURL("https://monkeytype.com".into()),
            Action::OpenURL("https://excalidraw.com".into()),
            Action::OpenURL("https://www.desmos.com/calculator".into()),
            Action::OpenURL("https://news.ycombinator.com".into()),
            Action::OpenURL("https://open.spotify.com".into()),
            Action::PairMobile,
            Action::RevokeMobilePairing,
            Action::ReloadConfig,
            Action::Quit,
        ]
    }
}
