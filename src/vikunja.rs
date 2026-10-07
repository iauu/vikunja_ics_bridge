use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};

use crate::config::Bridge;

const PER_PAGE: u32 = 50;
const MAX_PAGES: u32 = 200;

#[derive(Debug, Deserialize)]
pub struct Label {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Task {
    pub id: i64,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub percent_done: f64,
    #[serde(default, deserialize_with = "optional_date")]
    pub start_date: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_date")]
    pub end_date: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_date")]
    pub due_date: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_date")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_date")]
    pub updated: Option<DateTime<Utc>>,
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
    #[serde(default)]
    pub repeat_after: Option<i64>,
    #[serde(default)]
    pub repeat_mode: i32,
}

/// Serialises optional date from Vikunja
fn optional_date<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
    let opt = Option::<DateTime<Utc>>::deserialize(d)?;
    Ok(opt.filter(|dt| chrono::Datelike::year(dt) > 1))
}

#[derive(Debug)]
pub enum FetchError {
    Upstream { status: u16, message: String },
    Transport(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Upstream { status, message } => {
                write!(f, "vikunja returned {status}: {message}")
            }
            FetchError::Transport(m) => write!(f, "cannot reach vikunja: {m}"),
        }
    }
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

pub async fn fetch_tasks(http: &reqwest::Client, bridge: &Bridge) -> Result<Vec<Task>, FetchError> {
    let url = format!("{}/projects/{}/tasks", bridge.api_root(), bridge.project_id);
    let filter = bridge.filter.as_deref().unwrap_or("");

    let mut tasks = Vec::new();
    for page in 1..=MAX_PAGES {
        let resp = http
            .get(&url)
            .bearer_auth(&bridge.api_key)
            .query(&[
                ("filter", filter.to_string()),
                ("page", page.to_string()),
                ("per_page", PER_PAGE.to_string()),
                ("sort_by", "start_date".to_string()),
                ("order_by", "asc".to_string()),
            ])
            .send()
            .await
            .map_err(|e| FetchError::Transport(e.to_string()))?;

        let status = resp.status();
        let total_pages: Option<u32> = resp
            .headers()
            .get("x-pagination-total-pages")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());

        let body = resp
            .text()
            .await
            .map_err(|e| FetchError::Transport(e.to_string()))?;

        if !status.is_success() {
            let message = serde_json::from_str::<ApiError>(&body)
                .map(|e| e.message)
                .ok()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| body.chars().take(300).collect());
            return Err(FetchError::Upstream {
                status: status.as_u16(),
                message,
            });
        }

        let page_tasks: Vec<Task> = serde_json::from_str(&body)
            .map_err(|e| FetchError::Transport(format!("unexpected task payload: {e}")))?;
        let got = page_tasks.len();
        tasks.extend(page_tasks);

        let done = match total_pages {
            Some(tp) => page >= tp.max(1),
            None => got < PER_PAGE as usize,
        };
        if done || got == 0 {
            break;
        }
    }
    Ok(tasks)
}
