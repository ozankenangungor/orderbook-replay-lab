# orderbook-replay-lab-rs

This repo is my Rust playground for order book replay and execution simulation.
Main goal is deterministic behavior with clean, modular components so I can iterate fast on strategy and engine ideas.

## What is inside
- L2 market event model with integer ticks/lots.
- JSONL codec, plus optional binary records for faster replay.
- Deterministic replay reader.
- Single-symbol L2 order book with best bid/ask tracking.
- Simple engine pipeline: strategy -> risk -> OMS -> simulated venue.
- Basic strategies (`noop`, `twap`, `mm`) and CLI flows.

## Quick start
Generate synthetic data:

```sh
cargo run -p cli -- gen --output /tmp/lob.log --symbol BTC-USD --events 1000
```

Replay and print stats:

```sh
cargo run -p cli -- replay --input /tmp/lob.log --symbol BTC-USD
```

Run end-to-end simulation:

```sh
cargo run -p cli -- simulate --input /tmp/lob.log --symbol BTC-USD --strategy mm
```

Use `--format bin` for binary replay/generation when building with `bin` feature.

## Project layout
- `core`: domain types and invariants.
- `codec`: JSON/binary encode-decode.
- `replay`: event readers.
- `orderbook`: in-memory book.
- `metrics`: latency and throughput helpers.
- `strategy-api`: strategy interface and context.
- `strategies`: sample strategy implementations.
- `risk`: policy chain.
- `oms`: order state machine.
- `venue` / `venue-sim`: venue interface and simulation.
- `engine`: orchestration.
- `cli`: commands (`gen`, `replay`, `simulate`).

## Notes
- Built for research and simulation, not for live trading.
- Determinism and observability are prioritized over exchange-specific complexity.

License: MIT
