use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Bridge {
    pub base: String,
    pub project_id: i64,
    #[serde(default)]
    pub filter: Option<String>,
    pub api_key: String,
    #[serde(default)]
    pub name: Option<String>,
}

impl Bridge {
    pub fn api_root(&self) -> String {
        let base = self.base.trim().trim_end_matches('/');
        let base = base.strip_suffix("/api/v1").unwrap_or(base);
        format!("{}/api/v1", base.trim_end_matches('/'))
    }

    pub fn uid_domain(&self) -> &str {
        let s = self
            .base
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let host = s.split(['/', ':', '?']).next().unwrap_or("");
        if host.is_empty() {
            "vikunja.invalid"
        } else {
            host
        }
    }
}

pub type Bridges = HashMap<String, Bridge>;

pub fn load(path: &Path) -> Result<Bridges, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let bridges: Bridges =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;

    if bridges.is_empty() {
        return Err(format!("{} contains no bridges", path.display()));
    }
    for (secret, b) in &bridges {
        let label = redact(secret);
        if secret.len() < 16 {
            return Err(format!(
                "bridge {label}: secret is only {} chars; use at least 16 (it is the only access control)",
                secret.len()
            ));
        }
        if !(b.base.starts_with("http://") || b.base.starts_with("https://")) {
            return Err(format!(
                "bridge {label}: `base` must start with http:// or https://"
            ));
        }
        if b.api_key.trim().is_empty() {
            return Err(format!("bridge {label}: `api_key` is empty"));
        }
        if b.project_id <= 0 {
            return Err(format!(
                "bridge {label}: `project_id` must be a positive integer"
            ));
        }
    }
    Ok(bridges)
}

pub fn redact(secret: &str) -> String {
    let head: String = secret.chars().take(4).collect();
    format!("{head}…({} chars)", secret.chars().count())
}
