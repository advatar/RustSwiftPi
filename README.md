# pi-mono-rust

A Rust workspace translation of the TypeScript monorepo `badlogic/pi-mono`, implemented using a **contracts → tests → implementation** flow and a **hexagonal ports/adapters** layout.

This repository is intentionally **library-first**:
- `contracts/`: public types + errors
- `core/`: domain logic + port traits (no I/O)
- `adapters/*`: I/O implementations (providers, filesystem tools, shell, UIs, etc.)
- `app/`: composition root(s)

## What is implemented (Drop 1)

✅ Agent runtime with tool loop (`core/`)  
✅ OpenAI chat-completions provider adapter (`adapters/adapter_openai/`)  
✅ Coding tools: `read`, `write`, `edit` (`adapters/adapter_fs/`)  
✅ `bash` tool (`adapters/adapter_shell/`)  
✅ CLI composition root (`app/`)  
✅ Swift Package wrapper (`PiSwift/`)  

🚧 Stubs (compile, but not feature-complete): slack (`adapter_slack`), web-ui (`adapter_web_ui`), pods (`adapter_pods`), proxy (`adapter_proxy`), tui library (`adapter_tui`).

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
