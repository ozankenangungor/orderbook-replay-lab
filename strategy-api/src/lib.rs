use lob_core::{MarketEvent, Price, Qty, Symbol};
use trading_types::{ExecutionReport, Intent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSnapshot {
    pub ts_ns: u64,
    pub symbol: Symbol,
    pub best_bid: Option<(Price, Qty)>,
    pub best_ask: Option<(Price, Qty)>,
    pub position_lots: i64,
    pub open_orders: usize,
    pub mid_price: Option<Price>,
}

impl ContextSnapshot {
    pub fn new(
        ts_ns: u64,
        symbol: Symbol,
        best_bid: Option<(Price, Qty)>,
        best_ask: Option<(Price, Qty)>,
        position_lots: i64,
        open_orders: usize,
    ) -> Self {
        let mid_price = match (best_bid, best_ask) {
            (Some((bid, _)), Some((ask, _))) => {
                let bid_ticks = bid.ticks();
                let ask_ticks = ask.ticks();
                let mid_ticks = (bid_ticks + ask_ticks) / 2;
                Price::new(mid_ticks).ok()
            }
            _ => None,
        };
        Self {
            ts_ns,
            symbol,
            best_bid,
            best_ask,
            position_lots,
            open_orders,
            mid_price,
        }
    }
}

pub trait Strategy {
    fn on_market_event(
        &mut self,
        ctx: &ContextSnapshot,
        event: &MarketEvent,
        out: &mut Vec<Intent>,
    );

    fn on_timer(&mut self, _ctx: &ContextSnapshot, _out: &mut Vec<Intent>) {}

    fn on_execution_report(
        &mut self,
        _ctx: &ContextSnapshot,
        _report: &ExecutionReport,
        _out: &mut Vec<Intent>,
    ) {
    }
}
