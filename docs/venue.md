# Execution Venue Port

## Interface Sketch
- submit(intent) -> ack/reject
- cancel(order_id) -> ack/reject
- replace(order_id, new_params) -> ack/reject
- stream reports (fills, cancels, rejections)

## Report Types
- Accepted / Rejected
- PartialFill / Fill
- Canceled / Expired

## Mode Notes
- Simulation: deterministic fills based on model/rules.
- Paper: uses delayed or mocked execution, no real orders.
- Live: explicitly out of scope in this phase (no exchange keys).
