#![forbid(unsafe_code)]

//! OpenAI chat-completions adapter.
//!
//! Implements [`pi_core::ChatProvider`] using `POST /v1/chat/completions`.
//!
//! Environment variables:
//! - `OPENAI_API_KEY` (required)
//! - `OPENAI_BASE_URL` (optional, default `https://api.openai.com`)

use async_trait::async_trait;
use pi_contracts::{ChatMessage, ChatRequest, ChatResponse, NonEmptyString, PiError, TokenUsage, ToolCall, ToolSpec};
use pi_core::ChatProvider;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::time::Duration;
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
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| PiError::Invalid("OPENAI_API_KEY not set".into()))?;
        let base_url = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
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
        h.insert(AUTHORIZATION, HeaderValue::from_str(&v).map_err(|e| PiError::Http(e.to_string()))?);
        Ok(h)
    }
}

#[async_trait]
impl ChatProvider for OpenAiChatProvider {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, PiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let body = OpenAiChatRequest::try_from(req)?;
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

        let out: OpenAiChatResponse = resp.json().await.map_err(|e| PiError::Http(e.to_string()))?;
        out.try_into()
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
}

impl TryFrom<ChatRequest> for OpenAiChatRequest {
    type Error = PiError;

    fn try_from(req: ChatRequest) -> Result<Self, Self::Error> {
        let tools: Vec<OpenAiTool> = req.tools.into_iter().map(OpenAiTool::from).collect();
        Ok(Self {
            model: req.model.into_string(),
            messages: req.messages.into_iter().map(OpenAiMessage::from).collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tool_choice: (!tools.is_empty()).then_some("auto".into()),
            tools,
        })
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
            ChatMessage::Assistant { content, tool_calls } => Self {
                role: "assistant".into(),
                content: (!content.is_empty()).then_some(content),
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls.into_iter().map(OpenAiToolCall::from).collect()),
                tool_call_id: None,
            },
            ChatMessage::Tool { tool_call_id, content } => Self {
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
            return Err(PiError::Provider(format!("openai: expected assistant role, got {}", m.role)));
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
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_tool_calls() {
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
    }
}
