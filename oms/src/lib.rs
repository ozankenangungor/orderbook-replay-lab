use std::collections::HashMap;

use lob_core::{Price, Qty, Side, SymbolId};
use trading_types::{
    ClientOrderId, ExecutionReport, Intent, OrderRequest as NewOrderRequest, OrderStatus, OrderType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderState {
    PendingNew,
    Live,
    PendingCancel,
    Canceled,
    Filled,
    Rejected,
}

impl OrderState {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            OrderState::Canceled | OrderState::Filled | OrderState::Rejected
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderRequest {
    Place(NewOrderRequest),
    Cancel {
        client_order_id: ClientOrderId,
        ts_ns: u64,
    },
    Replace {
        client_order_id: ClientOrderId,
        new_price: Price,
        new_qty: Qty,
        ts_ns: u64,
    },
}

#[derive(Debug, Clone)]
struct OrderEntry {
    state: OrderState,
    filled_qty: Qty,
    symbol: SymbolId,
    side: Side,
    total_qty: Qty,
    last_report_ts_ns: u64,
}

fn zero_qty() -> Qty {
    match Qty::new(0) {
        Ok(qty) => qty,
        Err(_) => unreachable!("zero qty must be valid"),
    }
}

pub struct Oms {
    next_id: u64,
    orders: HashMap<ClientOrderId, OrderEntry>,
    open_orders_count: usize,
    orphan_reports: u64,
    stale_reports: u64,
    invalid_reports: u64,
}

impl Oms {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            orders: HashMap::new(),
            open_orders_count: 0,
            orphan_reports: 0,
            stale_reports: 0,
            invalid_reports: 0,
        }
    }

    pub fn apply_intent(&mut self, intent: Intent, ts_ns: u64) -> Option<OrderRequest> {
        match intent {
            Intent::PlaceLimit {
                symbol,
                side,
                price,
                qty,
                tif,
                tag: _,
            } => {
                let client_order_id = ClientOrderId(self.next_id);
                self.next_id += 1;
                let request = NewOrderRequest {
                    client_order_id,
                    symbol,
                    side,
                    order_type: OrderType::Limit,
                    price: Some(price),
                    qty,
                    tif,
                };
                self.orders.insert(
                    client_order_id,
                    OrderEntry {
                        state: OrderState::PendingNew,
                        filled_qty: zero_qty(),
                        symbol,
                        side,
                        total_qty: qty,
                        last_report_ts_ns: 0,
                    },
                );
                self.open_orders_count = self.open_orders_count.saturating_add(1);
                Some(OrderRequest::Place(request))
            }
            Intent::Cancel { client_order_id } => {
                if let Some(entry) = self.orders.get_mut(&client_order_id) {
                    if entry.state.is_terminal() {
                        return None;
                    }
                    entry.state = OrderState::PendingCancel;
                    return Some(OrderRequest::Cancel {
                        client_order_id,
                        ts_ns,
                    });
                }
                None
            }
            Intent::Replace {
                client_order_id,
                new_price,
                new_qty,
            } => {
                if let Some(entry) = self.orders.get_mut(&client_order_id) {
                    if entry.state.is_terminal() {
                        return None;
                    }
                    entry.state = OrderState::PendingNew;
                    entry.total_qty = if new_qty.lots() >= entry.filled_qty.lots() {
                        new_qty
                    } else {
                        entry.filled_qty
                    };
                    return Some(OrderRequest::Replace {
                        client_order_id,
                        new_price,
                        new_qty,
                        ts_ns,
                    });
                }
                None
            }
        }
    }

    pub fn on_execution_report(&mut self, report: &ExecutionReport) {
        let Some(entry) = self.orders.get_mut(&report.client_order_id) else {
            self.orphan_reports += 1;
            return;
        };

        if report.ts_ns < entry.last_report_ts_ns {
            self.stale_reports += 1;
            return;
        }
        if report.symbol != entry.symbol || report.side != entry.side {
            self.invalid_reports += 1;
            return;
        }

        let new_state = map_status(report.status);
        let report_qty = report.filled_qty.lots();
        let current_qty = entry.filled_qty.lots();
        if report_qty > entry.total_qty.lots() {
            self.invalid_reports += 1;
            return;
        }

        if report_qty < current_qty {
            // `filled_qty` is treated as cumulative per order. A lower value indicates a
            // stale/out-of-order report, so keep the last known max and ignore regression.
            self.stale_reports += 1;
            return;
        }
        debug_assert!(
            report_qty >= current_qty,
            "execution report cumulative filled_qty regressed"
        );
        if report_qty == current_qty && entry.state == new_state {
            entry.last_report_ts_ns = report.ts_ns;
            return;
        }
        if !is_valid_transition(entry.state, new_state) {
            self.invalid_reports += 1;
            return;
        }

        let was_open = !entry.state.is_terminal();
        let is_open = !new_state.is_terminal();
        if was_open && !is_open {
            self.open_orders_count = self.open_orders_count.saturating_sub(1);
        } else if !was_open && is_open {
            self.open_orders_count = self.open_orders_count.saturating_add(1);
        }

        entry.filled_qty = report.filled_qty;
        entry.state = new_state;
        entry.last_report_ts_ns = report.ts_ns;
    }

    pub fn orphan_report_count(&self) -> u64 {
        self.orphan_reports
    }

    pub fn open_orders(&self) -> usize {
        self.open_orders_count
    }

    pub fn stale_report_count(&self) -> u64 {
        self.stale_reports
    }

    pub fn invalid_report_count(&self) -> u64 {
        self.invalid_reports
    }

    #[cfg(test)]
    fn order_state(&self, client_order_id: ClientOrderId) -> Option<OrderState> {
        self.orders.get(&client_order_id).map(|entry| entry.state)
    }

    #[cfg(test)]
    fn filled_qty(&self, client_order_id: ClientOrderId) -> Option<Qty> {
        self.orders
            .get(&client_order_id)
            .map(|entry| entry.filled_qty)
    }
}

impl Default for Oms {
    fn default() -> Self {
        Self::new()
    }
}

fn map_status(status: OrderStatus) -> OrderState {
    match status {
        OrderStatus::New => OrderState::PendingNew,
        OrderStatus::Accepted | OrderStatus::Working | OrderStatus::PartiallyFilled => {
            OrderState::Live
        }
        OrderStatus::Canceled | OrderStatus::Expired => OrderState::Canceled,
        OrderStatus::Filled => OrderState::Filled,
        OrderStatus::Rejected => OrderState::Rejected,
    }
}

fn is_valid_transition(current: OrderState, next: OrderState) -> bool {
    match current {
        OrderState::PendingNew => matches!(
            next,
            OrderState::PendingNew
                | OrderState::Live
                | OrderState::Filled
                | OrderState::Canceled
                | OrderState::Rejected
        ),
        OrderState::Live => matches!(
            next,
            OrderState::Live
                | OrderState::PendingCancel
                | OrderState::Filled
                | OrderState::Canceled
        ),
        OrderState::PendingCancel => matches!(
            next,
            OrderState::PendingCancel
                | OrderState::Live
                | OrderState::Filled
                | OrderState::Canceled
        ),
        OrderState::Canceled | OrderState::Filled | OrderState::Rejected => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{Side, SymbolId};
    use trading_types::TimeInForce;

    fn build_report(
        client_order_id: ClientOrderId,
        symbol: SymbolId,
        side: Side,
        status: OrderStatus,
        filled_qty: i64,
        ts_ns: u64,
    ) -> ExecutionReport {
        ExecutionReport {
            client_order_id,
            status,
            filled_qty: Qty::new(filled_qty).unwrap(),
            last_fill_price: Price::new(100).unwrap(),
            fee_ticks: 0,
            ts_ns,
            symbol,
            side,
        }
    }

    #[test]
    fn new_ack_fill_flow() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(1);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Bid,
            price: Price::new(100).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };

        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };
        let id = order.client_order_id;
        assert_eq!(oms.order_state(id), Some(OrderState::PendingNew));
        assert_eq!(oms.open_orders(), 1);

        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Bid,
            OrderStatus::Accepted,
            0,
            2,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Live));
        assert_eq!(oms.open_orders(), 1);

        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Bid,
            OrderStatus::Filled,
            2,
            3,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Filled));
        assert_eq!(oms.filled_qty(id).unwrap().lots(), 2);
        assert_eq!(oms.open_orders(), 0);
    }

    #[test]
    fn cancel_flow() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(2);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Ask,
            price: Price::new(200).unwrap(),
            qty: Qty::new(1).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };
        let id = order.client_order_id;
        assert_eq!(oms.open_orders(), 1);
        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Ask,
            OrderStatus::Accepted,
            0,
            2,
        ));
        assert_eq!(oms.open_orders(), 1);

        let cancel_intent = Intent::Cancel {
            client_order_id: id,
        };
        let cancel_req = oms.apply_intent(cancel_intent, 3).unwrap();
        assert!(matches!(cancel_req, OrderRequest::Cancel { .. }));
        assert_eq!(oms.order_state(id), Some(OrderState::PendingCancel));
        assert_eq!(oms.open_orders(), 1);

        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Ask,
            OrderStatus::Canceled,
            0,
            4,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Canceled));
        assert_eq!(oms.open_orders(), 0);
    }

    #[test]
    fn duplicate_fill_report_does_not_double_count() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(3);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Bid,
            price: Price::new(50).unwrap(),
            qty: Qty::new(3).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };
        let id = order.client_order_id;
        assert_eq!(oms.open_orders(), 1);
        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Bid,
            OrderStatus::Accepted,
            0,
            2,
        ));

        let fill = build_report(id, symbol, Side::Bid, OrderStatus::Filled, 3, 3);
        oms.on_execution_report(&fill);
        oms.on_execution_report(&fill);

        assert_eq!(oms.order_state(id), Some(OrderState::Filled));
        assert_eq!(oms.filled_qty(id).unwrap().lots(), 3);
        assert_eq!(oms.open_orders(), 0);
    }

    #[test]
    fn terminal_orders_do_not_reopen_on_late_live_reports() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(4);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Bid,
            price: Price::new(100).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };
        let id = order.client_order_id;

        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Bid,
            OrderStatus::Filled,
            2,
            2,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Filled));
        assert_eq!(oms.open_orders(), 0);

        oms.on_execution_report(&build_report(
            id,
            symbol,
            Side::Bid,
            OrderStatus::Accepted,
            2,
            3,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Filled));
        assert_eq!(oms.open_orders(), 0);
        assert_eq!(oms.invalid_report_count(), 1);
    }

    #[test]
    fn mismatched_symbol_report_is_rejected() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(5);
        let other_symbol = SymbolId::from_u32(6);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Ask,
            price: Price::new(120).unwrap(),
            qty: Qty::new(1).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };

        oms.on_execution_report(&build_report(
            order.client_order_id,
            other_symbol,
            Side::Ask,
            OrderStatus::Accepted,
            0,
            2,
        ));
        assert_eq!(
            oms.order_state(order.client_order_id),
            Some(OrderState::PendingNew)
        );
        assert_eq!(oms.invalid_report_count(), 1);
    }

    #[test]
    fn excessive_cumulative_fill_is_rejected() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(7);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Bid,
            price: Price::new(50).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };

        oms.on_execution_report(&build_report(
            order.client_order_id,
            symbol,
            Side::Bid,
            OrderStatus::PartiallyFilled,
            3,
            2,
        ));
        assert_eq!(
            oms.order_state(order.client_order_id),
            Some(OrderState::PendingNew)
        );
        assert_eq!(oms.invalid_report_count(), 1);
    }

    #[test]
    fn stale_timestamp_report_is_ignored() {
        let mut oms = Oms::new();
        let symbol = SymbolId::from_u32(8);
        let intent = Intent::PlaceLimit {
            symbol,
            side: Side::Ask,
            price: Price::new(200).unwrap(),
            qty: Qty::new(1).unwrap(),
            tif: TimeInForce::Gtc,
            tag: None,
        };
        let request = oms.apply_intent(intent, 1).unwrap();
        let OrderRequest::Place(order) = request else {
            panic!("expected place request");
        };

        oms.on_execution_report(&build_report(
            order.client_order_id,
            symbol,
            Side::Ask,
            OrderStatus::Accepted,
            0,
            3,
        ));
        assert_eq!(
            oms.order_state(order.client_order_id),
            Some(OrderState::Live)
        );

        oms.on_execution_report(&build_report(
            order.client_order_id,
            symbol,
            Side::Ask,
            OrderStatus::Accepted,
            0,
            2,
        ));
        assert_eq!(
            oms.order_state(order.client_order_id),
            Some(OrderState::Live)
        );
        assert_eq!(oms.stale_report_count(), 1);
    }
}
