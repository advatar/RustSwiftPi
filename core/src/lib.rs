#![forbid(unsafe_code)]

//! Domain logic + port traits.
//!
//! `pi_core` MUST NOT do I/O. All I/O lives in `adapters/*`.

use async_trait::async_trait;
use pi_contracts::{ChatMessage, ChatRequest, ChatResponse, ModelId, PiError, SessionId, ToolCall, ToolName, ToolSpec};
use serde_json::Value as Json;
use std::{path::PathBuf, sync::Arc};

/// A transcript of messages.
pub type Transcript = Vec<ChatMessage>;

/// Execution context passed to tools.
#[derive(Clone, Debug)]
pub struct ToolContext {
    /// Current working directory (driving adapter decides).
    pub cwd: PathBuf,
}

/// Tool execution result.
#[derive(Clone, Debug)]
pub struct ToolResult {
    pub content: String,
    pub details: Option<Json>,
}

impl ToolResult {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            details: None,
        }
    }
}

/// Outbound port: chat completion provider.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, PiError>;
}

/// Outbound port: tool execution.
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, args: Json, ctx: ToolContext) -> Result<ToolResult, PiError>;
}

/// Outbound port: persist sessions.
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn load(&self, id: SessionId) -> Result<Option<Transcript>, PiError>;
    async fn save(&self, id: SessionId, transcript: &Transcript) -> Result<(), PiError>;
}

/// Tool set (registry + specs).
#[derive(Clone, Default)]
pub struct ToolSet {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolSet {
    pub fn new(tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Self {
        Self {
            tools: tools.into_iter().collect(),
        }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|t| t.spec().name.as_str() == name.as_str())
            .cloned()
    }
}

/// Agent configuration.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub model: ModelId,
    pub system_prompt: Option<String>,
    pub max_steps: usize,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl AgentConfig {
    pub fn minimal(model: ModelId) -> Self {
        Self {
            model,
            system_prompt: None,
            max_steps: 32,
            temperature: None,
            max_tokens: None,
        }
    }
}

/// Agent runtime.
///
/// Drives a [`ChatProvider`] and executes tool calls via a [`ToolSet`].
pub struct Agent<P: ChatProvider> {
    provider: P,
    tools: ToolSet,
    cfg: AgentConfig,
}

impl<P: ChatProvider> Agent<P> {
    pub fn new(provider: P, tools: ToolSet, cfg: AgentConfig) -> Self {
        Self { provider, tools, cfg }
    }

    /// Runs one user input to quiescence (until the model stops issuing tool calls or `max_steps` is hit).
    pub async fn run_to_end(&self, transcript: &mut Transcript, user_input: &str, ctx: ToolContext) -> Result<(), PiError> {
        if transcript.is_empty() {
            if let Some(sys) = &self.cfg.system_prompt {
                transcript.push(ChatMessage::system(sys));
            }
        }

        transcript.push(ChatMessage::user(user_input));

        for _ in 0..self.cfg.max_steps {
            let req = ChatRequest {
                model: self.cfg.model.clone(),
                messages: transcript.clone(),
                tools: self.tools.specs(),
                temperature: self.cfg.temperature,
                max_tokens: self.cfg.max_tokens,
            };

            let resp = self.provider.chat(req).await?;
            let assistant = match &resp.assistant {
                ChatMessage::Assistant { .. } => resp.assistant,
                _ => return Err(PiError::Provider("provider returned non-assistant message".into())),
            };

            let tool_calls = match &assistant {
                ChatMessage::Assistant { tool_calls, .. } => tool_calls.clone(),
                _ => vec![],
            };

            transcript.push(assistant);

            if tool_calls.is_empty() {
                return Ok(());
            }

            for call in tool_calls {
                self.exec_tool_call(transcript, call, ctx.clone()).await?;
            }
        }

        Err(PiError::Provider("max_steps reached".into()))
    }

    async fn exec_tool_call(&self, transcript: &mut Transcript, call: ToolCall, ctx: ToolContext) -> Result<(), PiError> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PiError::Tool(format!("unknown tool: {}", call.name)))?;

        let out = tool.execute(call.arguments, ctx).await?;
        transcript.push(ChatMessage::tool(call.id, out.content));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_contracts::{NonEmptyString, ToolCall, ToolSpec};
    use std::sync::Mutex;

    #[derive(Clone)]
    struct StubProvider {
        // queued assistant messages
        q: Arc<Mutex<Vec<ChatMessage>>>,
    }

    #[async_trait]
    impl ChatProvider for StubProvider {
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, PiError> {
            let msg = self.q.lock().unwrap().remove(0);
            Ok(ChatResponse { assistant: msg, usage: None })
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: NonEmptyString::new("echo").unwrap(),
                description: "echo".into(),
                parameters: serde_json::json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
            }
        }

        async fn execute(&self, args: Json, _ctx: ToolContext) -> Result<ToolResult, PiError> {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(ToolResult::text(text))
        }
    }

    #[tokio::test]
    async fn agent_tool_loop_orders_messages() {
        let call_id = NonEmptyString::new("call_1").unwrap();
        let tool_name = NonEmptyString::new("echo").unwrap();
        let tool_call = ToolCall {
            id: call_id.clone(),
            name: tool_name,
            arguments: serde_json::json!({"text":"hi"}),
        };

        let assistant_1 = ChatMessage::assistant("", vec![tool_call]);
        let assistant_2 = ChatMessage::assistant("done", vec![]);

        let provider = StubProvider { q: Arc::new(Mutex::new(vec![assistant_1, assistant_2])) };
        let tools = ToolSet::new([Arc::new(EchoTool) as Arc<dyn Tool>]);

        let cfg = AgentConfig { model: NonEmptyString::new("gpt-test").unwrap(), system_prompt: None, max_steps: 8, temperature: None, max_tokens: None };

        let agent = Agent::new(provider, tools, cfg);
        let mut tr: Transcript = vec![];
        agent
            .run_to_end(&mut tr, "go", ToolContext { cwd: PathBuf::from(".") })
            .await
            .unwrap();

        // Expected: user, assistant(toolcall), tool(result), assistant(final)
        assert!(matches!(tr[0], ChatMessage::User { .. }));
        assert!(matches!(tr[1], ChatMessage::Assistant { .. }));
        assert!(matches!(tr[2], ChatMessage::Tool { .. }));
        assert!(matches!(tr[3], ChatMessage::Assistant { .. }));

        match &tr[2] {
            ChatMessage::Tool { tool_call_id, content } => {
                assert_eq!(tool_call_id, &call_id);
                assert_eq!(content, "hi");
            }
            _ => panic!("expected tool message"),
        }
    }
}
