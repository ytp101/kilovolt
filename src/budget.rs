use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::Bytes;
use futures_util::stream::Stream;
use tiktoken_rs::bpe_for_model;
use tracing::{error, info, warn};

use crate::config::AppState;
use crate::ledger::BudgetError;
use crate::pricing::ModelPricing;
use crate::proxy::streaming_output_tokens;

/// Pre-flight check for multi-tier token budgeting.
#[allow(clippy::collapsible_if)]
pub fn check_token_budgets(
    state: &AppState,
    pipeline_id: Option<&str>,
    prompt_tokens: usize,
) -> Result<(), String> {
    // 1. Day Rollover check
    let today = chrono::Local::now().date_naive();
    {
        let mut day_start_lock = state.day_start.write().unwrap();
        if *day_start_lock != today {
            *day_start_lock = today;
            state.tokens_used_today.store(0, Ordering::Relaxed);
        }
    }

    // 2. Per-Step Check (enforcing that this specific step's prompt tokens don't exceed the limit)
    if let Some(step_limit) = state.per_step_tokens {
        if prompt_tokens > step_limit {
            return Err(format!(
                "BUDGET_BLOCKED: step token limit {} exceeded by prompt size (prompt size: {})",
                step_limit, prompt_tokens
            ));
        }
    }

    // 3. Per-Day Check
    if let Some(day_limit) = state.per_day_tokens {
        let requested = state.per_step_tokens.unwrap_or(2048);
        let used_today = state.tokens_used_today.load(Ordering::Relaxed);
        if used_today + requested > day_limit {
            return Err(format!(
                "BUDGET_BLOCKED: daily token limit {} would be exceeded (used: {}, requested: {})",
                day_limit, used_today, requested
            ));
        }
    }

    // 4. Per-Pipeline Check
    if let Some(pipeline_limit) = state.per_pipeline_tokens {
        if let Some(pid) = pipeline_id {
            let requested = state.per_step_tokens.unwrap_or(2048);
            let tracker = state.pipeline_tracker.read().unwrap();
            let used_pipeline = tracker.get(pid).cloned().unwrap_or(0);
            if used_pipeline + requested > pipeline_limit {
                return Err(format!(
                    "BUDGET_BLOCKED: pipeline token limit {} would be exceeded (used: {}, requested: {})",
                    pipeline_limit, used_pipeline, requested
                ));
            }
        }
    }

    Ok(())
}

/// A bounded SSE frame monitor that reconstructs events independently of
/// transport chunk boundaries and charges output before making a frame
/// available to the downstream response body.
pub struct StreamMonitor<S> {
    pub inner: S,
    pub start_time: Instant,
    pub bytes_written: usize,
    pub chunks_written: usize,
    pub logged: bool,
    pub request_id: String,
    pub user_id: String,
    pub model: String,
    pub pricing: ModelPricing,
    pub prompt_tokens: usize,
    pub prompt_cost: f64,
    pub bpe: Option<&'static tiktoken_rs::CoreBPE>,
    pub total_spend: f64,
    pub user_budget_limit: f64,
    pub output_tokens_count: usize,
    pub state: AppState,
    pub is_gemini: bool,
    pub frame_buffer: Vec<u8>,
    pub pending_output: VecDeque<Bytes>,
    pub max_sse_frame_bytes: usize,
    pub sent_done: bool,
    pub pipeline_id: Option<String>,
    terminal: Option<(u16, &'static str, bool)>,
}

impl<S> StreamMonitor<S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: S,
        request_id: String,
        user_id: String,
        model: String,
        pricing: ModelPricing,
        prompt_tokens: usize,
        prompt_cost: f64,
        total_spend: f64,
        user_budget_limit: f64,
        state: AppState,
        is_gemini: bool,
        pipeline_id: Option<String>,
    ) -> Self {
        let bpe = bpe_for_model(&model)
            .ok()
            .or_else(|| bpe_for_model("gpt-4o").ok());

        Self {
            inner,
            start_time: Instant::now(),
            bytes_written: 0,
            chunks_written: 0,
            logged: false,
            request_id,
            user_id,
            model,
            pricing,
            prompt_tokens,
            prompt_cost,
            bpe,
            total_spend,
            user_budget_limit,
            output_tokens_count: 0,
            max_sse_frame_bytes: state.max_sse_frame_bytes,
            state,
            is_gemini,
            frame_buffer: Vec::new(),
            pending_output: VecDeque::new(),
            sent_done: false,
            pipeline_id,
            terminal: None,
        }
    }

    fn try_charge_output_tokens(&mut self, new_tokens: usize) -> Result<(), BudgetError> {
        let incremental_cost = new_tokens as f64 * self.pricing.output_cost_per_token;
        let (project_budget_limit, _) = self.state.effective_budgets();
        let snapshot = self.state.budget_ledger.try_charge_output(
            &self.user_id,
            incremental_cost,
            project_budget_limit,
            self.user_budget_limit,
        )?;

        self.output_tokens_count += new_tokens;
        self.state
            .total_tokens_consumed
            .fetch_add(new_tokens, Ordering::Relaxed);
        self.total_spend = snapshot.user.total_spend;
        Ok(())
    }

    fn token_count(&self, text: &str) -> usize {
        self.bpe.as_ref().map_or_else(
            || text.len().div_ceil(4),
            |bpe| bpe.encode_with_special_tokens(text).len(),
        )
    }

    fn charge_text(&mut self, text: &str) -> Result<(), BudgetError> {
        if text.is_empty() {
            return Ok(());
        }
        self.try_charge_output_tokens(self.token_count(text))
    }

    fn mark_terminal(&mut self, status: u16, outcome: &'static str, is_cutoff: bool) {
        if self.terminal.is_none() {
            self.terminal = Some((status, outcome, is_cutoff));
        }
    }

    fn fail_protocol(&mut self, reason: &'static str) {
        warn!(
            user_id = %self.user_id,
            request_id = %self.request_id,
            buffered_bytes = %self.frame_buffer.len(),
            max_sse_frame_bytes = %self.max_sse_frame_bytes,
            "Rejecting malformed upstream SSE stream: {}", reason
        );
        self.frame_buffer.clear();
        self.mark_terminal(502, reason, false);
    }

    fn process_frame(&mut self, frame: Vec<u8>) {
        if frame.len() > self.max_sse_frame_bytes {
            self.fail_protocol("upstream SSE frame exceeded configured limit");
            return;
        }

        let payload_end = if frame.ends_with(b"\r\n\r\n") {
            frame.len() - 4
        } else if frame.ends_with(b"\n\n") {
            frame.len() - 2
        } else {
            frame.len()
        };
        let Ok(event_text) = std::str::from_utf8(&frame[..payload_end]) else {
            self.fail_protocol("upstream SSE frame was not valid UTF-8");
            return;
        };

        let mut data_lines = Vec::new();
        for raw_line in event_text.split('\n') {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            if line.is_empty()
                || line.starts_with(':')
                || line.starts_with("event:")
                || line.starts_with("id:")
                || line.starts_with("retry:")
            {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                self.fail_protocol("upstream SSE frame contained an invalid field");
                return;
            };
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }

        // Comments and metadata-only events are valid SSE and carry no billable
        // completion content.
        if data_lines.is_empty() {
            self.pending_output.push_back(Bytes::from(frame));
            return;
        }

        let data = data_lines.join("\n");
        if data == "[DONE]" {
            self.sent_done = true;
            self.pending_output.push_back(Bytes::from(frame));
            self.frame_buffer.clear();
            self.mark_terminal(200, "completed successfully", false);
            return;
        }

        let value = match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(value) => value,
            Err(_) => {
                self.fail_protocol("upstream SSE data was not valid JSON");
                return;
            }
        };

        if self.is_gemini {
            let mut text_extracted = String::new();
            let mut finish_reason = None;
            if let Some(candidate) = value
                .get("candidates")
                .and_then(serde_json::Value::as_array)
                .and_then(|candidates| candidates.first())
            {
                if let Some(parts) = candidate
                    .get("content")
                    .and_then(|content| content.get("parts"))
                    .and_then(serde_json::Value::as_array)
                {
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                            text_extracted.push_str(text);
                        }
                    }
                }
                finish_reason = candidate
                    .get("finishReason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_ascii_lowercase);
            }

            if text_extracted.is_empty() && finish_reason.is_none() {
                self.fail_protocol("Gemini SSE frame did not contain a supported candidate");
                return;
            }
            if let Err(error) = self.charge_text(&text_extracted) {
                warn!(
                    user_id = %self.user_id,
                    error = %error,
                    "Budget rejected a Gemini output increment"
                );
                self.mark_terminal(429, "tripped mid-stream", true);
                return;
            }

            let normalized_finish_reason = match finish_reason.as_deref() {
                Some("stop" | "completed") => serde_json::Value::String("stop".to_string()),
                Some(other) => serde_json::Value::String(other.to_string()),
                None => serde_json::Value::Null,
            };
            let openai_chunk = serde_json::json!({
                "id": format!("chatcmpl-{}", self.request_id),
                "object": "chat.completion.chunk",
                "model": self.model,
                "choices": [{
                    "index": 0,
                    "delta": {"content": text_extracted},
                    "logprobs": null,
                    "finish_reason": normalized_finish_reason
                }]
            });
            self.pending_output
                .push_back(Bytes::from(format!("data: {openai_chunk}\n\n")));
            return;
        }

        let output_tokens = match streaming_output_tokens(&self.model, &value) {
            Ok(tokens) => tokens,
            Err(reason) => {
                self.fail_protocol(reason);
                return;
            }
        };
        if let Err(error) = self.try_charge_output_tokens(output_tokens) {
            let (project_budget_limit, _) = self.state.effective_budgets();
            warn!(
                user_id = %self.user_id,
                project_budget_limit = %project_budget_limit,
                user_budget_limit = %self.user_budget_limit,
                error = %error,
                "Budget rejected an OpenAI-compatible output increment"
            );
            self.mark_terminal(429, "tripped mid-stream", true);
            return;
        }
        self.pending_output.push_back(Bytes::from(frame));
    }

    fn ingest_transport_chunk(&mut self, bytes: &Bytes) {
        for byte in bytes {
            if self.frame_buffer.len() >= self.max_sse_frame_bytes {
                self.fail_protocol("upstream SSE frame exceeded configured limit");
                return;
            }
            self.frame_buffer.push(*byte);
            if self.frame_buffer.ends_with(b"\n\n") || self.frame_buffer.ends_with(b"\r\n\r\n") {
                let frame = std::mem::take(&mut self.frame_buffer);
                self.process_frame(frame);
                if self.terminal.is_some() {
                    return;
                }
            }
        }
    }

    /// Logs the final summary of the stream upon completion or cutoff.
    pub fn log_final_status(&mut self, status_code: u16, is_cutoff: bool, outcome: &str) {
        if self.logged {
            return;
        }
        let duration = self.start_time.elapsed();
        let duration_ms = duration.as_millis() as u64;
        let total_output_cost =
            self.output_tokens_count as f64 * self.pricing.output_cost_per_token;

        info!(
            user_id = %self.user_id,
            model = %self.model,
            duration = ?duration,
            chunks = %self.chunks_written,
            bytes = %self.bytes_written,
            prompt_cost = %self.prompt_cost,
            output_tokens = %self.output_tokens_count,
            output_cost = %total_output_cost,
            final_total_spend = %self.total_spend,
            cutoff = %is_cutoff,
            "Stream closed: {}", outcome
        );

        // Record request status for dashboard and average latency tracking
        let total_tokens = self.prompt_tokens + self.output_tokens_count;
        let request_cost = self.prompt_cost + total_output_cost;

        self.state.record_request(
            &self.request_id,
            &self.user_id,
            &self.model,
            status_code,
            duration_ms,
            total_tokens,
            request_cost,
        );

        // Record token budgets consumption
        self.state
            .tokens_used_today
            .fetch_add(total_tokens, Ordering::Relaxed);
        if let Some(pid) = &self.pipeline_id {
            let mut tracker = self.state.pipeline_tracker.write().unwrap();
            let entry = tracker.entry(pid.clone()).or_insert(0);
            *entry += total_tokens;
        }

        self.logged = true;
    }
}

impl<S, E> Stream for StreamMonitor<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    type Item = Result<Bytes, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(bytes) = this.pending_output.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }
        if let Some((status, outcome, is_cutoff)) = this.terminal.take() {
            this.log_final_status(status, is_cutoff, outcome);
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.bytes_written += bytes.len();
                    this.chunks_written += 1;
                    this.ingest_transport_chunk(&bytes);

                    if let Some(output) = this.pending_output.pop_front() {
                        return Poll::Ready(Some(Ok(output)));
                    }
                    if let Some((status, outcome, is_cutoff)) = this.terminal.take() {
                        this.log_final_status(status, is_cutoff, outcome);
                        return Poll::Ready(None);
                    }
                }
                Poll::Ready(Some(Err(err))) => {
                    let duration = this.start_time.elapsed();
                    error!(
                        "Stream failed after {:.2?} (chunks: {}, bytes: {}): {:?}",
                        duration, this.chunks_written, this.bytes_written, err
                    );
                    this.log_final_status(502, false, "upstream disconnected during stream");
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Ready(None) => {
                    if !this.frame_buffer.is_empty() {
                        let final_frame = std::mem::take(&mut this.frame_buffer);
                        this.process_frame(final_frame);
                    }
                    if this.terminal.is_none() && this.is_gemini && !this.sent_done {
                        this.sent_done = true;
                        this.pending_output
                            .push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                    }
                    if this.terminal.is_none() {
                        this.mark_terminal(200, "completed successfully", false);
                    }
                    if let Some(output) = this.pending_output.pop_front() {
                        return Poll::Ready(Some(Ok(output)));
                    }
                    let (status, outcome, is_cutoff) =
                        this.terminal.take().expect("terminal status should be set");
                    this.log_final_status(status, is_cutoff, outcome);
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// When the downstream client cancels the connection, Axum drops the stream,
// triggering this Drop implementation. This cleans up and logs the cancellation.
impl<S> Drop for StreamMonitor<S> {
    fn drop(&mut self) {
        if !self.logged {
            if let Some((status, outcome, is_cutoff)) = self.terminal.take() {
                self.log_final_status(status, is_cutoff, outcome);
            } else {
                self.log_final_status(499, false, "aborted by client");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamMonitor;
    use axum::body::Bytes;
    use futures_util::stream::{self, StreamExt};

    use crate::config::test_state;
    use crate::pricing::ModelPricing;

    #[tokio::test]
    async fn rejected_output_increment_is_not_charged_or_forwarded() {
        let state = test_state(0, 0.5);
        let reservation = state
            .budget_ledger
            .reserve_prompt("request", "user", 0.5, 0.5, 0.5)
            .expect("prompt reservation at limit should succeed");
        let committed = state
            .budget_ledger
            .commit_prompt("request", "user")
            .expect("prompt reservation should commit");
        assert_eq!(
            reservation.project.total_spend,
            committed.project.total_spend
        );
        assert_eq!(reservation.user.total_spend, committed.user.total_spend);

        let upstream = stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"forecast\",\"arguments\":\"{\\\"city\\\":\\\"Bangkok\\\"}\"}}]}}]}\n\n",
        ))]);
        let mut monitor = StreamMonitor::new(
            upstream,
            "request".to_string(),
            "user".to_string(),
            "gpt-4o".to_string(),
            ModelPricing {
                input_cost_per_token: 0.0,
                output_cost_per_token: 1.0,
            },
            0,
            0.5,
            committed.user.total_spend,
            0.5,
            state.clone(),
            false,
            None,
        );

        assert!(monitor.next().await.is_none());

        let after_rejection = state.budget_ledger.user_snapshot("user");
        assert_eq!(after_rejection.committed_spend, 0.5);
        assert_eq!(after_rejection.reserved_spend, 0.0);
        assert_eq!(after_rejection.total_spend, 0.5);
        assert_eq!(
            state
                .total_tokens_consumed
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    fn free_pricing() -> ModelPricing {
        ModelPricing {
            input_cost_per_token: 0.0,
            output_cost_per_token: 0.0,
        }
    }

    async fn monitor_output(chunks: Vec<Bytes>) -> (String, crate::config::AppState) {
        let state = test_state(0, 10.0);
        let upstream = stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Bytes, std::io::Error>)
                .collect::<Vec<_>>(),
        );
        let monitor = StreamMonitor::new(
            upstream,
            "request".to_string(),
            "user".to_string(),
            "gpt-4o".to_string(),
            free_pricing(),
            0,
            0.0,
            0.0,
            10.0,
            state.clone(),
            false,
            None,
        );
        let output = monitor
            .map(|chunk| chunk.expect("test stream should not fail"))
            .collect::<Vec<_>>()
            .await
            .concat();
        (
            String::from_utf8(output).expect("forwarded SSE should be UTF-8"),
            state,
        )
    }

    #[tokio::test]
    async fn arbitrary_chunks_preserve_order_utf8_delimiters_and_final_frame() {
        let expected = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"สวัสดี\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\r\n\r\n",
            "data: [DONE]"
        );
        let bytes = expected.as_bytes();
        let split_points = [2, 7, 19, 43, 44, 45, 68, bytes.len() - 2];
        let mut start = 0;
        let mut chunks = Vec::new();
        for end in split_points.into_iter().chain(std::iter::once(bytes.len())) {
            chunks.push(Bytes::copy_from_slice(&bytes[start..end]));
            start = end;
        }

        let (actual, _) = monitor_output(chunks).await;
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn one_chunk_can_contain_multiple_events() {
        let expected = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"two\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (actual, _) = monitor_output(vec![Bytes::from_static(expected.as_bytes())]).await;
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn split_function_and_tool_call_arguments_are_accounted_before_forwarding() {
        let expected = concat!(
            "data: {\"choices\":[{\"delta\":{\"function_call\":{\"name\":\"legacy\",\"arguments\":\"{\\\"a\\\":\"}}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"first\",\"arguments\":\"1}\"}},{\"index\":1,\"id\":\"call_2\",\"type\":\"function\",\"function\":{\"name\":\"second\",\"arguments\":\"{\\\"b\\\":2}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let bytes = expected.as_bytes();
        let midpoint = bytes.len() / 2;
        let (actual, state) = monitor_output(vec![
            Bytes::copy_from_slice(&bytes[..midpoint]),
            Bytes::copy_from_slice(&bytes[midpoint..]),
        ])
        .await;
        assert_eq!(actual, expected);
        assert!(
            state
                .total_tokens_consumed
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        );
    }

    #[tokio::test]
    async fn unknown_potentially_billable_delta_is_not_forwarded() {
        let event =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"\",\"audio\":\"hidden\"}}]}\n\n";
        let (actual, state) = monitor_output(vec![Bytes::from_static(event)]).await;
        assert!(actual.is_empty());
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(502)
        );
    }

    #[tokio::test]
    async fn final_json_event_without_trailing_newline_is_forwarded() {
        let expected = "data: {\"choices\":[{\"delta\":{\"content\":\"final\"}}]}";
        let (actual, state) = monitor_output(vec![Bytes::from_static(expected.as_bytes())]).await;
        assert_eq!(actual, expected);
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(200)
        );
    }

    #[tokio::test]
    async fn malformed_sse_is_not_forwarded_and_records_failure() {
        let (actual, state) =
            monitor_output(vec![Bytes::from_static(b"data: {not-json}\n\n")]).await;
        assert!(actual.is_empty());
        let requests = state.recent_requests.lock().unwrap();
        assert_eq!(requests.front().map(|record| record.status), Some(502));
    }

    #[tokio::test]
    async fn unterminated_frame_is_bounded() {
        let mut state = test_state(0, 10.0);
        state.max_sse_frame_bytes = 16;
        let upstream = stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
            b"data: this frame never terminates",
        ))]);
        let monitor = StreamMonitor::new(
            upstream,
            "request".to_string(),
            "user".to_string(),
            "gpt-4o".to_string(),
            free_pricing(),
            0,
            0.0,
            0.0,
            10.0,
            state.clone(),
            false,
            None,
        );
        let output = monitor.collect::<Vec<_>>().await;
        assert!(output.is_empty());
        assert_eq!(
            state
                .recent_requests
                .lock()
                .unwrap()
                .front()
                .map(|record| record.status),
            Some(502)
        );
    }

    #[test]
    fn independently_tokenized_stream_deltas_can_differ_from_complete_output() {
        let bpe =
            tiktoken_rs::bpe_for_model("gpt-4o").expect("gpt-4o tokenizer should be available");
        let complete = bpe.encode_with_special_tokens("hello world").len();
        let divided = ["hel", "lo ", "wor", "ld"]
            .iter()
            .map(|part| bpe.encode_with_special_tokens(part).len())
            .sum::<usize>();
        assert_ne!(complete, divided);
        assert!(divided > complete);
    }
}
