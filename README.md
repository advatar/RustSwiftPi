# pi-mono-rust

A Rust workspace translation of the TypeScript monorepo `badlogic/pi-mono`, implemented using a **contracts → tests → implementation** flow and a **hexagonal ports/adapters** layout.

This repository is intentionally **library-first**:
- `contracts/`: public types + errors
- `core/`: domain logic + port traits (no I/O)
- `adapters/*`: I/O implementations (providers, filesystem tools, shell, UIs, etc.)
- `app/`: composition root(s)

## What is implemented

✅ Drop 1: agent runtime + OpenAI adapter + core coding tools + minimal CLI + Swift Package wrapper  
✅ Drop 2: pi-ai primitives: model catalog, provider registry, cost/token accounting types, streaming surface (OpenAI chat-completions SSE)

🚧 Stubs (compile, but not feature-complete): slack (`adapter_slack`), web-ui (`adapter_web_ui`), pods (`adapter_pods`), proxy (`adapter_proxy`), tui library (`adapter_tui`).

### pi-ai style usage (library)

```rust
use pi_contracts::{ChatMessage, Context};
use pi_core::{AiClient, ModelCatalog, ProviderHub};
use pi_adapter_openai::OpenAiChatProvider;
use std::sync::Arc;

# async fn demo() -> Result<(), pi_contracts::PiError> {
let models = ModelCatalog::builtin();
let mut providers = ProviderHub::new();
providers.insert(pi_contracts::NonEmptyString::new("openai")?, Arc::new(OpenAiChatProvider::from_env()?) );
let ai = AiClient::new(models, providers);

let model = ai.model("openai", "gpt-4o-mini")?;
let ctx = Context { messages: vec![ChatMessage::user("Hello")] };

let resp = ai.complete(&model, &ctx, vec![], None, None).await?;
println!("{resp:?}");
# Ok(())
# }
```

## Run (macOS)

```bash
export OPENAI_API_KEY="..."
cargo run -p pi_app -- -p "Say hi"
cargo run -p pi_app
```

Alternatively, create a `.env` file (see `.env.example`) with `OPENAI_API_KEY=...`.

Interactive mode commands:
- `/exit` or `/quit`
- `/reset`

## Swift Package (PiSwift)

`PiSwift/` is a SwiftPM wrapper that embeds the Rust agent runtime via a small C FFI surface (`pi_swift_ffi`).

Build the universal (arm64 + x86_64) Rust static library and copy it into the Swift package:

```bash
PiSwift/Scripts/build-rust-macos-universal.sh
```

This generates `PiSwift/Sources/PiRustFFI/lib/libpi_swift_ffi.a` (the `lib/` directory is generated and gitignored).

Then build/test the Swift package:

```bash
cd PiSwift
swift test
```

Usage:

```swift
import PiSwift

let client = PiSwiftClient(config: PiSwiftConfig(
    apiKey: "OPENAI_API_KEY", // or omit to use env / .env
    model: "gpt-4o-mini"
))

let reply = try await client.run(prompt: "Say hi")
print(reply)
```

## Mandatory gates

```bash
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
```

## License

MIT (mirrors upstream license intent).
