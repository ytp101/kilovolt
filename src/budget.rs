use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Instant;
use futures_util::stream::Stream;
use axum::body::Bytes;
use tiktoken_rs::bpe_for_model;
use tracing::{error, info, warn};
use crate::config::AppState;

// Model-specific pricing configuration struct
#[derive(Clone, Copy)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
}

/// Dynamic pricing matrix loader based on OpenAI model definitions.
pub fn get_model_pricing(model: &str) -> ModelPricing {
    match model {
        m if m.starts_with("gpt-4o-mini") => ModelPricing {
            input_cost_per_token: 0.15 / 1_000_000.0,
            output_cost_per_token: 0.60 / 1_000_000.0,
        },
        m if m.starts_with("gpt-4o") => ModelPricing {
            input_cost_per_token: 5.00 / 1_000_000.0,
            output_cost_per_token: 15.00 / 1_000_000.0,
        },
        m if m.starts_with("gpt-4") => ModelPricing {
            input_cost_per_token: 30.00 / 1_000_000.0,
            output_cost_per_token: 60.00 / 1_000_000.0,
        },
        m if m.starts_with("gpt-3.5-turbo") => ModelPricing {
            input_cost_per_token: 0.50 / 1_000_000.0,
            output_cost_per_token: 1.50 / 1_000_000.0,
        },
        m if m.starts_with("gemini-1.5-flash") => ModelPricing {
            input_cost_per_token: 0.075 / 1_000_000.0,
            output_cost_per_token: 0.30 / 1_000_000.0,
        },
        m if m.starts_with("gemini-1.5-pro") => ModelPricing {
            input_cost_per_token: 1.25 / 1_000_000.0,
            output_cost_per_token: 5.00 / 1_000_000.0,
        },
        m if m.starts_with("gemini-") => ModelPricing {
            input_cost_per_token: 0.075 / 1_000_000.0,
            output_cost_per_token: 0.30 / 1_000_000.0,
        },
        _ => ModelPricing {
            // Default fallback to gpt-4o pricing
            input_cost_per_token: 5.00 / 1_000_000.0,
            output_cost_per_token: 15.00 / 1_000_000.0,
        },
    }
}

/// Custom Stream wrapper designed to monitor chunk metrics, track spend aggregation,
/// and handle client-side disconnections without loading the stream into memory.
pub struct StreamMonitor<S> {
    pub inner: S,
    pub start_time: Instant,
    pub bytes_written: usize,
    pub chunks_written: usize,
    pub logged: bool,
    pub request_id: String,
    pub user_id: String,
    pub spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
    pub model: String,
    pub pricing: ModelPricing,
    pub prompt_tokens: usize,
    pub prompt_cost: f64,
    pub bpe: Option<tiktoken_rs::CoreBPE>,
    pub accumulated_text: String,
    pub total_spend: f64,
    pub budget_limit: f64,
    pub output_tokens_count: usize,
    pub state: AppState,
    pub is_gemini: bool,
    pub pending_output: Vec<u8>,
    pub sent_done: bool,
}

impl<S> StreamMonitor<S> {
    pub fn new(
        inner: S,
        request_id: String,
        user_id: String,
        spend_tracker: Arc<RwLock<HashMap<String, f64>>>,
        model: String,
        pricing: ModelPricing,
        prompt_tokens: usize,
        prompt_cost: f64,
        total_spend: f64,
        budget_limit: f64,
        state: AppState,
        is_gemini: bool,
    ) -> Self {
        let bpe = bpe_for_model(&model)
            .ok()
            .or_else(|| bpe_for_model("gpt-4o").ok())
            .cloned();

        Self {
            inner,
            start_time: Instant::now(),
            bytes_written: 0,
            chunks_written: 0,
            logged: false,
            request_id,
            user_id,
            spend_tracker,
            model,
            pricing,
            prompt_tokens,
            prompt_cost,
            bpe,
            accumulated_text: String::new(),
            total_spend,
            budget_limit,
            output_tokens_count: 0,
            state,
            is_gemini,
            pending_output: Vec::new(),
            sent_done: false,
        }
    }

    /// Logs the final summary of the stream upon completion or cutoff.
    pub fn log_final_status(&mut self, is_cutoff: bool, outcome: &str) {
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
        let status_code = if is_cutoff { 429 } else { 200 };
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

        // 1. Guard check: Trip the circuit breaker if budget is already breached before polling
        if this.total_spend >= this.budget_limit {
            warn!(
                user_id = %this.user_id,
                total_spend = %this.total_spend,
                budget_limit = %this.budget_limit,
                "Bankruptcy Shield tripped PRE-POLL. Severing connection."
            );
            this.log_final_status(true, "tripped mid-stream");
            return Poll::Ready(None); // Terminate response stream early
        }

        // If we have translated pending outputs from a previous chunk, drain it first
        if !this.pending_output.is_empty() {
            let bytes = Bytes::from(std::mem::take(&mut this.pending_output));
            return Poll::Ready(Some(Ok(bytes)));
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.bytes_written += bytes.len();
                    this.chunks_written += 1;

                    let text_chunk = String::from_utf8_lossy(&bytes);
                    this.accumulated_text.push_str(&text_chunk);

                    let mut parsed_any = false;
                    while let Some(pos) = this.accumulated_text.find('\n') {
                        let line = this.accumulated_text[..pos].trim().to_string();
                        this.accumulated_text = this.accumulated_text[pos + 1..].to_string();

                        if line.starts_with("data:") {
                            let data_content = line["data:".len()..].trim();
                            
                            if this.is_gemini {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_content) {
                                    let mut text_extracted = String::new();
                                    let mut finish_reason_opt: Option<String> = None;
                                    
                                    if let Some(candidates) = val.get("candidates").and_then(|c| c.as_array()) {
                                        if let Some(candidate) = candidates.first() {
                                            if let Some(content) = candidate.get("content") {
                                                if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                                                    for part in parts {
                                                        if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                                            text_extracted.push_str(txt);
                                                        }
                                                    }
                                                }
                                            }
                                            if let Some(fr) = candidate.get("finishReason").and_then(|f| f.as_str()) {
                                                finish_reason_opt = Some(fr.to_lowercase());
                                            }
                                        }
                                    }
                                    
                                    if !text_extracted.is_empty() || finish_reason_opt.is_some() {
                                        let new_tokens = if let Some(bpe) = &this.bpe {
                                            bpe.encode_with_special_tokens(&text_extracted).len()
                                        } else {
                                            (text_extracted.len() as f64 / 4.0).ceil() as usize
                                        };
                                        
                                        this.output_tokens_count += new_tokens;
                                        this.state.total_tokens_consumed.fetch_add(new_tokens, Ordering::Relaxed);
                                        
                                        let incremental_cost = new_tokens as f64 * this.pricing.output_cost_per_token;
                                        let new_spend = {
                                            let mut map = this.spend_tracker.write().unwrap();
                                            let entry = map.entry(this.user_id.clone()).or_insert(0.0);
                                            *entry += incremental_cost;
                                            *entry
                                        };
                                        this.total_spend = new_spend;
                                        
                                        let finish_reason_val = match finish_reason_opt.as_deref() {
                                            Some("stop") => serde_json::Value::String("stop".to_string()),
                                            Some("completed") => serde_json::Value::String("stop".to_string()),
                                            Some(other) => serde_json::Value::String(other.to_string()),
                                            None => serde_json::Value::Null,
                                        };
                                        
                                        let openai_chunk = serde_json::json!({
                                            "id": format!("chatcmpl-{}", this.request_id),
                                            "object": "chat.completion.chunk",
                                            "created": 1677825464,
                                            "model": this.model,
                                            "choices": [{
                                                "index": 0,
                                                "delta": {
                                                    "content": text_extracted
                                                },
                                                "logprobs": null,
                                                "finish_reason": finish_reason_val
                                            }]
                                        });
                                        
                                        let frame = format!("data: {}\n\n", openai_chunk);
                                        this.pending_output.extend_from_slice(frame.as_bytes());
                                        parsed_any = true;
                                    }
                                }
                            } else {
                                if data_content == "[DONE]" {
                                    this.pending_output.extend_from_slice(b"data: [DONE]\n\n");
                                    parsed_any = true;
                                    continue;
                                }
                                
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_content) {
                                    if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                        if let Some(choice) = choices.first() {
                                            if let Some(delta) = choice.get("delta") {
                                                if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                                    if let Some(bpe) = &this.bpe {
                                                        let tokens = bpe.encode_with_special_tokens(content);
                                                        let new_tokens = tokens.len();
                                                        this.output_tokens_count += new_tokens;
                                                        this.state.total_tokens_consumed.fetch_add(new_tokens, Ordering::Relaxed);
                                                        
                                                        let incremental_cost = new_tokens as f64 * this.pricing.output_cost_per_token;
                                                        let new_spend = {
                                                            let mut map = this.spend_tracker.write().unwrap();
                                                            let entry = map.entry(this.user_id.clone()).or_insert(0.0);
                                                            *entry += incremental_cost;
                                                            *entry
                                                        };
                                                        this.total_spend = new_spend;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                
                                let frame = format!("{}\n\n", line);
                                this.pending_output.extend_from_slice(frame.as_bytes());
                                parsed_any = true;
                            }
                        }
                    }

                    // Post-chunk evaluation: Sever socket if threshold is breached
                    if this.total_spend >= this.budget_limit {
                        warn!(
                            user_id = %this.user_id,
                            total_spend = %this.total_spend,
                            budget_limit = %this.budget_limit,
                            "Bankruptcy Shield tripped MID-STREAM. Severing connection."
                        );
                        this.log_final_status(true, "tripped mid-stream");
                        return Poll::Ready(None); // Drop connection immediately
                    }

                    if parsed_any && !this.pending_output.is_empty() {
                        let bytes = Bytes::from(std::mem::take(&mut this.pending_output));
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Poll::Ready(Some(Err(err))) => {
                    let duration = this.start_time.elapsed();
                    error!(
                        "Stream failed after {:.2?} (chunks: {}, bytes: {}): {:?}",
                        duration,
                        this.chunks_written,
                        this.bytes_written,
                        err
                    );
                    this.log_final_status(false, "stream failed");
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Ready(None) => {
                    if this.is_gemini && !this.sent_done {
                        this.sent_done = true;
                        this.log_final_status(false, "completed successfully");
                        return Poll::Ready(Some(Ok(Bytes::from("data: [DONE]\n\n"))));
                    } else {
                        this.log_final_status(false, "completed successfully");
                        return Poll::Ready(None);
                    }
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
            self.log_final_status(false, "aborted by client");
        }
    }
}
