use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use async_trait::async_trait;
use crabclaw::channels::traits::{Channel, ChannelMessage};
use crabclaw::memory::sqlite::SqliteMemory;
use crabclaw::memory::traits::{Memory, MemoryCategory};
use crabclaw::providers::traits::Provider;
use crabclaw::tools::traits::{Tool, ToolResult};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    metadata: BenchmarkMetadata,
    metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
struct BenchmarkMetadata {
    timestamp_utc: String,
    iterations: usize,
    note: String,
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

async fn bench_memory_recall(iterations: usize) -> anyhow::Result<Vec<f64>> {
    let mut dir = std::env::temp_dir();
    dir.push(format!("crabclaw-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;

    let mem = SqliteMemory::new(&dir)?;

    for i in 0..200 {
        mem.store(
            &format!("bench-key-{i}"),
            &format!("This is benchmark content number {i} for memory recall latency testing."),
            MemoryCategory::Conversation,
        )
        .await?;
    }

    let mut out = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let t0 = Instant::now();
        let _ = mem.recall(&format!("benchmark {}", i % 20), 10).await?;
        out.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(out)
}

fn benchmark_cost_per_task_usd() -> f64 {
    // Synthetic but stable: estimate cost for a representative task profile.
    // 1 task = 1,200 input tokens + 500 output tokens.
    // Default reference rates (USD per 1M tokens): input=5.0, output=15.0.
    let input_tokens = 1200.0;
    let output_tokens = 500.0;
    let input_rate_per_million = 5.0;
    let output_rate_per_million = 15.0;

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
    let iterations = 60usize;

    let fast_provider = SleepProvider {
        delay: Duration::from_millis(14),
    };
    let normal_provider = SleepProvider {
        delay: Duration::from_millis(32),
    };

    let provider_fast = bench_provider(&fast_provider, iterations).await?;
    let provider_normal = bench_provider(&normal_provider, iterations).await?;

    let channel = SleepChannel {
        delay: Duration::from_millis(18),
    };
    let channel_lat = bench_channel(&channel, iterations).await?;

    let tool = SleepTool {
        delay: Duration::from_millis(11),
    };
    let tool_lat = bench_tool(&tool, iterations).await?;

    let memory_recall = bench_memory_recall(iterations).await?;

    // Synthetic TTFT benchmark (approximated by fast provider latency p95)
    let ttft_p95 = percentile_ms(&provider_fast, 0.95);

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "provider.fast.p95_ms".to_string(),
        percentile_ms(&provider_fast, 0.95),
    );
    metrics.insert(
        "provider.normal.p95_ms".to_string(),
        percentile_ms(&provider_normal, 0.95),
    );
    metrics.insert(
        "channel.send.p95_ms".to_string(),
        percentile_ms(&channel_lat, 0.95),
    );
    metrics.insert("tool.exec.p95_ms".to_string(), percentile_ms(&tool_lat, 0.95));
    metrics.insert(
        "memory.recall.p95_ms".to_string(),
        percentile_ms(&memory_recall, 0.95),
    );
    metrics.insert(
        "memory.recall.avg_ms".to_string(),
        average(&memory_recall),
    );
    metrics.insert("ttft.p95_ms".to_string(), ttft_p95);
    metrics.insert("cost.per_task_usd".to_string(), benchmark_cost_per_task_usd());

    let report = BenchmarkReport {
        metadata: BenchmarkMetadata {
            timestamp_utc: chrono::Utc::now().to_rfc3339(),
            iterations,
            note: "Synthetic benchmark suite for regression detection in CI".to_string(),
        },
        metrics,
    };

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, serde_json::to_vec_pretty(&report)?)?;
    println!("Wrote benchmark report to {}", output_path.display());

    Ok(())
}
