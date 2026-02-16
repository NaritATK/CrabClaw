use super::traits::ChatMessage;
use super::Provider;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Check if an error is non-retryable (client errors that won't resolve with retries).
fn is_non_retryable(err: &anyhow::Error) -> bool {
    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        if let Some(status) = reqwest_err.status() {
            let code = status.as_u16();
            return status.is_client_error() && code != 429 && code != 408;
        }
    }
    let msg = err.to_string();
    for word in msg.split(|c: char| !c.is_ascii_digit()) {
        if let Ok(code) = word.parse::<u16>() {
            if (400..500).contains(&code) {
                return code != 429 && code != 408;
            }
        }
    }
    false
}

#[derive(Debug, Clone)]
struct CircuitState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl CircuitState {
    fn healthy() -> Self {
        Self {
            consecutive_failures: 0,
            open_until: None,
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    response: String,
    inserted_at: Instant,
}

/// Provider wrapper with retry + fallback + circuit-breaker + response-cache.
pub struct ReliableProvider {
    providers: Vec<(String, Box<dyn Provider>)>,
    max_retries: u32,
    base_backoff_ms: u64,

    circuit_breaker_failure_threshold: u32,
    circuit_breaker_cooldown_ms: u64,
    circuit_states: Mutex<HashMap<String, CircuitState>>,

    cache_ttl_secs: u64,
    cache_max_entries: usize,
    cache_context_fingerprint: String,
    response_cache: Mutex<HashMap<String, CacheEntry>>,

    cb_open_count: AtomicU64,
    cb_half_open_count: AtomicU64,
    cb_close_count: AtomicU64,
}

impl ReliableProvider {
    pub fn new(
        providers: Vec<(String, Box<dyn Provider>)>,
        max_retries: u32,
        base_backoff_ms: u64,
    ) -> Self {
        let cb_threshold = std::env::var("CRABCLAW_PROVIDER_CB_FAILURE_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(3);

        let cb_cooldown = std::env::var("CRABCLAW_PROVIDER_CB_COOLDOWN_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v >= 250)
            .unwrap_or(30_000);

        let cache_ttl_secs = std::env::var("CRABCLAW_PROVIDER_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(120);

        let cache_max_entries = std::env::var("CRABCLAW_PROVIDER_CACHE_MAX_ENTRIES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(256);

        let provider_chain = providers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let tool_schema_hash = std::env::var("CRABCLAW_TOOL_SCHEMA_HASH").unwrap_or_default();
        let cache_context_fingerprint =
            format!("providers={provider_chain};tools={tool_schema_hash}");

        Self {
            providers,
            max_retries,
            base_backoff_ms: base_backoff_ms.max(50),
            circuit_breaker_failure_threshold: cb_threshold,
            circuit_breaker_cooldown_ms: cb_cooldown,
            circuit_states: Mutex::new(HashMap::new()),
            cache_ttl_secs,
            cache_max_entries,
            cache_context_fingerprint,
            response_cache: Mutex::new(HashMap::new()),
            cb_open_count: AtomicU64::new(0),
            cb_half_open_count: AtomicU64::new(0),
            cb_close_count: AtomicU64::new(0),
        }
    }

    fn cache_key_chat(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> String {
        format!(
            "chat|{}|{}|{}|{:.4}|{}",
            system_prompt.unwrap_or_default(),
            message,
            model,
            temperature,
            self.cache_context_fingerprint,
        )
    }

    fn cache_key_history(&self, messages: &[ChatMessage], model: &str, temperature: f64) -> String {
        let messages_json = serde_json::to_string(messages).unwrap_or_default();
        format!(
            "history|{}|{}|{:.4}|{}",
            messages_json, model, temperature, self.cache_context_fingerprint,
        )
    }

    fn cache_get(&self, key: &str) -> Option<String> {
        if self.cache_ttl_secs == 0 || self.cache_max_entries == 0 {
            return None;
        }

        let ttl = Duration::from_secs(self.cache_ttl_secs);
        let now = Instant::now();

        let mut cache = self
            .response_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        cache.retain(|_, v| now.duration_since(v.inserted_at) <= ttl);
        cache.get(key).map(|entry| entry.response.clone())
    }

    fn cache_put(&self, key: String, response: String) {
        if self.cache_ttl_secs == 0 || self.cache_max_entries == 0 {
            return;
        }

        let now = Instant::now();
        let mut cache = self
            .response_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        cache.insert(
            key,
            CacheEntry {
                response,
                inserted_at: now,
            },
        );

        if cache.len() > self.cache_max_entries {
            let mut keys: Vec<(String, Instant)> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.inserted_at))
                .collect();
            keys.sort_by_key(|(_, ts)| *ts);
            let to_remove = cache.len().saturating_sub(self.cache_max_entries);
            for (k, _) in keys.into_iter().take(to_remove) {
                cache.remove(&k);
            }
        }
    }

    fn circuit_metrics_snapshot(&self) -> (u64, u64, u64) {
        (
            self.cb_open_count.load(Ordering::Relaxed),
            self.cb_half_open_count.load(Ordering::Relaxed),
            self.cb_close_count.load(Ordering::Relaxed),
        )
    }

    fn circuit_allows_call(&self, provider_name: &str) -> bool {
        let now = Instant::now();
        let mut states = self
            .circuit_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let state = states
            .entry(provider_name.to_string())
            .or_insert_with(CircuitState::healthy);

        if let Some(until) = state.open_until {
            if now < until {
                return false;
            }
            self.cb_half_open_count.fetch_add(1, Ordering::Relaxed);
            state.open_until = None;
            state.consecutive_failures = 0;
            let (open_count, half_open_count, close_count) = self.circuit_metrics_snapshot();
            tracing::info!(
                provider = provider_name,
                circuit_open_count = open_count,
                circuit_half_open_count = half_open_count,
                circuit_close_count = close_count,
                "Circuit transitioned to half-open"
            );
        }
        true
    }

    fn circuit_record_success(&self, provider_name: &str) {
        let mut states = self
            .circuit_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let state = states
            .entry(provider_name.to_string())
            .or_insert_with(CircuitState::healthy);

        let should_count_close = state.open_until.is_some() || state.consecutive_failures > 0;
        state.consecutive_failures = 0;
        state.open_until = None;

        if should_count_close {
            self.cb_close_count.fetch_add(1, Ordering::Relaxed);
            let (open_count, half_open_count, close_count) = self.circuit_metrics_snapshot();
            tracing::info!(
                provider = provider_name,
                circuit_open_count = open_count,
                circuit_half_open_count = half_open_count,
                circuit_close_count = close_count,
                "Circuit closed after successful call"
            );
        }
    }

    fn circuit_record_failure(&self, provider_name: &str) {
        let mut states = self
            .circuit_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let state = states
            .entry(provider_name.to_string())
            .or_insert_with(CircuitState::healthy);

        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= self.circuit_breaker_failure_threshold {
            let should_count_open = state.open_until.is_none_or(|until| Instant::now() >= until);
            state.open_until =
                Some(Instant::now() + Duration::from_millis(self.circuit_breaker_cooldown_ms));
            if should_count_open {
                self.cb_open_count.fetch_add(1, Ordering::Relaxed);
                let (open_count, half_open_count, close_count) = self.circuit_metrics_snapshot();
                tracing::warn!(
                    provider = provider_name,
                    circuit_open_count = open_count,
                    circuit_half_open_count = half_open_count,
                    circuit_close_count = close_count,
                    "Circuit opened due to repeated failures"
                );
            }
        }
    }
}

#[async_trait]
impl Provider for ReliableProvider {
    async fn warmup(&self) -> anyhow::Result<()> {
        for (name, provider) in &self.providers {
            tracing::info!(provider = name, "Warming up provider connection pool");
            if let Err(e) = provider.warmup().await {
                tracing::warn!(provider = name, "Warmup failed (non-fatal): {e}");
            }
        }
        Ok(())
    }

    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let cache_key = self.cache_key_chat(system_prompt, message, model, temperature);
        if let Some(hit) = self.cache_get(&cache_key) {
            tracing::debug!("Provider response cache hit (chat_with_system)");
            return Ok(hit);
        }

        let mut failures = Vec::new();

        for (provider_name, provider) in &self.providers {
            if !self.circuit_allows_call(provider_name) {
                failures.push(format!("{provider_name}: circuit open"));
                tracing::warn!(
                    provider = provider_name,
                    "Skipping provider due to open circuit breaker"
                );
                continue;
            }

            let mut backoff_ms = self.base_backoff_ms;

            for attempt in 0..=self.max_retries {
                match provider
                    .chat_with_system(system_prompt, message, model, temperature)
                    .await
                {
                    Ok(resp) => {
                        self.circuit_record_success(provider_name);
                        if attempt > 0 {
                            tracing::info!(
                                provider = provider_name,
                                attempt,
                                "Provider recovered after retries"
                            );
                        }
                        self.cache_put(cache_key.clone(), resp.clone());
                        return Ok(resp);
                    }
                    Err(e) => {
                        let non_retryable = is_non_retryable(&e);
                        failures.push(format!(
                            "{provider_name} attempt {}/{}: {e}",
                            attempt + 1,
                            self.max_retries + 1
                        ));

                        self.circuit_record_failure(provider_name);

                        if non_retryable {
                            tracing::warn!(
                                provider = provider_name,
                                "Non-retryable error, switching provider"
                            );
                            break;
                        }

                        if attempt < self.max_retries {
                            tracing::warn!(
                                provider = provider_name,
                                attempt = attempt + 1,
                                max_retries = self.max_retries,
                                "Provider call failed, retrying"
                            );
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(10_000);
                        }
                    }
                }
            }

            tracing::warn!(provider = provider_name, "Switching to fallback provider");
        }

        anyhow::bail!("All providers failed. Attempts:\n{}", failures.join("\n"))
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let cache_key = self.cache_key_history(messages, model, temperature);
        if let Some(hit) = self.cache_get(&cache_key) {
            tracing::debug!("Provider response cache hit (chat_with_history)");
            return Ok(hit);
        }

        let mut failures = Vec::new();

        for (provider_name, provider) in &self.providers {
            if !self.circuit_allows_call(provider_name) {
                failures.push(format!("{provider_name}: circuit open"));
                tracing::warn!(
                    provider = provider_name,
                    "Skipping provider due to open circuit breaker"
                );
                continue;
            }

            let mut backoff_ms = self.base_backoff_ms;

            for attempt in 0..=self.max_retries {
                match provider
                    .chat_with_history(messages, model, temperature)
                    .await
                {
                    Ok(resp) => {
                        self.circuit_record_success(provider_name);
                        if attempt > 0 {
                            tracing::info!(
                                provider = provider_name,
                                attempt,
                                "Provider recovered after retries"
                            );
                        }
                        self.cache_put(cache_key.clone(), resp.clone());
                        return Ok(resp);
                    }
                    Err(e) => {
                        let non_retryable = is_non_retryable(&e);
                        failures.push(format!(
                            "{provider_name} attempt {}/{}: {e}",
                            attempt + 1,
                            self.max_retries + 1
                        ));

                        self.circuit_record_failure(provider_name);

                        if non_retryable {
                            tracing::warn!(
                                provider = provider_name,
                                "Non-retryable error, switching provider"
                            );
                            break;
                        }

                        if attempt < self.max_retries {
                            tracing::warn!(
                                provider = provider_name,
                                attempt = attempt + 1,
                                max_retries = self.max_retries,
                                "Provider call failed, retrying"
                            );
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            backoff_ms = (backoff_ms.saturating_mul(2)).min(10_000);
                        }
                    }
                }
            }

            tracing::warn!(provider = provider_name, "Switching to fallback provider");
        }

        anyhow::bail!("All providers failed. Attempts:\n{}", failures.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MockProvider {
        calls: Arc<AtomicUsize>,
        fail_until_attempt: usize,
        response: &'static str,
        error: &'static str,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.fail_until_attempt {
                anyhow::bail!(self.error);
            }
            Ok(self.response.to_string())
        }

        async fn chat_with_history(
            &self,
            _messages: &[ChatMessage],
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.fail_until_attempt {
                anyhow::bail!(self.error);
            }
            Ok(self.response.to_string())
        }
    }

    #[tokio::test]
    async fn succeeds_without_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = ReliableProvider::new(
            vec![(
                "primary".into(),
                Box::new(MockProvider {
                    calls: Arc::clone(&calls),
                    fail_until_attempt: 0,
                    response: "ok",
                    error: "boom",
                }),
            )],
            2,
            1,
        );

        let result = provider.chat("hello", "test", 0.0).await.unwrap();
        assert_eq!(result, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_then_recovers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = ReliableProvider::new(
            vec![(
                "primary".into(),
                Box::new(MockProvider {
                    calls: Arc::clone(&calls),
                    fail_until_attempt: 1,
                    response: "recovered",
                    error: "temporary",
                }),
            )],
            2,
            1,
        );

        let result = provider.chat("hello", "test", 0.0).await.unwrap();
        assert_eq!(result, "recovered");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn falls_back_after_retries_exhausted() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));

        let provider = ReliableProvider::new(
            vec![
                (
                    "primary".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&primary_calls),
                        fail_until_attempt: usize::MAX,
                        response: "never",
                        error: "primary down",
                    }),
                ),
                (
                    "fallback".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&fallback_calls),
                        fail_until_attempt: 0,
                        response: "from fallback",
                        error: "fallback down",
                    }),
                ),
            ],
            1,
            1,
        );

        let result = provider.chat("hello", "test", 0.0).await.unwrap();
        assert_eq!(result, "from fallback");
        assert_eq!(primary_calls.load(Ordering::SeqCst), 2);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn returns_aggregated_error_when_all_providers_fail() {
        let provider = ReliableProvider::new(
            vec![
                (
                    "p1".into(),
                    Box::new(MockProvider {
                        calls: Arc::new(AtomicUsize::new(0)),
                        fail_until_attempt: usize::MAX,
                        response: "never",
                        error: "p1 error",
                    }),
                ),
                (
                    "p2".into(),
                    Box::new(MockProvider {
                        calls: Arc::new(AtomicUsize::new(0)),
                        fail_until_attempt: usize::MAX,
                        response: "never",
                        error: "p2 error",
                    }),
                ),
            ],
            0,
            1,
        );

        let err = provider
            .chat("hello", "test", 0.0)
            .await
            .expect_err("all providers should fail");
        let msg = err.to_string();
        assert!(msg.contains("All providers failed"));
        assert!(msg.contains("p1 attempt 1/1"));
        assert!(msg.contains("p2 attempt 1/1"));
    }

    #[test]
    fn non_retryable_detects_common_patterns() {
        assert!(is_non_retryable(&anyhow::anyhow!("400 Bad Request")));
        assert!(is_non_retryable(&anyhow::anyhow!("401 Unauthorized")));
        assert!(is_non_retryable(&anyhow::anyhow!("403 Forbidden")));
        assert!(is_non_retryable(&anyhow::anyhow!("404 Not Found")));
        assert!(is_non_retryable(&anyhow::anyhow!(
            "API error with 400 Bad Request"
        )));
        assert!(!is_non_retryable(&anyhow::anyhow!("429 Too Many Requests")));
        assert!(!is_non_retryable(&anyhow::anyhow!("408 Request Timeout")));
        assert!(!is_non_retryable(&anyhow::anyhow!(
            "500 Internal Server Error"
        )));
        assert!(!is_non_retryable(&anyhow::anyhow!("502 Bad Gateway")));
        assert!(!is_non_retryable(&anyhow::anyhow!("timeout")));
        assert!(!is_non_retryable(&anyhow::anyhow!("connection reset")));
    }

    #[tokio::test]
    async fn skips_retries_on_non_retryable_error() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));

        let provider = ReliableProvider::new(
            vec![
                (
                    "primary".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&primary_calls),
                        fail_until_attempt: usize::MAX,
                        response: "never",
                        error: "401 Unauthorized",
                    }),
                ),
                (
                    "fallback".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&fallback_calls),
                        fail_until_attempt: 0,
                        response: "from fallback",
                        error: "fallback err",
                    }),
                ),
            ],
            3,
            1,
        );

        let result = provider.chat("hello", "test", 0.0).await.unwrap();
        assert_eq!(result, "from fallback");
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn chat_with_history_retries_then_recovers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = ReliableProvider::new(
            vec![(
                "primary".into(),
                Box::new(MockProvider {
                    calls: Arc::clone(&calls),
                    fail_until_attempt: 1,
                    response: "history ok",
                    error: "temporary",
                }),
            )],
            2,
            1,
        );

        let messages = vec![ChatMessage::system("system"), ChatMessage::user("hello")];
        let result = provider
            .chat_with_history(&messages, "test", 0.0)
            .await
            .unwrap();
        assert_eq!(result, "history ok");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn chat_with_history_falls_back() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));

        let provider = ReliableProvider::new(
            vec![
                (
                    "primary".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&primary_calls),
                        fail_until_attempt: usize::MAX,
                        response: "never",
                        error: "primary down",
                    }),
                ),
                (
                    "fallback".into(),
                    Box::new(MockProvider {
                        calls: Arc::clone(&fallback_calls),
                        fail_until_attempt: 0,
                        response: "fallback ok",
                        error: "fallback err",
                    }),
                ),
            ],
            1,
            1,
        );

        let messages = vec![ChatMessage::user("hello")];
        let result = provider
            .chat_with_history(&messages, "test", 0.0)
            .await
            .unwrap();
        assert_eq!(result, "fallback ok");
        assert_eq!(primary_calls.load(Ordering::SeqCst), 2);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_hits_for_identical_chat_inputs() {
        let calls = Arc::new(AtomicUsize::new(0));
        std::env::set_var("CRABCLAW_PROVIDER_CACHE_TTL_SECS", "300");
        std::env::set_var("CRABCLAW_PROVIDER_CACHE_MAX_ENTRIES", "128");

        let provider = ReliableProvider::new(
            vec![(
                "primary".into(),
                Box::new(MockProvider {
                    calls: Arc::clone(&calls),
                    fail_until_attempt: 0,
                    response: "cached-response",
                    error: "n/a",
                }),
            )],
            1,
            1,
        );

        let a = provider.chat("same prompt", "m", 0.0).await.unwrap();
        let b = provider.chat("same prompt", "m", 0.0).await.unwrap();

        assert_eq!(a, "cached-response");
        assert_eq!(b, "cached-response");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        std::env::remove_var("CRABCLAW_PROVIDER_CACHE_TTL_SECS");
        std::env::remove_var("CRABCLAW_PROVIDER_CACHE_MAX_ENTRIES");
    }
}
