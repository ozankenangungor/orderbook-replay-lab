# Engine Architecture (Research/Simulation Oriented)

## Goal
Define an engine-first, strategy-agnostic architecture that prioritizes
determinism, observability, and modularity for research and simulation. No live
trading or exchange credentials are in scope.

## Module Boundaries
- Engine: coordinates event flow, time, and deterministic scheduling.
- MarketData: normalized market events (L2 deltas/snapshots, trades).
- Strategy: produces Intents from inputs and context snapshots.
- Risk: validates/transforms Intents before they reach OMS.
- OMS: owns order state machine and idempotent handling of reports.
- ExecutionVenue: port for simulated/paper execution (no live implementation).
- Metrics/Telemetry: latency, throughput, state counters.

## Event Flow (ASCII)
MarketData --> Engine --> Strategy --> Intents --> Risk --> OMS --> ExecutionVenue
                   ^                         |                 |
                   |                         v                 v
                 Clock/Time           Rejections/Transforms   Reports/Fills
                   |                         |                 |
                   +----------- State Snapshot/Telemetry <-----+

## Determinism Notes
- Single-threaded event loop for replay; no wall-clock dependence.
- All randomness must be seeded and captured.
- Inputs (market events, reports) fully define outputs.
- State transitions are idempotent and ordered by sequence number.
