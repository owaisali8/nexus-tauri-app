use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunRequest {
    pub agent_id: String,
    pub target_url: String,
    pub objective: String,
    pub resume_file_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRun {
    pub id: String,
    pub agent_id: String,
    pub target_url: String,
    pub objective: String,
    pub resume_file_name: String,
    pub status: BrowserRunStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRunStatus {
    Draft,
    Ready,
    Blocked,
}

pub fn prepare_run(request: BrowserRunRequest) -> Result<BrowserRun, String> {
    if request.agent_id.trim().is_empty() {
        return Err("agent_id is required".to_string());
    }

    if !request.target_url.starts_with("http://") && !request.target_url.starts_with("https://") {
        return Err("target_url must start with http:// or https://".to_string());
    }

    if request.objective.trim().len() < 12 {
        return Err("objective is too short for a browser automation run".to_string());
    }

    let resume_lower = request.resume_file_name.to_lowercase();
    let status = if resume_lower.ends_with(".pdf") {
        BrowserRunStatus::Ready
    } else {
        BrowserRunStatus::Blocked
    };

    Ok(BrowserRun {
        id: format!("run-{}", unix_timestamp()),
        agent_id: request.agent_id,
        target_url: request.target_url,
        objective: request.objective,
        resume_file_name: request.resume_file_name,
        status,
    })
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
