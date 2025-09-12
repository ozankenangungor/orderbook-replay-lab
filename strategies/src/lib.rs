use lob_core::{MarketEvent, Price, Qty, Side};
use strategy_api::{ContextSnapshot, Strategy};
use trading_types::{ClientOrderId, ExecutionReport, Intent, OrderStatus, TimeInForce};

pub struct NoopStrategy;

pub struct TwapStrategy {
    target_qty_lots: i64,
    slice_qty_lots: i64,
    remaining_qty_lots: i64,
    next_ts_ns: Option<u64>,
    interval_ns: u64,
    in_flight: bool,
    last_reported_qty: i64,
}

pub struct MmStrategy {
    half_spread_ticks: i64,
    quote_qty_lots: i64,
    skew_per_lot_ticks: i64,
    bid_order_id: Option<ClientOrderId>,
    ask_order_id: Option<ClientOrderId>,
    bid_price: Option<Price>,
    ask_price: Option<Price>,
    pending_bid: bool,
    pending_ask: bool,
}

impl TwapStrategy {
    pub fn new(target_qty_lots: i64, horizon_secs: u64, slice_qty_lots: i64) -> Self {
        let slice_qty_lots = slice_qty_lots.abs().max(1);
        let abs_target = target_qty_lots.unsigned_abs();
        let slice_u = slice_qty_lots as u64;
        let total_slices = if abs_target == 0 {
            0
        } else {
            abs_target.div_ceil(slice_u)
        };
        let horizon_ns = horizon_secs.saturating_mul(1_000_000_000);
        let mut interval_ns = if total_slices == 0 {
            0
        } else {
            horizon_ns / total_slices
        };
        if total_slices > 0 && interval_ns == 0 {
            interval_ns = 1;
        }

        Self {
            target_qty_lots,
            slice_qty_lots,
            remaining_qty_lots: target_qty_lots,
            next_ts_ns: None,
            interval_ns,
            in_flight: false,
            last_reported_qty: 0,
        }
    }

    fn maybe_place(&mut self, ctx: &ContextSnapshot) -> Vec<Intent> {
        if self.remaining_qty_lots == 0 || self.in_flight {
            return Vec::new();
        }

        let next_ts = self.next_ts_ns.get_or_insert(ctx.ts_ns);
        if ctx.ts_ns < *next_ts {
            return Vec::new();
        }

        let qty_lots = self.remaining_qty_lots.abs().min(self.slice_qty_lots);
        if qty_lots == 0 {
            return Vec::new();
        }

        let (side, price) = if self.remaining_qty_lots > 0 {
            (Side::Bid, ctx.best_ask.map(|(price, _)| price))
        } else {
            (Side::Ask, ctx.best_bid.map(|(price, _)| price))
        };
        let Some(price) = price else {
            return Vec::new();
        };

        self.in_flight = true;
        self.last_reported_qty = 0;
        *next_ts = ctx.ts_ns.saturating_add(self.interval_ns.max(1));

        vec![Intent::PlaceLimit {
            symbol: ctx.symbol.clone(),
            side,
            price,
            qty: Qty::new(qty_lots).expect("qty"),
            tif: TimeInForce::Gtc,
            tag: None,
        }]
    }

    fn on_report(&mut self, report: &ExecutionReport) {
        if !self.in_flight {
            return;
        }

        match report.status {
            OrderStatus::Filled | OrderStatus::PartiallyFilled => {
                let reported = report.filled_qty.lots();
                let delta = reported.saturating_sub(self.last_reported_qty);
                if delta > 0 {
                    if report.side == Side::Bid {
                        self.remaining_qty_lots -= delta;
                    } else {
                        self.remaining_qty_lots += delta;
                    }
                    self.last_reported_qty = reported;
                }
                if report.status == OrderStatus::Filled {
                    self.in_flight = false;
                    self.last_reported_qty = 0;
                }
            }
            OrderStatus::Canceled | OrderStatus::Rejected | OrderStatus::Expired => {
                self.in_flight = false;
                self.last_reported_qty = 0;
            }
            _ => {}
        }

        if (self.target_qty_lots >= 0 && self.remaining_qty_lots <= 0)
            || (self.target_qty_lots <= 0 && self.remaining_qty_lots >= 0)
        {
            self.remaining_qty_lots = 0;
            self.in_flight = false;
            self.last_reported_qty = 0;
        }
    }
}

impl MmStrategy {
    pub fn new(half_spread_ticks: i64, quote_qty_lots: i64, skew_per_lot_ticks: i64) -> Self {
        Self {
            half_spread_ticks: half_spread_ticks.abs().max(1),
            quote_qty_lots: quote_qty_lots.abs().max(1),
            skew_per_lot_ticks,
            bid_order_id: None,
            ask_order_id: None,
            bid_price: None,
            ask_price: None,
            pending_bid: false,
            pending_ask: false,
        }
    }

    fn quote(&mut self, ctx: &ContextSnapshot) -> Vec<Intent> {
        let Some(mid) = ctx.mid_price else {
            return self.cancel_all();
        };

        let skew = (ctx.position_lots as i128 * self.skew_per_lot_ticks as i128)
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
        let mid_ticks = mid.ticks();
        let mut bid_ticks = mid_ticks - self.half_spread_ticks - skew;
        let mut ask_ticks = mid_ticks + self.half_spread_ticks - skew;
        if bid_ticks < 1 {
            bid_ticks = 1;
        }
        if ask_ticks < 1 {
            ask_ticks = 1;
        }
        if ask_ticks <= bid_ticks {
            ask_ticks = bid_ticks + 1;
        }

        let bid_price = Price::new(bid_ticks).expect("bid price");
        let ask_price = Price::new(ask_ticks).expect("ask price");
        let qty = Qty::new(self.quote_qty_lots).expect("qty");

        let mut intents = Vec::new();
        if let Some(client_order_id) = self.bid_order_id {
            if !self.pending_bid && self.bid_price != Some(bid_price) {
                intents.push(Intent::Replace {
                    client_order_id,
                    new_price: bid_price,
                    new_qty: qty,
                });
                self.pending_bid = true;
                self.bid_price = Some(bid_price);
            }
        } else if !self.pending_bid {
            intents.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Bid,
                price: bid_price,
                qty,
                tif: TimeInForce::Gtc,
                tag: None,
            });
            self.pending_bid = true;
        }

        if let Some(client_order_id) = self.ask_order_id {
            if !self.pending_ask && self.ask_price != Some(ask_price) {
                intents.push(Intent::Replace {
                    client_order_id,
                    new_price: ask_price,
                    new_qty: qty,
                });
                self.pending_ask = true;
                self.ask_price = Some(ask_price);
            }
        } else if !self.pending_ask {
            intents.push(Intent::PlaceLimit {
                symbol: ctx.symbol.clone(),
                side: Side::Ask,
                price: ask_price,
                qty,
                tif: TimeInForce::Gtc,
                tag: None,
            });
            self.pending_ask = true;
        }

        intents
    }

    fn cancel_all(&mut self) -> Vec<Intent> {
        let mut intents = Vec::new();
        if let Some(client_order_id) = self.bid_order_id.take() {
            intents.push(Intent::Cancel { client_order_id });
        }
        if let Some(client_order_id) = self.ask_order_id.take() {
            intents.push(Intent::Cancel { client_order_id });
        }
        self.bid_price = None;
        self.ask_price = None;
        self.pending_bid = false;
        self.pending_ask = false;
        intents
    }

    fn on_report(&mut self, report: &ExecutionReport) {
        match report.side {
            Side::Bid => self.apply_report_side(report, true),
            Side::Ask => self.apply_report_side(report, false),
        }
    }

    fn apply_report_side(&mut self, report: &ExecutionReport, is_bid: bool) {
        let (order_id, price, pending) = if is_bid {
            (
                &mut self.bid_order_id,
                &mut self.bid_price,
                &mut self.pending_bid,
            )
        } else {
            (
                &mut self.ask_order_id,
                &mut self.ask_price,
                &mut self.pending_ask,
            )
        };

        match report.status {
            OrderStatus::Accepted | OrderStatus::Working | OrderStatus::PartiallyFilled => {
                *order_id = Some(report.client_order_id);
                *price = Some(report.last_fill_price);
                *pending = false;
            }
            OrderStatus::Filled
            | OrderStatus::Canceled
            | OrderStatus::Rejected
            | OrderStatus::Expired => {
                *order_id = None;
                *price = None;
                *pending = false;
            }
            _ => {}
        }
    }
}

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

impl Strategy for TwapStrategy {
    fn on_market_event(&mut self, ctx: &ContextSnapshot, _event: &MarketEvent) -> Vec<Intent> {
        self.maybe_place(ctx)
    }

    fn on_timer(&mut self, ctx: &ContextSnapshot) -> Vec<Intent> {
        self.maybe_place(ctx)
    }

    fn on_execution_report(
        &mut self,
        _ctx: &ContextSnapshot,
        report: &ExecutionReport,
    ) -> Vec<Intent> {
        self.on_report(report);
        Vec::new()
    }
}

impl Strategy for MmStrategy {
    fn on_market_event(&mut self, ctx: &ContextSnapshot, _event: &MarketEvent) -> Vec<Intent> {
        self.quote(ctx)
    }

    fn on_timer(&mut self, ctx: &ContextSnapshot) -> Vec<Intent> {
        self.quote(ctx)
    }

    fn on_execution_report(
        &mut self,
        _ctx: &ContextSnapshot,
        report: &ExecutionReport,
    ) -> Vec<Intent> {
        self.on_report(report);
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::Symbol;

    fn ctx_with_book(
        ts_ns: u64,
        symbol: Symbol,
        best_bid: i64,
        best_ask: i64,
        position_lots: i64,
    ) -> ContextSnapshot {
        ContextSnapshot::new(
            ts_ns,
            symbol,
            Some((Price::new(best_bid).unwrap(), Qty::new(1).unwrap())),
            Some((Price::new(best_ask).unwrap(), Qty::new(1).unwrap())),
            position_lots,
            0,
        )
    }

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

    #[test]
    fn twap_emits_until_target_reached() {
        let symbol = Symbol::new("TWAP-USD").unwrap();
        let mut strategy = TwapStrategy::new(3, 0, 1);

        let mut ctx = ctx_with_book(1, symbol.clone(), 100, 102, 0);
        let event = MarketEvent::L2Delta {
            ts_ns: 1,
            symbol: symbol.clone(),
            updates: vec![],
        };
        let intents = strategy.on_market_event(&ctx, &event);
        assert_eq!(intents.len(), 1);
        assert!(matches!(
            intents[0],
            Intent::PlaceLimit {
                side: Side::Bid,
                price,
                qty,
                tif: TimeInForce::Gtc,
                ..
            } if price == Price::new(102).unwrap() && qty == Qty::new(1).unwrap()
        ));

        let report = ExecutionReport {
            client_order_id: ClientOrderId(1),
            status: OrderStatus::Filled,
            filled_qty: Qty::new(1).unwrap(),
            last_fill_price: Price::new(102).unwrap(),
            fee_ticks: 0,
            ts_ns: 2,
            symbol: symbol.clone(),
            side: Side::Bid,
        };
        strategy.on_execution_report(&ctx, &report);

        ctx.ts_ns = 3;
        let intents = strategy.on_market_event(&ctx, &event);
        assert_eq!(intents.len(), 1);

        strategy.on_execution_report(
            &ctx,
            &ExecutionReport {
                filled_qty: Qty::new(1).unwrap(),
                ts_ns: 4,
                ..report.clone()
            },
        );

        ctx.ts_ns = 5;
        let intents = strategy.on_market_event(&ctx, &event);
        assert_eq!(intents.len(), 1);

        strategy.on_execution_report(
            &ctx,
            &ExecutionReport {
                filled_qty: Qty::new(1).unwrap(),
                ts_ns: 6,
                ..report
            },
        );

        ctx.ts_ns = 7;
        let intents = strategy.on_market_event(&ctx, &event);
        assert!(intents.is_empty());
    }

    #[test]
    fn mm_quotes_both_sides_and_skews_with_inventory() {
        let symbol = Symbol::new("MM-USD").unwrap();
        let mut mm = MmStrategy::new(2, 1, 1);
        let ctx = ctx_with_book(1, symbol.clone(), 100, 102, 0);
        let event = MarketEvent::L2Delta {
            ts_ns: 1,
            symbol: symbol.clone(),
            updates: vec![],
        };

        let intents = mm.on_market_event(&ctx, &event);
        assert_eq!(intents.len(), 2);

        let mut bid_price = None;
        let mut ask_price = None;
        for intent in intents {
            if let Intent::PlaceLimit { side, price, .. } = intent {
                match side {
                    Side::Bid => bid_price = Some(price.ticks()),
                    Side::Ask => ask_price = Some(price.ticks()),
                }
            }
        }

        assert_eq!(bid_price, Some(99));
        assert_eq!(ask_price, Some(103));

        let mut mm = MmStrategy::new(2, 1, 1);
        let skew_ctx = ctx_with_book(1, symbol, 100, 102, 5);
        let intents = mm.on_market_event(&skew_ctx, &event);

        let mut bid_price = None;
        let mut ask_price = None;
        for intent in intents {
            if let Intent::PlaceLimit { side, price, .. } = intent {
                match side {
                    Side::Bid => bid_price = Some(price.ticks()),
                    Side::Ask => ask_price = Some(price.ticks()),
                }
            }
        }

        assert_eq!(bid_price, Some(94));
        assert_eq!(ask_price, Some(98));
    }
}
