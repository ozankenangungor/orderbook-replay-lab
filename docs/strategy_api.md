# Strategy API (Research/Simulation Oriented)

## Interface Sketch
Inputs:
- MarketEvent stream (L2 snapshot/delta, trade, clock ticks)
- Context snapshot (positions, open orders, balances, last prices)
- Optional config parameters (static during a run)

Outputs:
- Intents (Place, Cancel, Modify)
- Optional telemetry annotations (tags for metrics)

## Context Snapshot Rules
- Snapshot is read-only for the strategy.
- Snapshot is point-in-time and consistent with the engine event order.
- Strategy must not cache mutable references to engine state.
- Snapshot data is derived from OMS + positions + latest market state.

## Intent Model
Place:
- symbol, side, price, qty, time_in_force, client_id

Cancel:
- client_id or venue_order_id

Modify:
- client_id or venue_order_id + updated price/qty

Notes:
- Intents are requests, not guarantees.
- Risk may reject/transform intents before OMS.
- OMS provides idempotent handling of retries and duplicates.
