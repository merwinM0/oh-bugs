use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_error_keywords")]
    pub error_keywords: Vec<String>,
    #[serde(default = "default_min_bugs")]
    pub min_bugs: usize,
    #[serde(default = "default_max_bugs")]
    pub max_bugs: usize,
    #[serde(default = "default_bug_lifetime_ms")]
    pub bug_lifetime_ms: u64,
    #[serde(default = "default_refresh_rate_ms")]
    pub refresh_rate_ms: u64,
    #[serde(default = "default_max_concurrent_bugs")]
    pub max_concurrent_bugs: usize,
}

fn default_error_keywords() -> Vec<String> {
    vec![
        "error".to_string(),
        "fail".to_string(),
        "exception".to_string(),
        "fatal".to_string(),
    ]
}
fn default_min_bugs() -> usize {
    2
}
fn default_max_bugs() -> usize {
    5
}
fn default_bug_lifetime_ms() -> u64 {
    2500
}
fn default_refresh_rate_ms() -> u64 {
    100
}
fn default_max_concurrent_bugs() -> usize {
    30
}

impl Default for Config {
    fn default() -> Self {
        Self {
            error_keywords: default_error_keywords(),
            min_bugs: default_min_bugs(),
            max_bugs: default_max_bugs(),
            bug_lifetime_ms: default_bug_lifetime_ms(),
            refresh_rate_ms: default_refresh_rate_ms(),
            max_concurrent_bugs: default_max_concurrent_bugs(),
        }
    }
}

impl Config {
    /// 加载配置文件，若文件不存在则返回默认配置
    pub fn load() -> Self {
        let config_path = Self::config_path();
        if let Some(ref path) = config_path {
            if path.exists() {
                let content = std::fs::read_to_string(path).unwrap_or_default();
                if !content.is_empty() {
                    if let Ok(config) = toml::from_str(&content) {
                        return config;
                    }
                }
            }
        }
        // 写入默认配置到文件
        if let Some(path) = &config_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
                let default_toml = toml::to_string_pretty(&Self::default()).unwrap();
                let _ = std::fs::write(path, default_toml);
            }
        }
        Self::default()
    }

    fn config_path() -> Option<PathBuf> {
        let base = dirs::config_dir()?;
        Some(base.join("obugs").join("config.toml"))
    }
}
