# Risk Pipeline (Research/Simulation Oriented)

## Policy Flow
Intents --> Policy Chain --> Allow | Reject | Transform --> OMS

## Semantics
- Allow: intent passes unchanged.
- Reject: intent is dropped with reason.
- Transform: intent is rewritten (e.g., size clamp, price band).

## Default Policies (Initial List)
- Max order size per symbol
- Price band vs mid or last trade
- Position limits (gross/net)
- Rate limits (intent/sec)
- Self-trade prevention (simulation mode)
- Kill switch (global halt)
