[Download pi-mono-rust — Drop 2 (pi-ai primitives + streaming)](sandbox:/mnt/data/pi-mono-rust-drop2.zip?_chatgptios_conversationID=6987329f-201c-8392-953c-00ba9fc09c0b&_chatgptios_messageID=0e7323cd-e7fb-4602-a044-1503aa80c8a9)

What’s in this drop:
- **Model catalog / discovery mechanism**: `pi_core::ModelCatalog` (with a small built-in starter catalog + extension support)
- **Provider registry**: `pi_core::ProviderHub` (maps `provider_id -> Arc<dyn AiProvider>`)
- **Unified multi-provider surface**: `pi_core::AiClient` with:
  - `model(provider, id)`
  - `complete(model, context, ...)`
  - `stream(model, context, ...)`
- **Cost/token accounting types** in `contracts/`:
  - `TokenCost` (USD per 1M tokens) + `estimate_usd()`
  - `TokenUsage` extended with cache fields
  - `ChatResponse.cost: Option<CostBreakdown>`
- **Streaming support**:
  - `pi_core::{ChatProviderStream, ChatStream}`
  - `pi_contracts::ChatStreamEvent`
  - `pi_adapter_openai` implements streaming via **OpenAI Chat Completions SSE** and emits normalized deltas

Unpack:
```bash
unzip pi-mono-rust-drop2.zip
cd pi-mono-rust
```
