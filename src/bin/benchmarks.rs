use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use async_trait::async_trait;
use crabclaw::channels::traits::{Channel, ChannelMessage};
use crabclaw::memory::sqlite::SqliteMemory;
use crabclaw::memory::traits::{Memory, MemoryCategory};
use crabclaw::providers::reliable::{ReliableProvider, ReliableProviderStats};
use crabclaw::providers::traits::Provider;
use crabclaw::tools::traits::{Tool, ToolResult};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    metadata: BenchmarkMetadata,
    metrics: BTreeMap<String, f64>,
    raw_samples_ms: BTreeMap<String, Vec<f64>>,
}

#[derive(Debug, Serialize)]
struct BenchmarkMetadata {
    timestamp_utc: String,
    iterations: usize,
    note: String,
}

#[derive(Debug, Clone, Copy)]
enum BenchMode {
    Synthetic,
    Real,
}

impl BenchMode {
    fn from_env() -> Self {
        match std::env::var("CRABCLAW_BENCH_MODE") {
            Ok(v) if v.eq_ignore_ascii_case("real") => Self::Real,
            _ => Self::Synthetic,
        }
    }
}

struct SleepProvider {
    delay: Duration,
}

#[async_trait]
impl Provider for SleepProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        tokio::time::sleep(self.delay).await;
        Ok("ok".to_string())
    }
}

struct RealProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl Provider for RealProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let mut messages = vec![];
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({"role":"system","content":sys}));
        }
        messages.push(serde_json::json!({"role":"user","content":message}));

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.0,
            "max_tokens": 16
        });

        let res = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            anyhow::bail!("real provider call failed: {}", res.status());
        }

        let v: serde_json::Value = res.json().await?;
        let text = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(text)
    }
}

struct SleepChannel {
    delay: Duration,
}

#[async_trait]
impl Channel for SleepChannel {
    fn name(&self) -> &str {
        "sleep-channel"
    }

    async fn send(&self, _message: &str, _recipient: &str) -> anyhow::Result<()> {
        tokio::time::sleep(self.delay).await;
        Ok(())
    }

    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }
}

struct RealWebhookChannel {
    client: reqwest::Client,
    webhook_url: String,
}

#[async_trait]
impl Channel for RealWebhookChannel {
    fn name(&self) -> &str {
        "real-webhook-channel"
    }

    async fn send(&self, message: &str, recipient: &str) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "recipient": recipient,
            "message": message,
            "source": "crabclaw-benchmark"
        });
        let res = self
            .client
            .post(&self.webhook_url)
            .json(&body)
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!("real webhook send failed: {}", res.status());
        }
        Ok(())
    }

    async fn listen(&self, _tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        Ok(())
    }
}

struct SleepTool {
    delay: Duration,
}

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &str {
        "sleep-tool"
    }

    fn description(&self) -> &str {
        "Synthetic tool latency benchmark"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        tokio::time::sleep(self.delay).await;
        Ok(ToolResult {
            success: true,
            output: "ok".to_string(),
            error: None,
        })
    }
}

struct RealCommandTool {
    command: String,
}

#[async_trait]
impl Tool for RealCommandTool {
    fn name(&self) -> &str {
        "real-command-tool"
    }

    fn description(&self) -> &str {
        "Runs a real command for benchmark timing"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let out = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&self.command)
            .output()
            .await
            .context("run real benchmark tool command")?;

        Ok(ToolResult {
            success: out.status.success(),
            output: String::from_utf8_lossy(&out.stdout).to_string(),
            error: if out.status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&out.stderr).to_string())
            },
        })
    }
}

fn percentile_ms(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn average(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

fn insert_latency_metrics(metrics: &mut BTreeMap<String, f64>, key_prefix: &str, samples: &[f64]) {
    metrics.insert(
        format!("{key_prefix}.median_ms"),
        percentile_ms(samples, 0.50),
    );
    metrics.insert(format!("{key_prefix}.p90_ms"), percentile_ms(samples, 0.90));
    metrics.insert(format!("{key_prefix}.p95_ms"), percentile_ms(samples, 0.95));
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

struct FlakyProvider {
    attempts: std::sync::Mutex<usize>,
    fail_for_attempts: usize,
    timeout_error: bool,
}

#[async_trait]
impl Provider for FlakyProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        *attempts += 1;
        if *attempts <= self.fail_for_attempts {
            if self.timeout_error {
                anyhow::bail!("request timeout")
            }
            anyhow::bail!("temporary failure")
        }
        Ok("ok".to_string())
    }
}

async fn bench_provider(provider: &dyn Provider, iterations: usize) -> anyhow::Result<Vec<f64>> {
    let mut out = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t0 = Instant::now();
        provider
            .chat("hello", "benchmark-model", 0.0)
            .await
            .context("provider benchmark call")?;
        out.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(out)
}

async fn bench_channel(channel: &dyn Channel, iterations: usize) -> anyhow::Result<Vec<f64>> {
    let mut out = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t0 = Instant::now();
        channel.send("hello", "bench-user").await?;
        out.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(out)
}

async fn bench_tool(tool: &dyn Tool, iterations: usize) -> anyhow::Result<Vec<f64>> {
    let mut out = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t0 = Instant::now();
        tool.execute(serde_json::json!({})).await?;
        out.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(out)
}

async fn bench_memory_recall(iterations: usize) -> anyhow::Result<(Vec<f64>, f64, f64)> {
    let mut dir = std::env::temp_dir();
    dir.push(format!("crabclaw-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;

    let mem = SqliteMemory::new(&dir)?;

    for i in 0..200 {
        let topic = if i % 2 == 0 { "rust" } else { "python" };
        mem.store(
            &format!("bench-key-{i}"),
            &format!(
                "This is benchmark content number {i} about {topic} memory recall latency testing."
            ),
            MemoryCategory::Conversation,
        )
        .await?;
    }

    let mut out = Vec::with_capacity(iterations);
    let mut hit = 0usize;
    let mut precision_sum = 0.0f64;
    for i in 0..iterations {
        let topic = if i % 2 == 0 { "rust" } else { "python" };
        let t0 = Instant::now();
        let rows = mem.recall(topic, 10).await?;
        out.push(t0.elapsed().as_secs_f64() * 1000.0);

        if rows
            .iter()
            .any(|r| r.content.to_lowercase().contains(topic))
        {
            hit += 1;
        }
        if !rows.is_empty() {
            let relevant = rows
                .iter()
                .filter(|r| r.content.to_lowercase().contains(topic))
                .count();
            precision_sum += relevant as f64 / rows.len() as f64;
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    let hit_at_k = hit as f64 / iterations as f64;
    let precision_proxy = precision_sum / iterations as f64;
    Ok((out, hit_at_k, precision_proxy))
}

async fn probe_http_breakdown(base_url: &str) -> anyhow::Result<(f64, f64, f64)> {
    use tokio::net::{lookup_host, TcpStream};

    let url = reqwest::Url::parse(base_url).context("parse provider URL for http breakdown")?;
    let host = url
        .host_str()
        .context("provider URL host missing for http breakdown")?;
    let port = url.port_or_known_default().unwrap_or(443);

    let dns_t0 = Instant::now();
    let addrs: Vec<_> = lookup_host((host, port)).await?.collect();
    let dns_ms = dns_t0.elapsed().as_secs_f64() * 1000.0;

    let connect_t0 = Instant::now();
    let _sock = TcpStream::connect(
        addrs
            .first()
            .context("no resolved address for provider host")?,
    )
    .await?;
    let connect_ms = connect_t0.elapsed().as_secs_f64() * 1000.0;

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()?;
    let ttfb_t0 = Instant::now();
    let _ = client.get(base_url).send().await;
    let ttfb_ms = ttfb_t0.elapsed().as_secs_f64() * 1000.0;

    Ok((dns_ms, connect_ms, ttfb_ms))
}

async fn collect_reliability_observability_metrics() -> anyhow::Result<ReliableProviderStats> {
    let provider = ReliableProvider::new(
        vec![(
            "flaky".to_string(),
            Box::new(FlakyProvider {
                attempts: std::sync::Mutex::new(0),
                fail_for_attempts: 2,
                timeout_error: true,
            }),
        )],
        3,
        1,
    );

    // First call triggers retries/timeouts; second call should hit cache.
    let _ = provider
        .chat_with_system(Some("bench"), "hello", "benchmark-model", 0.0)
        .await;
    let _ = provider
        .chat_with_system(Some("bench"), "hello", "benchmark-model", 0.0)
        .await;

    Ok(provider.stats_snapshot())
}

fn benchmark_cost_per_task_usd() -> f64 {
    // Override-capable synthetic cost model.
    let input_tokens = env_f64("CRABCLAW_BENCH_INPUT_TOKENS", 1200.0);
    let output_tokens = env_f64("CRABCLAW_BENCH_OUTPUT_TOKENS", 500.0);
    let input_rate_per_million = env_f64("CRABCLAW_BENCH_INPUT_RATE_PER_M", 5.0);
    let output_rate_per_million = env_f64("CRABCLAW_BENCH_OUTPUT_RATE_PER_M", 15.0);

    (input_tokens / 1_000_000.0) * input_rate_per_million
        + (output_tokens / 1_000_000.0) * output_rate_per_million
}

fn parse_output_path() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut out = PathBuf::from("benchmark/results/latest.json");
    while let Some(arg) = args.next() {
        if arg == "--output" {
            if let Some(v) = args.next() {
                out = PathBuf::from(v);
            }
        }
    }
    out
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output_path = parse_output_path();
    let iterations = env_usize("CRABCLAW_BENCH_ITERATIONS", 60);
    let mode = BenchMode::from_env();

    let mut note_parts: Vec<String> = vec![];

    let provider_fast: Vec<f64>;
    let provider_normal: Vec<f64>;
    let channel_lat: Vec<f64>;
    let tool_lat: Vec<f64>;

    let mut real_provider_used = 0.0;
    let mut real_channel_used = 0.0;
    let mut real_tool_used = 0.0;

    match mode {
        BenchMode::Synthetic => {
            let fast_provider = SleepProvider {
                delay: Duration::from_millis(14),
            };
            let normal_provider = SleepProvider {
                delay: Duration::from_millis(32),
            };
            provider_fast = bench_provider(&fast_provider, iterations).await?;
            provider_normal = bench_provider(&normal_provider, iterations).await?;

            let channel = SleepChannel {
                delay: Duration::from_millis(18),
            };
            channel_lat = bench_channel(&channel, iterations).await?;

            let tool = SleepTool {
                delay: Duration::from_millis(11),
            };
            tool_lat = bench_tool(&tool, iterations).await?;

            note_parts.push("synthetic mode".to_string());
        }
        BenchMode::Real => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?;

            let require_real = std::env::var("CRABCLAW_BENCH_REAL_REQUIRED")
                .ok()
                .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
                .unwrap_or(false);

            let provider_url = std::env::var("CRABCLAW_BENCH_REAL_PROVIDER_URL").ok();
            let provider_key = std::env::var("CRABCLAW_BENCH_REAL_PROVIDER_API_KEY").ok();
            let provider_model = std::env::var("CRABCLAW_BENCH_REAL_PROVIDER_MODEL")
                .unwrap_or_else(|_| "gpt-4o-mini".to_string());

            if let (Some(url), Some(key)) = (provider_url, provider_key) {
                let real_provider = RealProvider {
                    client: client.clone(),
                    base_url: url,
                    api_key: key,
                    model: provider_model,
                };
                provider_fast = bench_provider(&real_provider, iterations).await?;
                provider_normal = provider_fast.clone();
                real_provider_used = 1.0;
                note_parts.push("real provider".to_string());
            } else if require_real {
                anyhow::bail!(
                    "CRABCLAW_BENCH_MODE=real with CRABCLAW_BENCH_REAL_REQUIRED=true requires provider envs"
                );
            } else {
                let fallback = SleepProvider {
                    delay: Duration::from_millis(14),
                };
                provider_fast = bench_provider(&fallback, iterations).await?;
                provider_normal = provider_fast.clone();
                note_parts.push("real provider unavailable -> synthetic fallback".to_string());
            }

            if let Ok(webhook) = std::env::var("CRABCLAW_BENCH_REAL_CHANNEL_WEBHOOK_URL") {
                let real_channel = RealWebhookChannel {
                    client: client.clone(),
                    webhook_url: webhook,
                };
                channel_lat = bench_channel(&real_channel, iterations).await?;
                real_channel_used = 1.0;
                note_parts.push("real channel".to_string());
            } else if require_real {
                anyhow::bail!(
                    "CRABCLAW_BENCH_MODE=real with CRABCLAW_BENCH_REAL_REQUIRED=true requires CRABCLAW_BENCH_REAL_CHANNEL_WEBHOOK_URL"
                );
            } else {
                let fallback = SleepChannel {
                    delay: Duration::from_millis(18),
                };
                channel_lat = bench_channel(&fallback, iterations).await?;
                note_parts.push("real channel unavailable -> synthetic fallback".to_string());
            }

            if let Ok(cmd) = std::env::var("CRABCLAW_BENCH_REAL_TOOL_COMMAND") {
                let real_tool = RealCommandTool { command: cmd };
                tool_lat = bench_tool(&real_tool, iterations).await?;
                real_tool_used = 1.0;
                note_parts.push("real tool".to_string());
            } else if require_real {
                anyhow::bail!(
                    "CRABCLAW_BENCH_MODE=real with CRABCLAW_BENCH_REAL_REQUIRED=true requires CRABCLAW_BENCH_REAL_TOOL_COMMAND"
                );
            } else {
                let fallback = SleepTool {
                    delay: Duration::from_millis(11),
                };
                tool_lat = bench_tool(&fallback, iterations).await?;
                note_parts.push("real tool unavailable -> synthetic fallback".to_string());
            }
        }
    }

    let (memory_recall, memory_hit_at_k, memory_precision_proxy) =
        bench_memory_recall(iterations).await?;

    // TTFT proxy
    let ttft_p95 = percentile_ms(&provider_fast, 0.95);

    let mut metrics = BTreeMap::new();
    insert_latency_metrics(&mut metrics, "provider.fast", &provider_fast);
    insert_latency_metrics(&mut metrics, "provider.normal", &provider_normal);
    insert_latency_metrics(&mut metrics, "channel.send", &channel_lat);
    insert_latency_metrics(&mut metrics, "tool.exec", &tool_lat);
    insert_latency_metrics(&mut metrics, "memory.recall", &memory_recall);

    metrics.insert("memory.recall.avg_ms".to_string(), average(&memory_recall));
    metrics.insert("memory.recall.hit_at_k".to_string(), memory_hit_at_k);
    metrics.insert(
        "memory.recall.precision_proxy".to_string(),
        memory_precision_proxy,
    );
    metrics.insert(
        "ttft.p90_ms".to_string(),
        percentile_ms(&provider_fast, 0.90),
    );
    metrics.insert("ttft.p95_ms".to_string(), ttft_p95);
    metrics.insert(
        "ttft.median_ms".to_string(),
        percentile_ms(&provider_fast, 0.50),
    );
    metrics.insert(
        "cost.per_task_usd".to_string(),
        benchmark_cost_per_task_usd(),
    );
    metrics.insert(
        "bench.mode.real".to_string(),
        if matches!(mode, BenchMode::Real) {
            1.0
        } else {
            0.0
        },
    );
    metrics.insert("bench.real_provider_used".to_string(), real_provider_used);
    metrics.insert("bench.real_channel_used".to_string(), real_channel_used);
    metrics.insert("bench.real_tool_used".to_string(), real_tool_used);

    let reliability_stats = collect_reliability_observability_metrics().await?;
    metrics.insert(
        "provider.retry_count".to_string(),
        reliability_stats.retry_count as f64,
    );
    metrics.insert(
        "provider.coalesced_wait_count".to_string(),
        reliability_stats.coalesced_wait_count as f64,
    );
    metrics.insert(
        "provider.hedge_launch_count".to_string(),
        reliability_stats.hedge_launch_count as f64,
    );
    metrics.insert(
        "provider.hedge_win_count".to_string(),
        reliability_stats.hedge_win_count as f64,
    );
    metrics.insert(
        "provider.timeout_rate".to_string(),
        reliability_stats.timeout_rate(),
    );
    metrics.insert(
        "circuitbreaker.open_count".to_string(),
        reliability_stats.circuit_open_count as f64,
    );
    metrics.insert(
        "circuitbreaker.half_open_count".to_string(),
        reliability_stats.circuit_half_open_count as f64,
    );
    metrics.insert(
        "circuitbreaker.close_count".to_string(),
        reliability_stats.circuit_close_count as f64,
    );
    metrics.insert(
        "cache.response.hit_rate".to_string(),
        reliability_stats.cache_hit_rate(),
    );

    if matches!(mode, BenchMode::Real) {
        if let Ok(base_url) = std::env::var("CRABCLAW_BENCH_REAL_PROVIDER_URL") {
            if let Ok((dns_ms, connect_ms, ttfb_ms)) = probe_http_breakdown(&base_url).await {
                metrics.insert("http.dns_ms".to_string(), dns_ms);
                metrics.insert("http.connect_ms".to_string(), connect_ms);
                metrics.insert("http.ttfb_ms".to_string(), ttfb_ms);
            }
        }
    }

    let mut raw_samples_ms = BTreeMap::new();
    raw_samples_ms.insert("provider.fast".to_string(), provider_fast.clone());
    raw_samples_ms.insert("provider.normal".to_string(), provider_normal.clone());
    raw_samples_ms.insert("channel.send".to_string(), channel_lat.clone());
    raw_samples_ms.insert("tool.exec".to_string(), tool_lat.clone());
    raw_samples_ms.insert("memory.recall".to_string(), memory_recall.clone());

    let report = BenchmarkReport {
        metadata: BenchmarkMetadata {
            timestamp_utc: chrono::Utc::now().to_rfc3339(),
            iterations,
            note: note_parts.join("; "),
        },
        metrics,
        raw_samples_ms,
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, serde_json::to_vec_pretty(&report)?)?;
    println!("Wrote benchmark report to {}", output_path.display());

    Ok(())
}
