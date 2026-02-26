use std::collections::HashMap;

use lob_core::{Price, Qty};
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
}

impl Oms {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            orders: HashMap::new(),
            open_orders_count: 0,
            orphan_reports: 0,
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
                    },
                );
                self.open_orders_count = self.open_orders_count.saturating_add(1);
                Some(OrderRequest::Place(request))
            }
            Intent::Cancel { client_order_id } => {
                if let Some(entry) = self.orders.get_mut(&client_order_id) {
                    if !entry.state.is_terminal() {
                        entry.state = OrderState::PendingCancel;
                    }
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
                    if !entry.state.is_terminal() {
                        entry.state = OrderState::PendingNew;
                    }
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

        let new_state = map_status(report.status);
        let report_qty = report.filled_qty.lots();
        let current_qty = entry.filled_qty.lots();

        if report_qty < current_qty {
            // `filled_qty` is treated as cumulative per order. A lower value indicates a
            // stale/out-of-order report, so keep the last known max and ignore regression.
            return;
        }
        debug_assert!(
            report_qty >= current_qty,
            "execution report cumulative filled_qty regressed"
        );
        if report_qty == current_qty && entry.state == new_state {
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
    }

    pub fn orphan_report_count(&self) -> u64 {
        self.orphan_reports
    }

    pub fn open_orders(&self) -> usize {
        self.open_orders_count
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

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{Side, Symbol};
    use trading_types::TimeInForce;

    fn build_report(
        client_order_id: ClientOrderId,
        symbol: &Symbol,
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
            symbol: symbol.clone(),
            side,
        }
    }

    #[test]
    fn new_ack_fill_flow() {
        let mut oms = Oms::new();
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
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
            &Symbol::new("BTC-USD").unwrap(),
            Side::Bid,
            OrderStatus::Accepted,
            0,
            2,
        ));
        assert_eq!(oms.order_state(id), Some(OrderState::Live));
        assert_eq!(oms.open_orders(), 1);

        oms.on_execution_report(&build_report(
            id,
            &Symbol::new("BTC-USD").unwrap(),
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
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("ETH-USD").unwrap(),
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
            &Symbol::new("ETH-USD").unwrap(),
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
            &Symbol::new("ETH-USD").unwrap(),
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
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("SOL-USD").unwrap(),
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
            &Symbol::new("SOL-USD").unwrap(),
            Side::Bid,
            OrderStatus::Accepted,
            0,
            2,
        ));

        let fill = build_report(
            id,
            &Symbol::new("SOL-USD").unwrap(),
            Side::Bid,
            OrderStatus::Filled,
            3,
            3,
        );
        oms.on_execution_report(&fill);
        oms.on_execution_report(&fill);

        assert_eq!(oms.order_state(id), Some(OrderState::Filled));
        assert_eq!(oms.filled_qty(id).unwrap().lots(), 3);
        assert_eq!(oms.open_orders(), 0);
    }
}
