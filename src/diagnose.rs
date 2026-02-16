use crate::config::Config;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct DiagnoseReport {
    version: String,
    workspace: String,
    config_path: String,
    config_exists: bool,
    provider: ProviderState,
    runtime: RuntimeState,
    healthchecks: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
struct ProviderState {
    default_provider: String,
    default_model: String,
    has_api_key: bool,
    reliability_provider_retries: u32,
}

#[derive(Debug, Serialize)]
struct RuntimeState {
    kind: String,
    heartbeat_enabled: bool,
    heartbeat_interval_minutes: u32,
    daemon_state_file: String,
    daemon_state_age_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
struct CheckResult {
    name: String,
    ok: bool,
    detail: String,
}

pub fn run(config: &Config) -> Result<()> {
    let state_file = crate::daemon::state_file_path(config);
    let daemon_age = daemon_state_age_seconds(&state_file).ok().flatten();

    let mut checks = Vec::new();

    checks.push(CheckResult {
        name: "config.load".into(),
        ok: config.config_path.exists(),
        detail: if config.config_path.exists() {
            "config file present".into()
        } else {
            "config file missing".into()
        },
    });

    let workspace_write_ok = workspace_write_check(&config.workspace_dir).is_ok();
    checks.push(CheckResult {
        name: "workspace.write".into(),
        ok: workspace_write_ok,
        detail: if workspace_write_ok {
            "workspace writable".into()
        } else {
            "workspace not writable".into()
        },
    });

    checks.push(CheckResult {
        name: "provider.configured".into(),
        ok: config.default_provider.is_some(),
        detail: format!(
            "provider={} model={}",
            config.default_provider.as_deref().unwrap_or("openrouter"),
            config.default_model.as_deref().unwrap_or("(default)")
        ),
    });

    checks.push(CheckResult {
        name: "daemon.state.fresh".into(),
        ok: daemon_age.is_some_and(|age| age <= 60),
        detail: daemon_age
            .map(|age| format!("state age {age}s"))
            .unwrap_or_else(|| "state file missing/stale".into()),
    });

    checks.push(CheckResult {
        name: "memory.backend".into(),
        ok: matches!(
            config.memory.backend.as_str(),
            "sqlite" | "markdown" | "none"
        ),
        detail: format!("backend={}", config.memory.backend),
    });

    let report = DiagnoseReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        workspace: config.workspace_dir.display().to_string(),
        config_path: config.config_path.display().to_string(),
        config_exists: config.config_path.exists(),
        provider: ProviderState {
            default_provider: config
                .default_provider
                .clone()
                .unwrap_or_else(|| "openrouter".into()),
            default_model: config
                .default_model
                .clone()
                .unwrap_or_else(|| "(default)".into()),
            has_api_key: config.api_key.as_ref().is_some_and(|v| !v.is_empty()),
            reliability_provider_retries: config.reliability.provider_retries,
        },
        runtime: RuntimeState {
            kind: config.runtime.kind.clone(),
            heartbeat_enabled: config.heartbeat.enabled,
            heartbeat_interval_minutes: config.heartbeat.interval_minutes,
            daemon_state_file: state_file.display().to_string(),
            daemon_state_age_seconds: daemon_age,
        },
        healthchecks: checks,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn workspace_write_check(workspace_dir: &std::path::Path) -> Result<()> {
    let probe = workspace_dir.join(".diagnose-write-probe");
    std::fs::write(&probe, b"ok").context("write probe")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

fn daemon_state_age_seconds(state_file: &std::path::Path) -> Result<Option<i64>> {
    if !state_file.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(state_file)
        .with_context(|| format!("read daemon state: {}", state_file.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw).context("parse daemon state json")?;
    let updated = json
        .get("updated_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let ts = DateTime::parse_from_rfc3339(updated)
        .map(|d| d.with_timezone(&Utc))
        .ok();
    Ok(ts.map(|ts| Utc::now().signed_duration_since(ts).num_seconds()))
}
