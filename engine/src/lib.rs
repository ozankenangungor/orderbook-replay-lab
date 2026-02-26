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
use trading_types::{ExecutionReport, Intent};
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
    intent_queue: VecDeque<Intent>,
    intent_buffer: Vec<Intent>,
    report_buffer: Vec<ExecutionReport>,
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
            intent_queue: VecDeque::new(),
            intent_buffer: Vec::new(),
            report_buffer: Vec::new(),
        }
    }

    pub fn on_market_event(&mut self, event: &MarketEvent) -> bool {
        // Measures book apply + strategy decision + routing/venue response handling.
        let start = Instant::now();
        let applied = self.on_market_event_deterministic(event);
        if !applied {
            return false;
        }

        let ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.latency.record(ns.max(1));
        true
    }

    pub fn on_market_event_deterministic(&mut self, event: &MarketEvent) -> bool {
        let applied = self.book.borrow_mut().apply(event);
        if !applied {
            return false;
        }

        let (ts_ns, symbol) = match event {
            MarketEvent::L2Delta { ts_ns, symbol, .. } => (*ts_ns, symbol),
            MarketEvent::L2Snapshot { ts_ns, symbol, .. } => (*ts_ns, symbol),
        };

        let mut queue = std::mem::take(&mut self.intent_queue);
        let mut intents = std::mem::take(&mut self.intent_buffer);
        let mut reports = std::mem::take(&mut self.report_buffer);

        queue.clear();
        intents.clear();
        reports.clear();

        self.venue.on_book_update(&mut reports);
        self.process_reports(&mut reports, &mut queue, &mut intents);

        let ctx = self.build_context(ts_ns, symbol);
        self.strategy.on_market_event(&ctx, event, &mut intents);
        queue.extend(intents.drain(..));
        self.handle_intent_queue(ts_ns, symbol, &mut queue, &mut reports, &mut intents);

        self.intent_queue = queue;
        self.intent_buffer = intents;
        self.report_buffer = reports;
        true
    }

    pub fn on_timer(&mut self, ts_ns: u64, symbol: &Symbol) {
        let mut queue = std::mem::take(&mut self.intent_queue);
        let mut intents = std::mem::take(&mut self.intent_buffer);
        let mut reports = std::mem::take(&mut self.report_buffer);

        queue.clear();
        intents.clear();
        reports.clear();

        let ctx = self.build_context(ts_ns, symbol);
        self.strategy.on_timer(&ctx, &mut intents);
        queue.extend(intents.drain(..));
        self.handle_intent_queue(ts_ns, symbol, &mut queue, &mut reports, &mut intents);

        self.intent_queue = queue;
        self.intent_buffer = intents;
        self.report_buffer = reports;
    }

    fn handle_intent_queue(
        &mut self,
        ts_ns: u64,
        symbol: &Symbol,
        queue: &mut VecDeque<Intent>,
        reports: &mut Vec<ExecutionReport>,
        intents: &mut Vec<Intent>,
    ) {
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

            let intent_ctx = self.build_context(ts_ns, symbol);
            let decision = self.risk.evaluate(&intent_ctx, &intent);
            let intent = match decision {
                RiskAction::Allow(intent) | RiskAction::Transform(intent) => intent,
                RiskAction::Reject { .. } => continue,
            };

            let Some(request) = self.oms.apply_intent(intent, ts_ns) else {
                continue;
            };
            reports.clear();
            self.venue.submit(&request, reports);
            self.process_reports(reports, queue, intents);
        }
    }

    fn process_reports(
        &mut self,
        reports: &mut Vec<ExecutionReport>,
        queue: &mut VecDeque<Intent>,
        intents: &mut Vec<Intent>,
    ) {
        for report in reports.drain(..) {
            self.oms.on_execution_report(&report);
            self.portfolio.on_execution_report(&report);
            let report_ctx = self.build_context(report.ts_ns, &report.symbol);
            intents.clear();
            self.strategy
                .on_execution_report(&report_ctx, &report, intents);
            queue.extend(intents.drain(..));
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
    use std::collections::HashMap;

    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side};
    use trading_types::{ClientOrderId, ExecutionReport, OrderStatus, TimeInForce};

    struct DummyStrategy {
        placed: bool,
    }

    impl DummyStrategy {
        fn new() -> Self {
            Self { placed: false }
        }
    }

    impl Strategy for DummyStrategy {
        fn on_market_event(
            &mut self,
            ctx: &ContextSnapshot,
            _event: &MarketEvent,
            out: &mut Vec<Intent>,
        ) {
            if self.placed {
                return;
            }
            let Some((ask, _)) = ctx.best_ask else {
                return;
            };
            self.placed = true;
            out.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            });
        }
    }

    struct DummyVenue;

    impl ExecutionVenue for DummyVenue {
        fn submit(&mut self, req: &oms::OrderRequest, out: &mut Vec<ExecutionReport>) {
            let oms::OrderRequest::Place(order) = req else {
                return;
            };

            let symbol = order.symbol.clone();
            let side = order.side;
            let price = order.price.unwrap_or_else(|| Price::new(0).unwrap());
            let qty = order.qty;
            out.push(ExecutionReport {
                client_order_id: order.client_order_id,
                status: OrderStatus::Accepted,
                filled_qty: Qty::new(0).unwrap(),
                last_fill_price: price,
                fee_ticks: 0,
                ts_ns: 1,
                symbol: symbol.clone(),
                side,
            });
            out.push(ExecutionReport {
                client_order_id: order.client_order_id,
                status: OrderStatus::Filled,
                filled_qty: qty,
                last_fill_price: price,
                fee_ticks: 0,
                ts_ns: 2,
                symbol,
                side,
            });
        }
    }

    #[derive(Clone)]
    struct PassiveLiveOrder {
        symbol: Symbol,
        side: Side,
        price: Price,
        qty: Qty,
    }

    struct PassiveFillVenue {
        book: Rc<RefCell<OrderBook>>,
        next_ts_ns: u64,
        live_orders: HashMap<ClientOrderId, PassiveLiveOrder>,
    }

    impl PassiveFillVenue {
        fn new(book: Rc<RefCell<OrderBook>>) -> Self {
            Self {
                book,
                next_ts_ns: 1,
                live_orders: HashMap::new(),
            }
        }

        fn next_ts(&mut self) -> u64 {
            let ts = self.next_ts_ns;
            self.next_ts_ns = self.next_ts_ns.saturating_add(1);
            ts
        }
    }

    impl ExecutionVenue for PassiveFillVenue {
        fn submit(&mut self, req: &oms::OrderRequest, out: &mut Vec<ExecutionReport>) {
            let oms::OrderRequest::Place(order) = req else {
                return;
            };
            let Some(limit_price) = order.price else {
                return;
            };

            let (best_bid, best_ask) = {
                let book = self.book.borrow();
                (book.best_bid(), book.best_ask())
            };
            let crossing_price = match order.side {
                Side::Bid => best_ask.and_then(|(ask, _)| {
                    if limit_price.ticks() >= ask.ticks() {
                        Some(ask)
                    } else {
                        None
                    }
                }),
                Side::Ask => best_bid.and_then(|(bid, _)| {
                    if limit_price.ticks() <= bid.ticks() {
                        Some(bid)
                    } else {
                        None
                    }
                }),
            };

            out.push(ExecutionReport {
                client_order_id: order.client_order_id,
                status: OrderStatus::Accepted,
                filled_qty: Qty::new(0).unwrap(),
                last_fill_price: limit_price,
                fee_ticks: 0,
                ts_ns: self.next_ts(),
                symbol: order.symbol.clone(),
                side: order.side,
            });

            if let Some(fill_price) = crossing_price {
                out.push(ExecutionReport {
                    client_order_id: order.client_order_id,
                    status: OrderStatus::Filled,
                    filled_qty: order.qty,
                    last_fill_price: fill_price,
                    fee_ticks: 0,
                    ts_ns: self.next_ts(),
                    symbol: order.symbol.clone(),
                    side: order.side,
                });
            } else {
                self.live_orders.insert(
                    order.client_order_id,
                    PassiveLiveOrder {
                        symbol: order.symbol.clone(),
                        side: order.side,
                        price: limit_price,
                        qty: order.qty,
                    },
                );
            }
        }

        fn on_book_update(&mut self, out: &mut Vec<ExecutionReport>) {
            let (best_bid, best_ask) = {
                let book = self.book.borrow();
                (book.best_bid(), book.best_ask())
            };

            let mut order_ids: Vec<ClientOrderId> = self.live_orders.keys().copied().collect();
            order_ids.sort_by_key(|id| id.0);
            for client_order_id in order_ids {
                let Some(order) = self.live_orders.get(&client_order_id).cloned() else {
                    continue;
                };

                let fill_price = match order.side {
                    Side::Bid => best_ask.and_then(|(ask, _)| {
                        if order.price.ticks() >= ask.ticks() {
                            Some(ask)
                        } else {
                            None
                        }
                    }),
                    Side::Ask => best_bid.and_then(|(bid, _)| {
                        if order.price.ticks() <= bid.ticks() {
                            Some(bid)
                        } else {
                            None
                        }
                    }),
                };
                let Some(fill_price) = fill_price else {
                    continue;
                };

                self.live_orders.remove(&client_order_id);
                out.push(ExecutionReport {
                    client_order_id,
                    status: OrderStatus::Filled,
                    filled_qty: order.qty,
                    last_fill_price: fill_price,
                    fee_ticks: 0,
                    ts_ns: self.next_ts(),
                    symbol: order.symbol,
                    side: order.side,
                });
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
        fn on_market_event(
            &mut self,
            ctx: &ContextSnapshot,
            _event: &MarketEvent,
            out: &mut Vec<Intent>,
        ) {
            if self.placed_initial {
                return;
            }
            let Some((ask, _)) = ctx.best_ask else {
                return;
            };
            self.placed_initial = true;
            out.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            });
        }

        fn on_execution_report(
            &mut self,
            ctx: &ContextSnapshot,
            report: &ExecutionReport,
            out: &mut Vec<Intent>,
        ) {
            if self.placed_after_fill || report.status != OrderStatus::Filled {
                return;
            }
            let Some((ask, _)) = ctx.best_ask else {
                return;
            };
            self.placed_after_fill = true;
            out.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            });
        }
    }

    struct TimerOnlyStrategy {
        fired: bool,
    }

    impl TimerOnlyStrategy {
        fn new() -> Self {
            Self { fired: false }
        }
    }

    impl Strategy for TimerOnlyStrategy {
        fn on_market_event(
            &mut self,
            _ctx: &ContextSnapshot,
            _event: &MarketEvent,
            _out: &mut Vec<Intent>,
        ) {
        }

        fn on_timer(&mut self, ctx: &ContextSnapshot, out: &mut Vec<Intent>) {
            if self.fired {
                return;
            }
            let Some((ask, _)) = ctx.best_ask else {
                return;
            };
            self.fired = true;
            out.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: ask,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            });
        }
    }

    struct RestingBidStrategy {
        placed: bool,
    }

    impl RestingBidStrategy {
        fn new() -> Self {
            Self { placed: false }
        }
    }

    impl Strategy for RestingBidStrategy {
        fn on_market_event(
            &mut self,
            ctx: &ContextSnapshot,
            _event: &MarketEvent,
            out: &mut Vec<Intent>,
        ) {
            if self.placed {
                return;
            }
            let Some((bid, _)) = ctx.best_bid else {
                return;
            };
            self.placed = true;
            out.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: bid,
                qty: Qty::new(1).unwrap(),
                tif: TimeInForce::Gtc,
                tag: None,
            });
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

    #[test]
    fn timer_tick_routes_strategy_intents() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let mut engine = Engine::new(
            OrderBook::new(symbol.clone()),
            Portfolio::new(),
            Oms::new(),
            RiskEngine::new(),
            Box::new(TimerOnlyStrategy::new()),
            Box::new(DummyVenue),
        );

        let snapshot = MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol: symbol.clone(),
            bids: vec![(Price::new(100).unwrap(), Qty::new(1).unwrap())],
            asks: vec![(Price::new(101).unwrap(), Qty::new(1).unwrap())],
        };
        assert!(engine.on_market_event(&snapshot));
        assert_eq!(engine.position_lots(&symbol), 0);

        engine.on_timer(2, &symbol);
        assert_eq!(engine.position_lots(&symbol), 1);
    }

    #[test]
    fn passive_fill_triggers_when_market_moves_through_resting_order() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let shared_book = Rc::new(RefCell::new(OrderBook::new(symbol.clone())));
        let venue = PassiveFillVenue::new(shared_book.clone());
        let mut engine = Engine::with_shared_book(
            shared_book,
            Portfolio::new(),
            Oms::new(),
            RiskEngine::new(),
            Box::new(RestingBidStrategy::new()),
            Box::new(venue),
        );

        let snapshot = MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol: symbol.clone(),
            bids: vec![(Price::new(100).unwrap(), Qty::new(1).unwrap())],
            asks: vec![(Price::new(101).unwrap(), Qty::new(1).unwrap())],
        };
        assert!(engine.on_market_event(&snapshot));
        assert_eq!(engine.position_lots(&symbol), 0);

        let delta = MarketEvent::L2Delta {
            ts_ns: 2,
            symbol: symbol.clone(),
            updates: vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(100).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        };
        assert!(engine.on_market_event(&delta));
        assert_eq!(engine.position_lots(&symbol), 1);
    }
}
