#![forbid(unsafe_code)]
//! Public types and errors for the pi-mono-rust workspace.

use serde::{Deserialize, Serialize};
use std::{fmt, num::NonZeroUsize};
use thiserror::Error;
use uuid::Uuid;

/// Workspace-wide error type.
///
/// This is intentionally typed but compact; adapters should wrap provider-specific details into
/// `PiError::Provider` or `PiError::Adapter`.
#[derive(Debug, Error)]
pub enum PiError {
    /// Invalid input / invariant violation.
    #[error("invalid: {0}")]
    Invalid(String),

    /// Tool execution failed.
    #[error("tool: {0}")]
    Tool(String),

    /// LLM/provider failure.
    #[error("provider: {0}")]
    Provider(String),

    /// Adapter-specific failure (slack/pods/web/etc).
    #[error("adapter: {0}")]
    Adapter(String),

    /// I/O.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// HTTP client.
    #[error("http: {0}")]
    Http(String),

    /// Timeout.
    #[error("timeout: {0}")]
    Timeout(String),
}

/// A validated, non-empty string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Creates a new [`NonEmptyString`].
    pub fn new(s: impl Into<String>) -> Result<Self, PiError> {
        let s = s.into();
        if s.trim().is_empty() {
            return Err(PiError::Invalid("empty string".into()));
        }
        Ok(Self(s))
    }

    /// Returns `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes into `String`.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A model identifier (e.g. `gpt-4.1-mini`).
pub type ModelId = NonEmptyString;

/// Provider identifier (e.g. `openai`, `anthropic`, `ollama`).
pub type ProviderId = NonEmptyString;

/// Tool name.
pub type ToolName = NonEmptyString;

/// Tool call id.
pub type ToolCallId = NonEmptyString;

/// Supported provider API families.
///
/// pi-ai's design centers around "the four APIs":
/// - OpenAI Chat Completions
/// - OpenAI Responses
/// - Anthropic Messages
/// - Google Generative AI
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiKind {
    OpenAiCompletions,
    OpenAiResponses,
    AnthropicMessages,
    GoogleGenerativeAi,
}

/// Input modalities a model accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputModality {
    Text,
    Image,
    Audio,
}

/// Per-1M-token costs in USD.
///
/// All fields are expressed as USD / 1,000,000 tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenCost {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

impl TokenCost {
    pub const fn free() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        }
    }

    /// Best-effort estimate of USD cost for the given usage.
    pub fn estimate_usd(&self, usage: &TokenUsage) -> CostBreakdown {
        let per_m = 1_000_000.0;
        let input = (usage.prompt_tokens as f64 / per_m) * self.input;
        let output = (usage.completion_tokens as f64 / per_m) * self.output;
        let cache_read = (usage.cache_read_tokens as f64 / per_m) * self.cache_read;
        let cache_write = (usage.cache_write_tokens as f64 / per_m) * self.cache_write;
        CostBreakdown {
            input,
            output,
            cache_read,
            cache_write,
            total: input + output + cache_read + cache_write,
            currency: Currency::Usd,
        }
    }
}

/// Currency code for cost tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    Usd,
}

/// Cost breakdown for a request.
///
/// This is a best-effort estimate; providers differ wildly in token accounting and cache reporting.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
    pub currency: Currency,
}

/// Standardized model descriptor.
///
/// This mirrors the rough shape used in `@mariozechner/pi-ai`: a stable identifier, a provider,
/// an API family, capability flags, and cost metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: ModelId,
    pub name: String,
    pub api: ApiKind,
    pub provider: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<InputModality>,
    #[serde(default)]
    pub cost: TokenCost,
    #[serde(default)]
    pub context_window: u32,
    #[serde(default)]
    pub max_tokens: u32,
}

impl Model {
    /// Creates a new model descriptor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: ProviderId,
        id: ModelId,
        api: ApiKind,
        name: impl Into<String>,
        cost: TokenCost,
        context_window: u32,
        max_tokens: u32,
        input: Vec<InputModality>,
        reasoning: bool,
        base_url: Option<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            api,
            provider,
            base_url,
            reasoning,
            input,
            cost,
            context_window,
            max_tokens,
        }
    }
}

/// A portable, serializable conversation context.
///
/// In Rust we already model messages as an ADT (`ChatMessage`), so a `Context` is essentially a
/// vector of messages. Keeping it as a struct makes room for future knobs while preserving
/// backward compatibility.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Context {
    pub messages: Vec<ChatMessage>,
}

/// Message role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt / instructions.
    System,
    /// Human/user message.
    User,
    /// Assistant message.
    Assistant,
    /// Tool result message.
    Tool,
}

/// A tool call made by the assistant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-generated id for correlating results.
    pub id: ToolCallId,
    /// Tool function name.
    pub name: ToolName,
    /// JSON arguments object.
    pub arguments: serde_json::Value,
}

/// Tool (function) specification sent to providers.
///
/// `parameters` is JSON Schema for a single arguments object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Domain chat message (stored in transcripts).
///
/// Uses an ADT to make illegal states unrepresentable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    /// System message.
    System { content: String },
    /// User message.
    User { content: String },
    /// Assistant message, optionally with tool calls.
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    /// Tool result message.
    Tool {
        tool_call_id: ToolCallId,
        content: String,
    },
}

impl ChatMessage {
    /// Creates a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    /// Creates a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    /// Creates an assistant message.
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls,
        }
    }

    /// Creates a tool result message.
    pub fn tool(tool_call_id: ToolCallId, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id,
            content: content.into(),
        }
    }

    /// Returns role.
    pub fn role(&self) -> Role {
        match self {
            ChatMessage::System { .. } => Role::System,
            ChatMessage::User { .. } => Role::User,
            ChatMessage::Assistant { .. } => Role::Assistant,
            ChatMessage::Tool { .. } => Role::Tool,
        }
    }
}

/// Token usage info (if the provider returns it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    pub const fn new(prompt_tokens: u64, completion_tokens: u64, total_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }
}

/// Chat request passed to a provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: ModelId,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// Chat response returned by a provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub assistant: ChatMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Optional best-effort cost estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostBreakdown>,
}

/// Streaming error category (normalized).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamErrorReason {
    Aborted,
    Provider,
    Decode,
}

/// Normalized streaming events.
///
/// The intention is "UI-friendly deltas" + a final `Done` event.
/// Providers may omit usage/cost information in streams; consumers should treat these as best-effort.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    TextDelta {
        delta: String,
    },
    ToolCallDelta {
        id: ToolCallId,
        name: ToolName,
        arguments_delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parsed_arguments: Option<serde_json::Value>,
    },
    Usage {
        usage: TokenUsage,
    },
    Done,
    Error {
        reason: StreamErrorReason,
        message: String,
    },
}

/// A session identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// New random session id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// A bounded, 1-based line range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: NonZeroUsize,
    pub end: NonZeroUsize,
}

impl LineRange {
    /// Creates a validated line range (`start <= end`).
    pub fn new(start: NonZeroUsize, end: NonZeroUsize) -> Result<Self, PiError> {
        if start.get() > end.get() {
            return Err(PiError::Invalid("line range start > end".into()));
        }
        Ok(Self { start, end })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cost_estimate_is_additive() {
        let c = TokenCost {
            input: 2.0,       // $2 / 1M input
            output: 10.0,     // $10 / 1M output
            cache_read: 1.0,  // $1 / 1M
            cache_write: 5.0, // $5 / 1M
        };
        let usage = TokenUsage {
            prompt_tokens: 500_000,
            completion_tokens: 100_000,
            total_tokens: 600_000,
            cache_read_tokens: 200_000,
            cache_write_tokens: 50_000,
        };
        let cost = c.estimate_usd(&usage);
        // 0.5*2 + 0.1*10 + 0.2*1 + 0.05*5 = 1 + 1 + 0.2 + 0.25 = 2.45
        assert!((cost.total - 2.45).abs() < 1e-9);
        assert_eq!(cost.currency, Currency::Usd);
    }
}
