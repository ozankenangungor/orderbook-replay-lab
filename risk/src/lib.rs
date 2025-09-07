use std::cell::RefCell;

use lob_core::Side;
use strategy_api::ContextSnapshot;
use trading_types::Intent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskAction {
    Allow(Intent),
    Reject { reason: String },
    Transform(Intent),
}

pub trait RiskPolicy {
    fn evaluate(&self, ctx: &ContextSnapshot, intent: &Intent) -> RiskAction;
}

pub struct RiskEngine {
    policies: Vec<Box<dyn RiskPolicy>>,
}

impl RiskEngine {
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    pub fn with_policy(mut self, policy: impl RiskPolicy + 'static) -> Self {
        self.policies.push(Box::new(policy));
        self
    }

    pub fn evaluate(&self, ctx: &ContextSnapshot, intent: &Intent) -> RiskAction {
        let mut current = intent.clone();
        for policy in &self.policies {
            match policy.evaluate(ctx, &current) {
                RiskAction::Allow(next) | RiskAction::Transform(next) => {
                    current = next;
                }
                RiskAction::Reject { reason } => return RiskAction::Reject { reason },
            }
        }
        RiskAction::Allow(current)
    }
}

impl Default for RiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MaxPositionPolicy {
    limit_lots: i64,
}

impl MaxPositionPolicy {
    pub fn new(limit_lots: i64) -> Self {
        Self { limit_lots }
    }
}

impl RiskPolicy for MaxPositionPolicy {
    fn evaluate(&self, ctx: &ContextSnapshot, intent: &Intent) -> RiskAction {
        let limit = self.limit_lots.abs();
        if limit == 0 {
            return RiskAction::Reject {
                reason: "position limit is zero".to_string(),
            };
        }

        let (side, qty) = match intent {
            Intent::PlaceLimit { side, qty, .. } => (*side, qty.lots()),
            _ => return RiskAction::Allow(intent.clone()),
        };

        let projected = if side == Side::Bid {
            ctx.position_lots + qty
        } else {
            ctx.position_lots - qty
        };

        if projected.abs() > limit {
            return RiskAction::Reject {
                reason: "max position exceeded".to_string(),
            };
        }

        RiskAction::Allow(intent.clone())
    }
}

pub struct PriceBandPolicy {
    max_distance_ticks: i64,
}

impl PriceBandPolicy {
    pub fn new(max_distance_ticks: i64) -> Self {
        Self { max_distance_ticks }
    }
}

impl RiskPolicy for PriceBandPolicy {
    fn evaluate(&self, ctx: &ContextSnapshot, intent: &Intent) -> RiskAction {
        let max_distance = self.max_distance_ticks.abs();
        let price = match intent {
            Intent::PlaceLimit { price, .. } => *price,
            _ => return RiskAction::Allow(intent.clone()),
        };

        let Some(mid) = ctx.mid_price else {
            return RiskAction::Allow(intent.clone());
        };
        let distance = (price.ticks() - mid.ticks()).abs();
        if distance > max_distance {
            return RiskAction::Reject {
                reason: "price outside band".to_string(),
            };
        }

        RiskAction::Allow(intent.clone())
    }
}

pub struct RateLimitPolicy {
    max_per_sec: u64,
    window_bucket: RefCell<u64>,
    count: RefCell<u64>,
}

impl RateLimitPolicy {
    pub fn new(max_per_sec: u64) -> Self {
        Self {
            max_per_sec,
            window_bucket: RefCell::new(u64::MAX),
            count: RefCell::new(0),
        }
    }
}

impl RiskPolicy for RateLimitPolicy {
    fn evaluate(&self, ctx: &ContextSnapshot, intent: &Intent) -> RiskAction {
        if !is_order_intent(intent) {
            return RiskAction::Allow(intent.clone());
        }

        if self.max_per_sec == 0 {
            return RiskAction::Reject {
                reason: "rate limit exceeded".to_string(),
            };
        }

        let mut window_bucket = self.window_bucket.borrow_mut();
        let mut count = self.count.borrow_mut();
        let bucket = ctx.ts_ns / 1_000_000_000;
        if *window_bucket != bucket {
            *window_bucket = bucket;
            *count = 0;
        }

        if *count + 1 > self.max_per_sec {
            return RiskAction::Reject {
                reason: "rate limit exceeded".to_string(),
            };
        }

        *count += 1;
        RiskAction::Allow(intent.clone())
    }
}

fn is_order_intent(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::PlaceLimit { .. } | Intent::Cancel { .. } | Intent::Replace { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{Price, Qty, Side, Symbol};
    use trading_types::TimeInForce;

    fn ctx_with_mid(ts_ns: u64, position_lots: i64) -> ContextSnapshot {
        let symbol = Symbol::new("BTC-USD").unwrap();
        ContextSnapshot::new(
            ts_ns,
            symbol,
            Some((Price::new(100).unwrap(), Qty::new(1).unwrap())),
            Some((Price::new(102).unwrap(), Qty::new(1).unwrap())),
            position_lots,
            0,
        )
    }

    #[test]
    fn max_position_rejects_excess() {
        let policy = MaxPositionPolicy::new(10);
        let ctx = ctx_with_mid(1, 9);
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
            side: Side::Bid,
            price: Price::new(101).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Reject { .. }
        ));
    }

    #[test]
    fn max_position_allows_reducing() {
        let policy = MaxPositionPolicy::new(10);
        let ctx = ctx_with_mid(1, 9);
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
            side: Side::Ask,
            price: Price::new(101).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Allow(_)
        ));
    }

    #[test]
    fn price_band_rejects_far_price() {
        let policy = PriceBandPolicy::new(3);
        let ctx = ctx_with_mid(1, 0);
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
            side: Side::Bid,
            price: Price::new(200).unwrap(),
            qty: Qty::new(1).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Reject { .. }
        ));
    }

    #[test]
    fn rate_limit_enforced_per_second() {
        let policy = RateLimitPolicy::new(2);
        let ctx = ctx_with_mid(1, 0);
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
            side: Side::Bid,
            price: Price::new(101).unwrap(),
            qty: Qty::new(1).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };

        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Allow(_)
        ));
        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Allow(_)
        ));
        assert!(matches!(
            policy.evaluate(&ctx, &intent),
            RiskAction::Reject { .. }
        ));

        let ctx_next = ctx_with_mid(1_000_000_000, 0);
        assert!(matches!(
            policy.evaluate(&ctx_next, &intent),
            RiskAction::Allow(_)
        ));
    }
}
