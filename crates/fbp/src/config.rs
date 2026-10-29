//! The config file: `$XDG_CONFIG_HOME/fbp/config.toml`.
//!
//! Deliberately tiny in P1b -- the resource root, the art mode, the volume and the layout
//! file name. Everything that is *state* (which panel was focused, the filter) lives in the
//! session file next door, so a config can be shared and a session cannot.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_volume() -> f32 {
    1.0
}

fn default_art() -> bool {
    true
}

fn default_layout() -> String {
    "default".to_string()
}

fn default_root() -> String {
    dirs::audio_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .display()
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_root")]
    pub root: String,
    #[serde(default = "default_art")]
    pub art: bool,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default = "default_layout")]
    pub layout: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: default_root(),
            art: true,
            volume: 1.0,
            layout: "default".to_string(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fbp")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fbp")
}

pub fn db_path() -> PathBuf {
    data_dir().join("library.db")
}

pub fn layouts_dir() -> PathBuf {
    config_dir().join("layouts")
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = config_dir().join("config.toml");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Read a layout file by name from the layouts directory.
    pub fn load_layout(&self, name: &str) -> Option<super::tui::layout::Node> {
        let path = layouts_dir().join(format!("{name}.toml"));
        let text = std::fs::read_to_string(path).ok()?;
        let file: super::tui::layout::File = toml::from_str(&text).ok()?;
        Some(file.node)
    }

    pub fn save_layout(
        &self,
        name: &str,
        node: &super::tui::layout::Node,
    ) -> std::io::Result<PathBuf> {
        let dir = layouts_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{name}.toml"));
        let file = super::tui::layout::File { node: node.clone() };
        let text = toml::to_string_pretty(&file).map_err(std::io::Error::other)?;
        std::fs::write(&path, text)?;
        Ok(path)
    }

    pub fn root_path(&self) -> &Path {
        Path::new(&self.root)
    }
}
