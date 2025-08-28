use lob_core::MarketEvent;
use strategy_api::{ContextSnapshot, Strategy};
use trading_types::{ExecutionReport, Intent};

pub struct NoopStrategy;

impl Strategy for NoopStrategy {
    fn on_market_event(&mut self, _ctx: &ContextSnapshot, _event: &MarketEvent) -> Vec<Intent> {
        Vec::new()
    }

    fn on_timer(&mut self, _ctx: &ContextSnapshot) -> Vec<Intent> {
        Vec::new()
    }

    fn on_execution_report(
        &mut self,
        _ctx: &ContextSnapshot,
        _report: &ExecutionReport,
    ) -> Vec<Intent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{Price, Qty, Symbol};

    #[test]
    fn noop_strategy_returns_empty_intents() {
        let mut strategy = NoopStrategy;
        let symbol = Symbol::new("TEST-USD").unwrap();
        let ctx = ContextSnapshot::new(
            1,
            symbol.clone(),
            Some((Price::new(100).unwrap(), Qty::new(1).unwrap())),
            Some((Price::new(101).unwrap(), Qty::new(1).unwrap())),
            0,
            0,
        );
        let event = MarketEvent::L2Delta {
            ts_ns: 1,
            symbol,
            updates: vec![],
        };

        assert!(strategy.on_market_event(&ctx, &event).is_empty());
        assert!(strategy.on_timer(&ctx).is_empty());
    }
}
