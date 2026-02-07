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

/// Tool name.
pub type ToolName = NonEmptyString;

/// Tool call id.
pub type ToolCallId = NonEmptyString;

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
    Tool { tool_call_id: ToolCallId, content: String },
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
