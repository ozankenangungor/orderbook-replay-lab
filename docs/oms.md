# OMS (Order Management System)

## Order Lifecycle State Machine
New -> Accepted -> Working -> PartiallyFilled -> Filled
New -> Rejected
Working/PartiallyFilled -> Canceled
Working/PartiallyFilled -> Expired

## Invariants
- Remaining qty >= 0
- Filled qty + remaining qty == original qty
- No transitions from terminal states (Filled/Canceled/Rejected/Expired)
- Only one active order per unique client_id

## Idempotency Rules
- Duplicate reports with same (order_id, seq) are ignored.
- Out-of-order reports are rejected or buffered (policy defined by engine).
- Cancel on unknown order_id is a no-op with a warning.
- Modify on terminal order is rejected.
