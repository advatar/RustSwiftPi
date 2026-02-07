#![forbid(unsafe_code)]

//! OpenAI chat-completions adapter.
//!
//! Implements [`pi_core::ChatProvider`] and [`pi_core::ChatProviderStream`] using
//! `POST /v1/chat/completions`.
//!
//! Environment variables:
//! - `OPENAI_API_KEY` (required)
//! - `OPENAI_BASE_URL` (optional, default `https://api.openai.com`)

use async_trait::async_trait;
use futures::{
    channel::{mpsc, oneshot},
    future::BoxFuture,
    SinkExt, StreamExt,
};
use pi_contracts::{
    ChatMessage, ChatRequest, ChatResponse, ChatStreamEvent, NonEmptyString, PiError, TokenUsage,
    ToolCall, ToolSpec,
};
use pi_core::{ChatProvider, ChatProviderStream, ChatStream};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::{collections::BTreeMap, time::Duration};
use tokio::task::JoinHandle;
use tracing::debug;

#[derive(Clone)]
pub struct OpenAiChatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl OpenAiChatProvider {
    pub fn from_env() -> Result<Self, PiError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| PiError::Invalid("OPENAI_API_KEY not set".into()))?;
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
        Ok(Self::new(base_url, api_key))
    }

    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder().build().expect("reqwest client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            timeout: Duration::from_secs(120),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn headers(&self) -> Result<HeaderMap, PiError> {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let v = format!("Bearer {}", self.api_key);
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&v).map_err(|e| PiError::Http(e.to_string()))?,
        );
        Ok(h)
    }
}

#[async_trait]
impl ChatProvider for OpenAiChatProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, PiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let body = OpenAiChatRequest::non_stream(req)?;
        debug!("openai request model={}", body.model);

        let resp = self
            .client
            .post(url)
            .headers(self.headers()?)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| PiError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(PiError::Provider(format!("openai {}: {}", status, txt)));
        }

        let out: OpenAiChatResponse = resp
            .json()
            .await
            .map_err(|e| PiError::Http(e.to_string()))?;
        out.try_into()
    }
}

#[async_trait]
impl ChatProviderStream for OpenAiChatProvider {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, PiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let body = OpenAiChatRequest::stream(req)?;
        debug!("openai stream request model={}", body.model);

        let resp = self
            .client
            .post(url)
            .headers(self.headers()?)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| PiError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(PiError::Provider(format!("openai {}: {}", status, txt)));
        }

        let (mut tx, rx) = mpsc::channel::<ChatStreamEvent>(128);
        let (res_tx, res_rx) = oneshot::channel::<Result<ChatResponse, PiError>>();

        let handle: JoinHandle<()> = tokio::spawn(async move {
            let mut asm = StreamAssembler::default();

            let mut buf = String::new();
            let mut bytes = resp.bytes_stream();

            let mut done = false;
            while let Some(chunk) = bytes.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx
                            .send(ChatStreamEvent::Error {
                                reason: pi_contracts::StreamErrorReason::Decode,
                                message: e.to_string(),
                            })
                            .await;
                        let _ = res_tx.send(Err(PiError::Http(e.to_string())));
                        return;
                    }
                };

                match std::str::from_utf8(&bytes) {
                    Ok(s) => buf.push_str(s),
                    Err(e) => {
                        let _ = tx
                            .send(ChatStreamEvent::Error {
                                reason: pi_contracts::StreamErrorReason::Decode,
                                message: e.to_string(),
                            })
                            .await;
                        let _ = res_tx
                            .send(Err(PiError::Provider(format!("openai: invalid utf8: {e}"))));
                        return;
                    }
                }

                while let Some(data) = next_sse_data(&mut buf) {
                    let data = data.trim();
                    if data == "[DONE]" {
                        done = true;
                        break;
                    }

                    let chunk: OpenAiStreamChunk = match serde_json::from_str(data) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx
                                .send(ChatStreamEvent::Error {
                                    reason: pi_contracts::StreamErrorReason::Decode,
                                    message: e.to_string(),
                                })
                                .await;
                            let _ = res_tx.send(Err(PiError::Provider(format!(
                                "openai: invalid chunk json: {e}"
                            ))));
                            return;
                        }
                    };

                    let events = match asm.apply(chunk) {
                        Ok(evs) => evs,
                        Err(e) => {
                            let _ = tx
                                .send(ChatStreamEvent::Error {
                                    reason: pi_contracts::StreamErrorReason::Decode,
                                    message: e.to_string(),
                                })
                                .await;
                            let _ = res_tx.send(Err(e));
                            return;
                        }
                    };

                    for ev in events {
                        // If receiver dropped, stop.
                        if tx.send(ev).await.is_err() {
                            let _ = res_tx.send(Err(PiError::Provider("stream dropped".into())));
                            return;
                        }
                    }
                }

                if done {
                    break;
                }
            }

            let _ = tx.send(ChatStreamEvent::Done).await;
            let _ = res_tx.send(asm.finish());
        });

        let result: BoxFuture<'static, Result<ChatResponse, PiError>> = Box::pin(async move {
            // best-effort join; ignore join error (panic).
            let _ = handle.await;
            res_rx
                .await
                .map_err(|_| PiError::Provider("stream dropped".into()))?
        });

        Ok(ChatStream::new(rx, result))
    }
}

#[derive(Debug, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
}

#[derive(Debug, Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

impl OpenAiChatRequest {
    fn base(req: ChatRequest) -> Result<Self, PiError> {
        let tools: Vec<OpenAiTool> = req.tools.into_iter().map(OpenAiTool::from).collect();
        Ok(Self {
            model: req.model.into_string(),
            messages: req.messages.into_iter().map(OpenAiMessage::from).collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tool_choice: (!tools.is_empty()).then_some("auto".into()),
            tools,
            stream: None,
            stream_options: None,
        })
    }

    fn non_stream(req: ChatRequest) -> Result<Self, PiError> {
        Self::base(req)
    }

    fn stream(req: ChatRequest) -> Result<Self, PiError> {
        let mut r = Self::base(req)?;
        r.stream = Some(true);
        r.stream_options = Some(OpenAiStreamOptions {
            include_usage: true,
        });
        Ok(r)
    }
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    kind: String,
    function: OpenAiToolFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiToolFunction {
    name: String,
    description: String,
    parameters: Json,
}

impl From<ToolSpec> for OpenAiTool {
    fn from(t: ToolSpec) -> Self {
        Self {
            kind: "function".into(),
            function: OpenAiToolFunction {
                name: t.name.into_string(),
                description: t.description,
                parameters: t.parameters,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl From<ChatMessage> for OpenAiMessage {
    fn from(m: ChatMessage) -> Self {
        match m {
            ChatMessage::System { content } => Self {
                role: "system".into(),
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage::User { content } => Self {
                role: "user".into(),
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => Self {
                role: "assistant".into(),
                content: (!content.is_empty()).then_some(content),
                tool_calls: (!tool_calls.is_empty())
                    .then_some(tool_calls.into_iter().map(OpenAiToolCall::from).collect()),
                tool_call_id: None,
            },
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => Self {
                role: "tool".into(),
                content: Some(content),
                tool_calls: None,
                tool_call_id: Some(tool_call_id.into_string()),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: OpenAiFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

impl From<ToolCall> for OpenAiToolCall {
    fn from(tc: ToolCall) -> Self {
        Self {
            id: tc.id.into_string(),
            kind: "function".into(),
            function: OpenAiFunctionCall {
                name: tc.name.into_string(),
                arguments: tc.arguments.to_string(),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageOut,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessageOut {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

impl TryFrom<OpenAiChatResponse> for ChatResponse {
    type Error = PiError;

    fn try_from(r: OpenAiChatResponse) -> Result<Self, Self::Error> {
        let m = r
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| PiError::Provider("openai: empty choices".into()))?
            .message;

        if m.role != "assistant" {
            return Err(PiError::Provider(format!(
                "openai: expected assistant role, got {}",
                m.role
            )));
        }

        let tool_calls = m
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                if tc.kind != "function" {
                    return Err(PiError::Provider("openai: non-function tool call".into()));
                }
                let args: Json = serde_json::from_str(&tc.function.arguments)
                    .map_err(|e| PiError::Provider(format!("openai: invalid tool args: {e}")))?;

                Ok(ToolCall {
                    id: NonEmptyString::new(tc.id)?,
                    name: NonEmptyString::new(tc.function.name)?,
                    arguments: args,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ChatResponse {
            assistant: ChatMessage::assistant(m.content.unwrap_or_default(), tool_calls),
            usage: r.usage.map(|u| TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            }),
            cost: None,
        })
    }
}

// ---------- streaming ----------

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    delta: OpenAiStreamDelta,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionCallDelta>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiFunctionCallDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default)]
struct ToolAcc {
    id: Option<String>,
    name: Option<String>,
    args: String,
}

#[derive(Debug, Default)]
struct StreamAssembler {
    content: String,
    tools: BTreeMap<usize, ToolAcc>,
    usage: Option<TokenUsage>,
}

impl StreamAssembler {
    fn apply(&mut self, chunk: OpenAiStreamChunk) -> Result<Vec<ChatStreamEvent>, PiError> {
        let mut out = Vec::new();

        if let Some(u) = chunk.usage {
            self.usage = Some(TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            });
            out.push(ChatStreamEvent::Usage {
                usage: self.usage.clone().unwrap(),
            });
        }

        let choice = match chunk.choices.into_iter().next() {
            Some(c) => c,
            None => return Ok(out),
        };

        if let Some(s) = choice.delta.content {
            if !s.is_empty() {
                self.content.push_str(&s);
                out.push(ChatStreamEvent::TextDelta { delta: s });
            }
        }

        if let Some(tcs) = choice.delta.tool_calls {
            for tc in tcs {
                if let Some(kind) = &tc.kind {
                    if kind != "function" {
                        return Err(PiError::Provider(
                            "openai: non-function tool call delta".into(),
                        ));
                    }
                }

                let acc = self.tools.entry(tc.index).or_default();
                if let Some(id) = tc.id {
                    acc.id = Some(id);
                }
                if let Some(func) = tc.function {
                    if let Some(name) = func.name {
                        acc.name = Some(name);
                    }
                    if let Some(args_delta) = func.arguments {
                        acc.args.push_str(&args_delta);
                        let (id, name) = match (&acc.id, &acc.name) {
                            (Some(i), Some(n)) => (
                                NonEmptyString::new(i.clone())?,
                                NonEmptyString::new(n.clone())?,
                            ),
                            _ => continue, // cannot emit typed event yet
                        };
                        let parsed = serde_json::from_str::<Json>(&acc.args).ok();
                        out.push(ChatStreamEvent::ToolCallDelta {
                            id,
                            name,
                            arguments_delta: args_delta,
                            parsed_arguments: parsed,
                        });
                    }
                }
            }
        }

        Ok(out)
    }

    fn finish(self) -> Result<ChatResponse, PiError> {
        let tool_calls = self
            .tools
            .into_values()
            .map(|acc| {
                let id = acc
                    .id
                    .ok_or_else(|| PiError::Provider("openai: tool call missing id".into()))?;
                let name = acc
                    .name
                    .ok_or_else(|| PiError::Provider("openai: tool call missing name".into()))?;
                let args: Json = serde_json::from_str(&acc.args)
                    .map_err(|e| PiError::Provider(format!("openai: invalid tool args: {e}")))?;
                Ok::<ToolCall, PiError>(ToolCall {
                    id: NonEmptyString::new(id)?,
                    name: NonEmptyString::new(name)?,
                    arguments: args,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ChatResponse {
            assistant: ChatMessage::assistant(self.content, tool_calls),
            usage: self.usage,
            cost: None,
        })
    }
}

/// Extracts one `data: ...` payload from an SSE buffer.
///
/// Keeps any incomplete trailing data in `buf`.
fn next_sse_data(buf: &mut String) -> Option<String> {
    // Find event boundary (blank line) and split after the delimiter.
    // The SSE spec uses a blank line to terminate an event. Allow both LF and CRLF.
    let crlf = buf.find("\r\n\r\n");
    let lf = buf.find("\n\n");
    let (i, delim_len) = match (crlf, lf) {
        (Some(a), Some(b)) => {
            if a < b {
                (a, 4)
            } else {
                (b, 2)
            }
        }
        (Some(a), None) => (a, 4),
        (None, Some(b)) => (b, 2),
        (None, None) => return None,
    };

    let head = buf[..i].to_string();
    *buf = buf[i + delim_len..].to_string();

    // Collect data lines (allow multi-line payloads).
    let mut data = String::new();
    for line in head.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.trim_start());
        }
    }
    (!data.is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_tool_calls_non_stream() {
        let json = serde_json::json!({
          "choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"echo","arguments":"{\"text\":\"hi\"}"}}]}}],
          "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}
        });

        let out: OpenAiChatResponse = serde_json::from_value(json).unwrap();
        let resp: ChatResponse = out.try_into().unwrap();
        match resp.assistant {
            ChatMessage::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name.as_str(), "echo");
            }
            _ => panic!("expected assistant"),
        }
        assert_eq!(resp.usage.unwrap().total_tokens, 3);
        assert!(resp.cost.is_none());
    }

    #[test]
    fn stream_assembler_accumulates_text_and_tool_args() {
        let mut asm = StreamAssembler::default();

        let c1: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices":[{"delta":{"content":"Hello "}, "finish_reason":null}]
        }))
        .unwrap();
        let e1 = asm.apply(c1).unwrap();
        assert_eq!(
            e1,
            vec![ChatStreamEvent::TextDelta {
                delta: "Hello ".into()
            }]
        );

        let c2: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"echo","arguments":"{\"text\":\"hi"}}]}}]
        }))
        .unwrap();
        let e2 = asm.apply(c2).unwrap();
        match &e2[0] {
            ChatStreamEvent::ToolCallDelta {
                id,
                name,
                arguments_delta,
                parsed_arguments,
            } => {
                assert_eq!(id.as_str(), "call_1");
                assert_eq!(name.as_str(), "echo");
                assert_eq!(arguments_delta, "{\"text\":\"hi");
                assert!(parsed_arguments.is_none());
            }
            _ => panic!("expected tool delta"),
        }

        let c3: OpenAiStreamChunk = serde_json::from_value(serde_json::json!({
            "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3},
            "choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"}"}}]}}]
        }))
        .unwrap();
        let e3 = asm.apply(c3).unwrap();
        assert!(matches!(e3[0], ChatStreamEvent::Usage { .. }));
        match &e3[1] {
            ChatStreamEvent::ToolCallDelta {
                parsed_arguments, ..
            } => {
                assert_eq!(parsed_arguments.as_ref().unwrap()["text"], "hi");
            }
            _ => panic!("expected tool delta"),
        }

        let resp = asm.finish().unwrap();
        assert_eq!(
            resp.assistant,
            ChatMessage::assistant(
                "Hello ",
                vec![ToolCall {
                    id: NonEmptyString::new("call_1").unwrap(),
                    name: NonEmptyString::new("echo").unwrap(),
                    arguments: serde_json::json!({"text":"hi"})
                }]
            )
        );
        assert_eq!(resp.usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn next_sse_data_splits_events() {
        let mut b = "data: 1\n\nnoise\ndata: 2\r\n\r\n".to_string();
        assert_eq!(next_sse_data(&mut b).unwrap(), "1");
        assert_eq!(next_sse_data(&mut b).unwrap(), "2");
    }
}
