use std::collections::HashMap;

use lob_core::{Price, Qty, Side, Symbol};
use trading_types::{ClientOrderId, ExecutionReport, OrderStatus};

#[derive(Debug, Default, Clone)]
struct Position {
    position_lots: i64,
    realized_pnl_ticks: i128,
    fees_paid_ticks: i128,
    avg_entry_price_ticks: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Portfolio {
    positions: HashMap<Symbol, Position>,
    filled_by_order: HashMap<ClientOrderId, i64>,
}

impl Portfolio {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            filled_by_order: HashMap::new(),
        }
    }

    pub fn on_execution_report(&mut self, report: &ExecutionReport) {
        if matches!(
            report.status,
            OrderStatus::Canceled | OrderStatus::Rejected | OrderStatus::Expired
        ) {
            self.filled_by_order.remove(&report.client_order_id);
            return;
        }

        if report.status != OrderStatus::Filled && report.status != OrderStatus::PartiallyFilled {
            return;
        }

        let reported = report.filled_qty.lots();
        let prev = self
            .filled_by_order
            .get(&report.client_order_id)
            .copied()
            .unwrap_or(0);
        if reported <= prev {
            if report.status == OrderStatus::Filled {
                self.filled_by_order.remove(&report.client_order_id);
            }
            return;
        }

        let delta_qty = reported - prev;
        self.filled_by_order
            .insert(report.client_order_id, reported);

        let pos = self.positions.entry(report.symbol.clone()).or_default();
        let fill_price = report.last_fill_price.ticks();
        let signed_qty = if report.side == Side::Bid {
            delta_qty
        } else {
            -delta_qty
        };

        // Update realized pnl if reducing or flipping position.
        if pos.position_lots != 0 && (pos.position_lots.signum() != signed_qty.signum()) {
            if let Some(avg_entry) = pos.avg_entry_price_ticks {
                let close_qty = signed_qty.abs().min(pos.position_lots.abs());
                let pnl_per_lot = if pos.position_lots > 0 {
                    fill_price - avg_entry
                } else {
                    avg_entry - fill_price
                };
                pos.realized_pnl_ticks += pnl_per_lot as i128 * close_qty as i128;
            }
        }

        let new_position = pos.position_lots + signed_qty;

        // Update avg entry price for remaining/open position.
        if new_position == 0 {
            pos.avg_entry_price_ticks = None;
        } else if pos.position_lots == 0 || pos.position_lots.signum() == signed_qty.signum() {
            let old_qty = pos.position_lots.abs() as i128;
            let add_qty = signed_qty.abs() as i128;
            let total_qty = old_qty + add_qty;
            let old_avg = pos.avg_entry_price_ticks.unwrap_or(fill_price) as i128;
            let new_avg = (old_avg * old_qty + fill_price as i128 * add_qty) / total_qty;
            pos.avg_entry_price_ticks = Some(new_avg as i64);
        } else if new_position != 0 {
            pos.avg_entry_price_ticks = Some(fill_price);
        }

        pos.position_lots = new_position;
        pos.fees_paid_ticks += report.fee_ticks as i128;

        if report.status == OrderStatus::Filled {
            self.filled_by_order.remove(&report.client_order_id);
        }
    }

    pub fn mark_to_mid(
        &self,
        symbol: &Symbol,
        best_bid: Option<(Price, Qty)>,
        best_ask: Option<(Price, Qty)>,
    ) -> Option<i128> {
        let pos = self.positions.get(symbol)?;
        let (bid, ask) = match (best_bid, best_ask) {
            (Some((bid, _)), Some((ask, _))) => (bid.ticks(), ask.ticks()),
            _ => return None,
        };
        let mid = (bid + ask) / 2;
        let avg_entry = pos.avg_entry_price_ticks?;
        let unrealized = (mid - avg_entry) as i128 * pos.position_lots as i128;
        Some(unrealized)
    }

    pub fn position_lots(&self, symbol: &Symbol) -> i64 {
        self.positions
            .get(symbol)
            .map(|pos| pos.position_lots)
            .unwrap_or(0)
    }

    pub fn realized_pnl_ticks(&self, symbol: &Symbol) -> i128 {
        self.positions
            .get(symbol)
            .map(|pos| pos.realized_pnl_ticks)
            .unwrap_or(0)
    }

    pub fn fees_paid_ticks(&self, symbol: &Symbol) -> i128 {
        self.positions
            .get(symbol)
            .map(|pos| pos.fees_paid_ticks)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trading_types::{ClientOrderId, OrderStatus};

    fn report(
        client_order_id: ClientOrderId,
        symbol: &Symbol,
        qty: i64,
        price: i64,
        fee_ticks: i64,
        status: OrderStatus,
        side: lob_core::Side,
    ) -> ExecutionReport {
        ExecutionReport {
            client_order_id,
            status,
            filled_qty: Qty::new(qty).unwrap(),
            last_fill_price: Price::new(price).unwrap(),
            fee_ticks,
            ts_ns: 1,
            symbol: symbol.clone(),
            side,
        }
    }

    #[test]
    fn buy_then_sell_realizes_profit() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let mut portfolio = Portfolio::new();

        portfolio.on_execution_report(&report(
            ClientOrderId(1),
            &symbol,
            2,
            100,
            0,
            OrderStatus::Filled,
            lob_core::Side::Bid,
        ));
        portfolio.on_execution_report(&report(
            ClientOrderId(2),
            &symbol,
            2,
            110,
            0,
            OrderStatus::Filled,
            lob_core::Side::Ask,
        ));

        assert_eq!(portfolio.position_lots(&symbol), 0);
        assert_eq!(portfolio.realized_pnl_ticks(&symbol), 20);
    }

    #[test]
    fn fees_reduce_pnl() {
        let symbol = Symbol::new("ETH-USD").unwrap();
        let mut portfolio = Portfolio::new();

        portfolio.on_execution_report(&report(
            ClientOrderId(1),
            &symbol,
            1,
            100,
            2,
            OrderStatus::Filled,
            lob_core::Side::Bid,
        ));
        portfolio.on_execution_report(&report(
            ClientOrderId(2),
            &symbol,
            1,
            105,
            1,
            OrderStatus::Filled,
            lob_core::Side::Ask,
        ));

        assert_eq!(portfolio.realized_pnl_ticks(&symbol), 5);
        assert_eq!(portfolio.fees_paid_ticks(&symbol), 3);
    }

    #[test]
    fn mark_to_mid_uses_mid_price() {
        let symbol = Symbol::new("SOL-USD").unwrap();
        let mut portfolio = Portfolio::new();

        portfolio.on_execution_report(&report(
            ClientOrderId(1),
            &symbol,
            2,
            100,
            0,
            OrderStatus::Filled,
            lob_core::Side::Bid,
        ));

        let unrealized = portfolio
            .mark_to_mid(
                &symbol,
                Some((Price::new(104).unwrap(), Qty::new(1).unwrap())),
                Some((Price::new(106).unwrap(), Qty::new(1).unwrap())),
            )
            .unwrap();
        assert_eq!(unrealized, 10);
    }

    #[test]
    fn cumulative_partial_fills_use_delta_per_report() {
        let symbol = Symbol::new("AVAX-USD").unwrap();
        let mut portfolio = Portfolio::new();
        let id = ClientOrderId(7);

        portfolio.on_execution_report(&report(
            id,
            &symbol,
            1,
            100,
            1,
            OrderStatus::PartiallyFilled,
            lob_core::Side::Bid,
        ));
        portfolio.on_execution_report(&report(
            id,
            &symbol,
            3,
            101,
            1,
            OrderStatus::PartiallyFilled,
            lob_core::Side::Bid,
        ));
        portfolio.on_execution_report(&report(
            id,
            &symbol,
            5,
            102,
            1,
            OrderStatus::Filled,
            lob_core::Side::Bid,
        ));

        assert_eq!(portfolio.position_lots(&symbol), 5);
        assert_eq!(portfolio.fees_paid_ticks(&symbol), 3);
    }
}
