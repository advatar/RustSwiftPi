#![forbid(unsafe_code)]

//! Domain logic + port traits.
//!
//! `pi_core` MUST NOT do I/O. All I/O lives in `adapters/*`.

use async_trait::async_trait;
use futures::{channel::mpsc, future::BoxFuture, stream::Stream};
use pi_contracts::{
    ChatMessage, ChatRequest, ChatResponse, ChatStreamEvent, Context as AiContext, Model, ModelId,
    PiError, ProviderId, SessionId, ToolCall, ToolName, ToolSpec,
};
use serde_json::Value as Json;
use std::{
    collections::HashMap,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

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

/// Outbound port: streaming chat completion provider.
#[async_trait]
pub trait ChatProviderStream: Send + Sync {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, PiError>;
}

/// Convenience super-trait for providers that support both non-streaming and streaming.
pub trait AiProvider: ChatProvider + ChatProviderStream {}
impl<T: ChatProvider + ChatProviderStream> AiProvider for T {}

/// A stream of normalized events plus a retrievable final [`ChatResponse`].
///
/// Pattern: consume deltas for UX, then call `.result().await` for the final message (possibly partial).
pub struct ChatStream {
    events: mpsc::Receiver<ChatStreamEvent>,
    result: Option<BoxFuture<'static, Result<ChatResponse, PiError>>>,
}

impl ChatStream {
    pub fn new(
        events: mpsc::Receiver<ChatStreamEvent>,
        result: BoxFuture<'static, Result<ChatResponse, PiError>>,
    ) -> Self {
        Self {
            events,
            result: Some(result),
        }
    }

    /// Returns the final response. May be called after the stream is fully consumed.
    pub async fn result(&mut self) -> Result<ChatResponse, PiError> {
        let fut = self
            .result
            .take()
            .ok_or_else(|| PiError::Invalid("stream result already taken".into()))?;
        fut.await
    }

    /// Maps the final response (e.g. to inject cost tracking) without touching the event stream.
    pub fn map_result<F>(mut self, f: F) -> Self
    where
        F: FnOnce(ChatResponse) -> ChatResponse + Send + 'static,
    {
        if let Some(fut) = self.result.take() {
            self.result = Some(Box::pin(async move { fut.await.map(f) }));
        }
        self
    }
}

impl Stream for ChatStream {
    type Item = ChatStreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.events).poll_next(cx)
    }
}

/// Tool execution.
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
        Self {
            provider,
            tools,
            cfg,
        }
    }

    /// Runs one user input to quiescence (until the model stops issuing tool calls or `max_steps` is hit).
    pub async fn run_to_end(
        &self,
        transcript: &mut Transcript,
        user_input: &str,
        ctx: ToolContext,
    ) -> Result<(), PiError> {
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
                _ => {
                    return Err(PiError::Provider(
                        "provider returned non-assistant message".into(),
                    ))
                }
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

    async fn exec_tool_call(
        &self,
        transcript: &mut Transcript,
        call: ToolCall,
        ctx: ToolContext,
    ) -> Result<(), PiError> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| PiError::Tool(format!("unknown tool: {}", call.name)))?;

        let out = tool.execute(call.arguments, ctx).await?;
        transcript.push(ChatMessage::tool(call.id, out.content));
        Ok(())
    }
}

/// Pure model catalog (built-in + extension).
#[derive(Clone, Default)]
pub struct ModelCatalog {
    models: Vec<Model>,
}

impl ModelCatalog {
    pub fn new(models: impl IntoIterator<Item = Model>) -> Self {
        Self {
            models: models.into_iter().collect(),
        }
    }

    /// Small built-in catalog for bootstrapping.
    ///
    /// Full parity with upstream's generated catalog is a later drop; this provides the *mechanism*
    /// for model discovery (list + lookup + extension).
    pub fn builtin() -> Self {
        use pi_contracts::{ApiKind, InputModality, NonEmptyString, TokenCost};

        let m = |provider: &str,
                 id: &str,
                 api: ApiKind,
                 name: &str,
                 cost: TokenCost,
                 ctx: u32,
                 max: u32,
                 input: Vec<InputModality>,
                 reasoning: bool,
                 base: Option<&str>| {
            Model::new(
                NonEmptyString::new(provider).unwrap(),
                NonEmptyString::new(id).unwrap(),
                api,
                name,
                cost,
                ctx,
                max,
                input,
                reasoning,
                base.map(|s| s.to_string()),
            )
        };

        Self::new([
            m(
                "openai",
                "gpt-4o-mini",
                ApiKind::OpenAiCompletions,
                "GPT-4o mini",
                TokenCost::free(),
                128_000,
                16_000,
                vec![InputModality::Text],
                false,
                None,
            ),
            m(
                "openai",
                "gpt-4o",
                ApiKind::OpenAiCompletions,
                "GPT-4o",
                TokenCost::free(),
                128_000,
                16_000,
                vec![InputModality::Text, InputModality::Image],
                false,
                None,
            ),
            m(
                "openai",
                "gpt-5.1-codex",
                ApiKind::OpenAiCompletions,
                "GPT-5.1 Codex",
                TokenCost::free(),
                200_000,
                32_000,
                vec![InputModality::Text],
                true,
                None,
            ),
            m(
                "anthropic",
                "claude-sonnet-4-5",
                ApiKind::AnthropicMessages,
                "Claude Sonnet 4.5",
                TokenCost::free(),
                200_000,
                32_000,
                vec![InputModality::Text, InputModality::Image],
                true,
                None,
            ),
            m(
                "google",
                "gemini-2.5-flash",
                ApiKind::GoogleGenerativeAi,
                "Gemini 2.5 Flash",
                TokenCost::free(),
                1_000_000,
                32_000,
                vec![InputModality::Text, InputModality::Image],
                true,
                None,
            ),
            m(
                "ollama",
                "llama-3.1-8b",
                ApiKind::OpenAiCompletions,
                "Llama 3.1 8B (Ollama)",
                TokenCost::free(),
                128_000,
                32_000,
                vec![InputModality::Text],
                false,
                Some("http://localhost:11434/v1"),
            ),
        ])
    }

    pub fn all(&self) -> impl Iterator<Item = &Model> {
        self.models.iter()
    }

    pub fn extend(&mut self, models: impl IntoIterator<Item = Model>) {
        self.models.extend(models);
    }

    pub fn find(&self, provider: &str, id: &str) -> Option<Model> {
        self.models
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .cloned()
    }

    pub fn get(&self, provider: &str, id: &str) -> Result<Model, PiError> {
        self.find(provider, id)
            .ok_or_else(|| PiError::Invalid(format!("unknown model {provider}:{id}")))
    }
}

/// Provider registry (ports/adapters live outside; this is the lookup layer).
#[derive(Clone, Default)]
pub struct ProviderHub {
    providers: HashMap<ProviderId, Arc<dyn AiProvider>>,
}

impl ProviderHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, provider: ProviderId, client: Arc<dyn AiProvider>) {
        self.providers.insert(provider, client);
    }

    pub fn get(&self, provider: &ProviderId) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(provider).cloned()
    }
}

/// Unified multi-provider API (pi-ai style), minus provider-specific I/O.
#[derive(Clone)]
pub struct AiClient {
    models: ModelCatalog,
    providers: ProviderHub,
}

impl AiClient {
    pub fn new(models: ModelCatalog, providers: ProviderHub) -> Self {
        Self { models, providers }
    }

    pub fn model(&self, provider: &str, id: &str) -> Result<Model, PiError> {
        self.models.get(provider, id)
    }

    fn provider(&self, provider: &ProviderId) -> Result<Arc<dyn AiProvider>, PiError> {
        self.providers
            .get(provider)
            .ok_or_else(|| PiError::Invalid(format!("no provider registered: {provider}")))
    }

    pub async fn complete(
        &self,
        model: &Model,
        ctx: &AiContext,
        tools: Vec<ToolSpec>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<ChatResponse, PiError> {
        let p = self.provider(&model.provider)?;
        let mut resp = p
            .chat(ChatRequest {
                model: model.id.clone(),
                messages: ctx.messages.clone(),
                tools,
                temperature,
                max_tokens,
            })
            .await?;

        if resp.cost.is_none() {
            if let Some(u) = resp.usage.as_ref() {
                resp.cost = Some(model.cost.estimate_usd(u));
            }
        }
        Ok(resp)
    }

    pub async fn stream(
        &self,
        model: &Model,
        ctx: &AiContext,
        tools: Vec<ToolSpec>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> Result<ChatStream, PiError> {
        let p = self.provider(&model.provider)?;
        let cost = model.cost;
        Ok(p.chat_stream(ChatRequest {
            model: model.id.clone(),
            messages: ctx.messages.clone(),
            tools,
            temperature,
            max_tokens,
        })
        .await?
        .map_result(move |mut resp| {
            if resp.cost.is_none() {
                if let Some(u) = resp.usage.as_ref() {
                    resp.cost = Some(cost.estimate_usd(u));
                }
            }
            resp
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{
        channel::{mpsc, oneshot},
        stream::StreamExt,
        SinkExt,
    };
    use pi_contracts::{NonEmptyString, TokenCost, TokenUsage};
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
            Ok(ChatResponse {
                assistant: msg,
                usage: None,
                cost: None,
            })
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
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
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

        let provider = StubProvider {
            q: Arc::new(Mutex::new(vec![assistant_1, assistant_2])),
        };
        let tools = ToolSet::new([Arc::new(EchoTool) as Arc<dyn Tool>]);

        let cfg = AgentConfig {
            model: NonEmptyString::new("gpt-test").unwrap(),
            system_prompt: None,
            max_steps: 8,
            temperature: None,
            max_tokens: None,
        };

        let agent = Agent::new(provider, tools, cfg);
        let mut tr: Transcript = vec![];
        agent
            .run_to_end(
                &mut tr,
                "go",
                ToolContext {
                    cwd: PathBuf::from("."),
                },
            )
            .await
            .unwrap();

        // Expected: user, assistant(toolcall), tool(result), assistant(final)
        assert!(matches!(tr[0], ChatMessage::User { .. }));
        assert!(matches!(tr[1], ChatMessage::Assistant { .. }));
        assert!(matches!(tr[2], ChatMessage::Tool { .. }));
        assert!(matches!(tr[3], ChatMessage::Assistant { .. }));

        match &tr[2] {
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                assert_eq!(tool_call_id, &call_id);
                assert_eq!(content, "hi");
            }
            _ => panic!("expected tool message"),
        }
    }

    #[derive(Clone)]
    struct StubStreamProvider;

    #[async_trait]
    impl ChatProvider for StubStreamProvider {
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, PiError> {
            Ok(ChatResponse {
                assistant: ChatMessage::assistant("hi", vec![]),
                usage: Some(TokenUsage::new(1_000_000, 1_000_000, 2_000_000)),
                cost: None,
            })
        }
    }

    #[async_trait]
    impl ChatProviderStream for StubStreamProvider {
        async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, PiError> {
            let (mut tx, rx) = mpsc::channel(8);
            let (res_tx, res_rx) = oneshot::channel::<Result<ChatResponse, PiError>>();
            tokio::spawn(async move {
                let _ = tx
                    .send(ChatStreamEvent::TextDelta { delta: "h".into() })
                    .await;
                let _ = tx
                    .send(ChatStreamEvent::TextDelta { delta: "i".into() })
                    .await;
                let _ = tx.send(ChatStreamEvent::Done).await;
                let _ = res_tx.send(Ok(ChatResponse {
                    assistant: ChatMessage::assistant("hi", vec![]),
                    usage: Some(TokenUsage::new(1_000_000, 1_000_000, 2_000_000)),
                    cost: None,
                }));
            });

            Ok(ChatStream::new(
                rx,
                Box::pin(async move {
                    res_rx
                        .await
                        .map_err(|_| PiError::Provider("stream dropped".into()))?
                }),
            ))
        }
    }

    #[tokio::test]
    async fn ai_client_injects_cost_on_complete_and_stream_result() {
        use pi_contracts::{ApiKind, InputModality, Model};

        let model = Model::new(
            NonEmptyString::new("stub").unwrap(),
            NonEmptyString::new("m").unwrap(),
            ApiKind::OpenAiCompletions,
            "stub",
            TokenCost {
                input: 1.0,
                output: 1.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            1,
            1,
            vec![InputModality::Text],
            false,
            None,
        );

        let models = ModelCatalog::new([model.clone()]);
        let mut providers = ProviderHub::new();
        providers.insert(
            NonEmptyString::new("stub").unwrap(),
            Arc::new(StubStreamProvider) as Arc<dyn AiProvider>,
        );

        let ai = AiClient::new(models, providers);
        let ctx = AiContext {
            messages: vec![ChatMessage::user("yo")],
        };

        let r = ai.complete(&model, &ctx, vec![], None, None).await.unwrap();
        assert!(r.cost.is_some());
        assert!((r.cost.unwrap().total - 2.0).abs() < 1e-9);

        let mut s = ai.stream(&model, &ctx, vec![], None, None).await.unwrap();
        let mut buf = String::new();
        while let Some(ev) = s.next().await {
            if let ChatStreamEvent::TextDelta { delta } = ev {
                buf.push_str(&delta);
            }
        }
        assert_eq!(buf, "hi");
        let r2 = s.result().await.unwrap();
        assert!(r2.cost.is_some());
        assert!((r2.cost.unwrap().total - 2.0).abs() < 1e-9);
    }
}
