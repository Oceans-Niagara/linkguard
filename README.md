# LinkGuard

**Deterministic physical-layer & framing analyzer for transport code.**

LinkGuard is an offline-first Rust CLI, MCP server, and seeded packet proxy developed by [Oceans Niagara, LLC](https://oceansniagara.com), which analyzes transport code against physical-layer limits without LLM dependencies.

- **Website & Docs:** https://oceansniagara.com/linkguard/
- **Interactive WASM Sandbox:** https://oceansniagara.com/linkguard/sandbox.html

## Install & Run Locally

```bash
cargo install --git https://github.com
linkguard analyze src/ --profile lora-sf12 --hz 10
linkguard check --config budgets.toml --format sarif
linkguard proxy --listen 127.0.0.1:9000 --target 127.0.0.1:9001 --profile ble-1m --seed 42
linkguard mcp
```

## Licensing (v0.3)

For detailed information on free, pro, and team licensing tiers verified via offline Ed25519-signed keys, please visit the [Oceans Niagara Website](https://oceansniagara.com).

---

© 2026 Oceans Niagara, LLC • Questions: [connect@oceansniagara.com](mailto:connect@oceansniagara.com)
