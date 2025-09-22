use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use lob_core::{MarketEvent, Symbol};
use metrics::LatencyStats;
use oms::Oms;
use orderbook::OrderBook;
use portfolio::Portfolio;
use risk::{RiskAction, RiskEngine};
use strategy_api::{ContextSnapshot, Strategy};
use trading_types::Intent;
use venue::ExecutionVenue;

const MAX_INTENT_STEPS: usize = 1024;

pub struct Engine {
    book: Rc<RefCell<OrderBook>>,
    portfolio: Portfolio,
    oms: Oms,
    risk: RiskEngine,
    strategy: Box<dyn Strategy>,
    venue: Box<dyn ExecutionVenue>,
    latency: LatencyStats,
}

impl Engine {
    pub fn new(
        book: OrderBook,
        portfolio: Portfolio,
        oms: Oms,
        risk: RiskEngine,
        strategy: Box<dyn Strategy>,
        venue: Box<dyn ExecutionVenue>,
    ) -> Self {
        Self::with_shared_book(
            Rc::new(RefCell::new(book)),
            portfolio,
            oms,
            risk,
            strategy,
            venue,
        )
    }

    pub fn with_shared_book(
        book: Rc<RefCell<OrderBook>>,
        portfolio: Portfolio,
        oms: Oms,
        risk: RiskEngine,
        strategy: Box<dyn Strategy>,
        venue: Box<dyn ExecutionVenue>,
    ) -> Self {
        Self {
            book,
            portfolio,
            oms,
            risk,
            strategy,
            venue,
            latency: LatencyStats::new(),
        }
    }

    pub fn on_market_event(&mut self, event: &MarketEvent) -> bool {
        // Measures book apply + strategy decision + routing/venue response handling.
        let start = Instant::now();

        let applied = self.book.borrow_mut().apply(event);
        if !applied {
            return false;
        }

        let (ts_ns, symbol) = match event {
            MarketEvent::L2Delta { ts_ns, symbol, .. } => (*ts_ns, symbol.clone()),
            MarketEvent::L2Snapshot { ts_ns, symbol, .. } => (*ts_ns, symbol.clone()),
        };

        let ctx = self.build_context(ts_ns, &symbol);
        let intents = self.strategy.on_market_event(&ctx, event);
        self.handle_intents(ts_ns, &ctx, intents);

        let ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.latency.record(ns.max(1));
        true
    }

    fn handle_intents(&mut self, ts_ns: u64, ctx: &ContextSnapshot, intents: Vec<Intent>) {
        let mut queue = VecDeque::from(intents);
        let mut processed_steps = 0usize;

        while let Some(intent) = queue.pop_front() {
            if processed_steps >= MAX_INTENT_STEPS {
                debug_assert!(
                    false,
                    "intent processing exceeded MAX_INTENT_STEPS; stopping to prevent churn"
                );
                break;
            }
            processed_steps += 1;

            let intent_ctx = self.build_context(ts_ns, &ctx.symbol);
            let decision = self.risk.evaluate(&intent_ctx, &intent);
            let intent = match decision {
                RiskAction::Allow(intent) | RiskAction::Transform(intent) => intent,
                RiskAction::Reject { .. } => continue,
            };

            let Some(request) = self.oms.apply_intent(intent, ts_ns) else {
                continue;
            };
            let reports = self.venue.submit(&request);
            for report in reports {
                self.oms.on_execution_report(&report);
                self.portfolio.on_execution_report(&report);
                let report_ctx = self.build_context(report.ts_ns, &report.symbol);
                let follow_up = self.strategy.on_execution_report(&report_ctx, &report);
                queue.extend(follow_up);
            }
        }
    }

    fn build_context(&self, ts_ns: u64, symbol: &Symbol) -> ContextSnapshot {
        let (best_bid, best_ask) = {
            let book = self.book.borrow();
            (book.best_bid(), book.best_ask())
        };
        let position_lots = self.portfolio.position_lots(symbol);
        let open_orders = self.oms.open_orders();
        ContextSnapshot::new(
            ts_ns,
            symbol.clone(),
            best_bid,
            best_ask,
            position_lots,
            open_orders,
        )
    }

    pub fn latency_stats(&self) -> &LatencyStats {
        &self.latency
    }

    pub fn position_lots(&self, symbol: &Symbol) -> i64 {
        self.portfolio.position_lots(symbol)
    }

    pub fn realized_pnl_ticks(&self, symbol: &Symbol) -> i128 {
        self.portfolio.realized_pnl_ticks(symbol)
    }

    pub fn fees_paid_ticks(&self, symbol: &Symbol) -> i128 {
        self.portfolio.fees_paid_ticks(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side};
    use trading_types::{ExecutionReport, OrderStatus, TimeInForce};

    struct DummyStrategy {
        placed: bool,
    }

    impl DummyStrategy {
        fn new() -> Self {
            Self { placed: false }
        }
    }

    impl Strategy for DummyStrategy {
        fn on_market_event(&mut self, ctx: &ContextSnapshot, _event: &MarketEvent) -> Vec<Intent> {
            if self.placed {
                return Vec::new();
            }
            let Some((ask, _)) = ctx.best_ask else {
                return Vec::new();
            };
            self.placed = true;
            vec![Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            }]
        }
    }

    struct DummyVenue;

    impl ExecutionVenue for DummyVenue {
        fn submit(&mut self, req: &oms::OrderRequest) -> Vec<ExecutionReport> {
            match req {
                oms::OrderRequest::Place(order) => {
                    let symbol = order.symbol.clone();
                    let side = order.side;
                    let price = order.price.unwrap_or_else(|| Price::new(0).unwrap());
                    let qty = order.qty;
                    vec![
                        ExecutionReport {
                            client_order_id: order.client_order_id,
                            status: OrderStatus::Accepted,
                            filled_qty: Qty::new(0).unwrap(),
                            last_fill_price: price,
                            fee_ticks: 0,
                            ts_ns: 1,
                            symbol: symbol.clone(),
                            side,
                        },
                        ExecutionReport {
                            client_order_id: order.client_order_id,
                            status: OrderStatus::Filled,
                            filled_qty: qty,
                            last_fill_price: price,
                            fee_ticks: 0,
                            ts_ns: 2,
                            symbol,
                            side,
                        },
                    ]
                }
                _ => Vec::new(),
            }
        }
    }

    struct ReactiveFollowUpStrategy {
        placed_initial: bool,
        placed_after_fill: bool,
    }

    impl ReactiveFollowUpStrategy {
        fn new() -> Self {
            Self {
                placed_initial: false,
                placed_after_fill: false,
            }
        }
    }

    impl Strategy for ReactiveFollowUpStrategy {
        fn on_market_event(&mut self, ctx: &ContextSnapshot, _event: &MarketEvent) -> Vec<Intent> {
            if self.placed_initial {
                return Vec::new();
            }
            let Some((ask, _)) = ctx.best_ask else {
                return Vec::new();
            };
            self.placed_initial = true;
            vec![Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            }]
        }

        fn on_execution_report(
            &mut self,
            ctx: &ContextSnapshot,
            report: &ExecutionReport,
        ) -> Vec<Intent> {
            if self.placed_after_fill || report.status != OrderStatus::Filled {
                return Vec::new();
            }
            let Some((ask, _)) = ctx.best_ask else {
                return Vec::new();
            };
            self.placed_after_fill = true;
            vec![Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            }]
        }
    }

    #[test]
    fn snapshot_then_delta_triggers_fill_and_position() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let mut engine = Engine::new(
            OrderBook::new(symbol.clone()),
            Portfolio::new(),
            Oms::new(),
            RiskEngine::new(),
            Box::new(DummyStrategy::new()),
            Box::new(DummyVenue),
        );

        let snapshot = MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol: symbol.clone(),
            bids: vec![(Price::new(100).unwrap(), Qty::new(1).unwrap())],
            asks: vec![(Price::new(101).unwrap(), Qty::new(1).unwrap())],
        };
        assert!(engine.on_market_event(&snapshot));

        let delta = MarketEvent::L2Delta {
            ts_ns: 2,
            symbol: symbol.clone(),
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(2).unwrap(),
            }],
        };
        assert!(engine.on_market_event(&delta));

        assert_eq!(engine.position_lots(&symbol), 1);
    }

    #[test]
    fn execution_report_follow_up_intents_are_processed() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let mut engine = Engine::new(
            OrderBook::new(symbol.clone()),
            Portfolio::new(),
            Oms::new(),
            RiskEngine::new(),
            Box::new(ReactiveFollowUpStrategy::new()),
            Box::new(DummyVenue),
        );

        let snapshot = MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol: symbol.clone(),
            bids: vec![(Price::new(100).unwrap(), Qty::new(1).unwrap())],
            asks: vec![(Price::new(101).unwrap(), Qty::new(1).unwrap())],
        };
        assert!(engine.on_market_event(&snapshot));
        assert_eq!(engine.position_lots(&symbol), 2);
    }
}
