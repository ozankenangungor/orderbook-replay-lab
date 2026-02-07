# orderbook-replay-lab-rs

A Rust workspace for low-latency limit order book (LOB) research.
It focuses on deterministic replay, a minimal L2 book, and latency/throughput metrics.
Designed to be readable and extendable for learning and prototyping.
Targets Rust 2021 on the stable toolchain.
Repository name: orderbook-replay-lab-rs.

## What it does
- Defines a strict market event model (L2 deltas) with integer ticks/lots.
- Encodes/decodes events as stable JSON lines for deterministic replay.
- Replays logs into a minimal L2 order book and reports metrics.
- Generates synthetic logs to avoid external data dependencies.

## Architecture
- `core`: domain types and invariants (Side, Symbol, Price, Qty, MarketEvent).
- `codec`: JSON-line format encoder/decoder for deterministic replay.
- `replay`: streaming reader for event logs (line-by-line).
- `orderbook`: minimal single-symbol L2 book with best bid/ask.
- `metrics`: latency histogram and throughput tracking.
- `cli`: `gen` and `replay` subcommands for end-to-end flow.

## Quickstart
Generate a synthetic log:
```sh
cargo run -p cli -- gen --output /tmp/lob.log --symbol BTC-USD --events 1000
```

Replay and print metrics:
```sh
cargo run -p cli -- replay --input /tmp/lob.log --symbol BTC-USD
```

Sample output:
```text
count=1000
throughput=12345.67 events/sec
latency=count=1000 p50=NN p95=NN p99=NN max=NN
best_bid=100123@4 best_ask=100124@2
```

## Performance methodology (brief)
- Measure per-event `orderbook.apply(...)` latency in nanoseconds.
- Track percentile stats (p50/p95/p99/max) via HDR histogram.
- Compute throughput over a sliding window and overall elapsed time.

## Roadmap
- Add L2 snapshot events and trade events.
- Extend order book with depth queries and crossed-book detection.
- Add binary log format for higher replay throughput.
- Expand CLI with replay summaries and export options.

## Educational disclaimer
This project is for learning and experimentation. It is not production trading
software and should not be used for live trading or financial decisions.

License: MIT.
