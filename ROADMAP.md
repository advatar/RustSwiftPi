# Roadmap (translation parity)

This workspace is built as a sequence of incremental drops that converge toward parity with `badlogic/pi-mono`.

## Drops

- [x] Drop 1: agent loop + OpenAI adapter + core tools + minimal CLI
- [ ] Drop 2: `pi-ai` parity (provider registry, model catalog/discovery, cost/token accounting, streaming)
- [ ] Drop 3: `pi-tui` parity (component API + differential renderer + input system)
- [ ] Drop 4: `pi-coding-agent` parity (session JSONL + events, RPC mode, SDK surface, extensions/skills/packages loader)
- [ ] Drop 5: `pi-web-ui` parity (web components / wasm bindings as needed)
- [ ] Drop 6: `pi-mom` parity (Slack socket mode bot + delegation to coding agent)
- [ ] Drop 7: `pi-pods` parity (vLLM deployment CLI + providers)
- [ ] Drop 8: `proxy` parity (CORS proxy / HTTP utilities)

Each drop adds tests first, then implementation.
