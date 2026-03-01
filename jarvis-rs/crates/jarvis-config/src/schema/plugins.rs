use serde::{Deserialize, Serialize};

/// A bookmark plugin: a named URL that appears in the command palette.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BookmarkPlugin {
    /// Display name in the palette.
    pub name: String,
    /// URL to open (e.g. "https://open.spotify.com").
    pub url: String,
    /// Palette category grouping.
    pub category: String,
}

impl Default for BookmarkPlugin {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            category: "Plugins".into(),
        }
    }
}

/// A local plugin discovered from the filesystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPlugin {
    /// Folder name (used as the plugin ID in URLs).
    pub id: String,
    /// Display name in the palette.
    pub name: String,
    /// Palette category grouping.
    pub category: String,
    /// Entry HTML file (default "index.html").
    pub entry: String,
}

/// Plugin configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginsConfig {
    /// Bookmark plugins loaded from TOML config.
    pub bookmarks: Vec<BookmarkPlugin>,
    /// Local plugins discovered from the filesystem (not serialized).
    #[serde(skip)]
    pub local: Vec<LocalPlugin>,
}
