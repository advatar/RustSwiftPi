Here’s **Drop 1** of the Rust translation as a **Cargo workspace** following your `ARCH-Rust.md` layout (contracts → core → adapters → app).

[**Download the repository (zip)**](sandbox:/mnt/data/pi-mono-rust.zip?_chatgptios_conversationID=6987329f-201c-8392-953c-00ba9fc09c0b&_chatgptios_messageID=4f01ed84-1e0d-4ca4-869a-12c3bafee675)

## What’s in Drop 1

Implemented (usable end-to-end):
- **`contracts/` (`pi_contracts`)**: public types + typed errors
- **`core/` (`pi_core`)**: agent runtime + ports (hexagonal), **no I/O**
- **`adapters/adapter_openai/` (`pi_adapter_openai`)**: OpenAI **chat-completions** provider adapter (`OPENAI_API_KEY`)
- **`adapters/adapter_fs/` (`pi_adapter_fs`)**: tools `read`, `write`, `edit` + JSON transcript session store
- **`adapters/adapter_shell/` (`pi_adapter_shell`)**: tool `bash` (`sh -lc ...`)
- **`app/` (`pi_app`)**: composition-root CLI (interactive + one-shot)

Stubs (compile, but intentionally not implemented yet):
- `adapters/adapter_tui/` (pi-tui)
- `adapters/adapter_web_ui/` (pi-web-ui)
- `adapters/adapter_slack/` (pi-mom)
- `adapters/adapter_pods/` (pi-pods)
- `adapters/adapter_proxy/` (proxy)

A checklist is included in **`ROADMAP.md`** inside the repo.

## Run it

```bash
export OPENAI_API_KEY="..."
cd pi-mono-rust
cargo run -p pi_app -- -p "Say hi"
cargo run -p pi_app
```

Interactive commands:
- `/exit` or `/quit`
- `/reset`

## “One by one” going forward

To proceed cleanly, treat each next step as a new “drop” that upgrades one of the remaining stubs to full parity.  
When you’re ready, just reply **`next`** and I’ll produce **Drop 2** (the next package in the roadmap, with tests first, then implementation).
