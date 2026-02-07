# PiSwift

SwiftPM wrapper around the Rust agent runtime in this repo, exposed via a small C FFI surface.

## Build

1. Build the Rust static library and copy it into the Swift package:

```bash
cd /Volumes/XCodeX/pi
PiSwift/Scripts/build-rust-macos-universal.sh
```

2. Build/test the Swift package:

```bash
cd PiSwift
swift test
```

## Usage

```swift
import PiSwift

let client = PiSwiftClient(config: PiSwiftConfig(
    apiKey: "OPENAI_API_KEY",
    model: "gpt-4o-mini"
))

let reply = try await client.run(prompt: "Say hi")
print(reply)
```

Notes:
- If `apiKey` is nil/empty, the Rust side will fall back to `OPENAI_API_KEY` from the environment.
- The Rust FFI will attempt to load a `.env` file once (searching current directory and parents).

