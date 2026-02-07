# ARCH-Rust.md (project rules)

- Workspace: `contracts/` + `core/` + `adapters/*` + optional `app/`
- Hexagonal dependency flow:
  - `core` contains domain models + pure logic + port traits
  - `adapters/*` implement ports and perform I/O
  - `app` wires everything
- Contracts are types + errors + port traits; keep public API minimal.
- Testing is the spec: every behavior needs tests before implementation.
- Prefer maximal abstraction with minimal redundancy; macros/codegen allowed.
- Mandatory gates:
  - `cargo test --all`
  - `cargo clippy --all-targets --all-features -- -D warnings`
