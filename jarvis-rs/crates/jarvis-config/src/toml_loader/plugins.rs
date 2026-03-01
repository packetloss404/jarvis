//! Plugin discovery: scan the filesystem for local plugin folders.

use std::path::PathBuf;

use crate::schema::LocalPlugin;

/// Return the platform plugins directory (e.g. `~/.config/jarvis/plugins/`).
pub fn plugins_dir() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join("jarvis").join("plugins"))
}

/// Scan `dir` for subdirectories containing a `plugin.toml` manifest.
///
/// Each valid subfolder produces a [`LocalPlugin`] with the folder name as `id`.
pub fn discover_local_plugins(dir: &PathBuf) -> Vec<LocalPlugin> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut plugins = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("plugin.toml");
        if !manifest_path.is_file() {
            continue;
        }

        let id = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let content = match std::fs::read_to_string(&manifest_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    plugin = %id,
                    error = %e,
                    "Failed to read plugin manifest"
                );
                continue;
            }
        };

        #[derive(serde::Deserialize)]
        #[serde(default)]
        struct Manifest {
            name: String,
            category: String,
            entry: String,
        }

        impl Default for Manifest {
            fn default() -> Self {
                Self {
                    name: String::new(),
                    category: "Plugins".into(),
                    entry: "index.html".into(),
                }
            }
        }

        match toml::from_str::<Manifest>(&content) {
            Ok(m) => {
                let name = if m.name.is_empty() {
                    id.clone()
                } else {
                    m.name
                };
                plugins.push(LocalPlugin {
                    id,
                    name,
                    category: m.category,
                    entry: m.entry,
                });
            }
            Err(e) => {
                tracing::warn!(
                    plugin = %id,
                    error = %e,
                    "Failed to parse plugin manifest"
                );
            }
        }
    }

    plugins
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plugins_dir_returns_some() {
        // Should return a path on any platform with a home directory
        let dir = plugins_dir();
        assert!(dir.is_some());
        let dir = dir.unwrap();
        assert!(dir.ends_with("jarvis/plugins") || dir.ends_with("jarvis\\plugins"));
    }

    #[test]
    fn discover_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = discover_local_plugins(&tmp.path().to_path_buf());
        assert!(plugins.is_empty());
    }

    #[test]
    fn discover_nonexistent_dir() {
        let plugins = discover_local_plugins(&PathBuf::from("/nonexistent/path/plugins"));
        assert!(plugins.is_empty());
    }

    #[test]
    fn discover_valid_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("my-timer");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "My Timer"
category = "Tools"
"#,
        )
        .unwrap();
        fs::write(plugin_dir.join("index.html"), "<html></html>").unwrap();

        let plugins = discover_local_plugins(&tmp.path().to_path_buf());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "my-timer");
        assert_eq!(plugins[0].name, "My Timer");
        assert_eq!(plugins[0].category, "Tools");
        assert_eq!(plugins[0].entry, "index.html");
    }

    #[test]
    fn discover_plugin_with_custom_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("dashboard");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "Dashboard"
entry = "app.html"
"#,
        )
        .unwrap();

        let plugins = discover_local_plugins(&tmp.path().to_path_buf());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].entry, "app.html");
    }

    #[test]
    fn discover_skips_dir_without_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        // Dir without plugin.toml
        fs::create_dir(tmp.path().join("no-manifest")).unwrap();
        // Dir with manifest
        let valid = tmp.path().join("valid");
        fs::create_dir(&valid).unwrap();
        fs::write(valid.join("plugin.toml"), "name = \"Valid\"").unwrap();

        let plugins = discover_local_plugins(&tmp.path().to_path_buf());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Valid");
    }

    #[test]
    fn discover_uses_folder_name_when_no_name() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("cool-plugin");
        fs::create_dir(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.toml"), "category = \"Fun\"").unwrap();

        let plugins = discover_local_plugins(&tmp.path().to_path_buf());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "cool-plugin");
        assert_eq!(plugins[0].category, "Fun");
    }
}
