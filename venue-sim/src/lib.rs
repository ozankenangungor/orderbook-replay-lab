use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use lob_core::{Price, Qty, Side, Symbol};
use oms::OrderRequest;
use orderbook::OrderBook;
use trading_types::{ClientOrderId, ExecutionReport, OrderStatus, OrderType};
use venue::ExecutionVenue;

#[derive(Debug, Clone)]
struct LiveOrder {
    symbol: Symbol,
    side: Side,
    price: Option<Price>,
    qty: Qty,
}

pub struct SimVenue {
    book: Rc<RefCell<OrderBook>>,
    #[allow(dead_code)]
    maker_fee_ticks: i64,
    taker_fee_ticks: i64,
    next_ts_ns: u64,
    live_orders: HashMap<ClientOrderId, LiveOrder>,
}

impl SimVenue {
    pub fn new(book: Rc<RefCell<OrderBook>>, maker_fee_ticks: i64, taker_fee_ticks: i64) -> Self {
        Self {
            book,
            maker_fee_ticks,
            taker_fee_ticks,
            next_ts_ns: 1,
            live_orders: HashMap::new(),
        }
    }

    fn next_ts(&mut self) -> u64 {
        let ts = self.next_ts_ns;
        self.next_ts_ns = self.next_ts_ns.saturating_add(1);
        ts
    }

    fn handle_place(&mut self, order: &trading_types::OrderRequest) -> Vec<ExecutionReport> {
        let (best_bid, best_ask) = {
            let book = self.book.borrow();
            (book.best_bid(), book.best_ask())
        };

        let crossing_price = match order.order_type {
            OrderType::Limit => {
                let Some(limit) = order.price else {
                    return vec![self.rejected(order)];
                };
                match order.side {
                    Side::Bid => best_ask.and_then(|(ask, _)| {
                        if limit.ticks() >= ask.ticks() {
                            Some(ask)
                        } else {
                            None
                        }
                    }),
                    Side::Ask => best_bid.and_then(|(bid, _)| {
                        if limit.ticks() <= bid.ticks() {
                            Some(bid)
                        } else {
                            None
                        }
                    }),
                }
            }
            OrderType::Market => match order.side {
                Side::Bid => best_ask.map(|(ask, _)| ask),
                Side::Ask => best_bid.map(|(bid, _)| bid),
            },
        };

        let mut reports = Vec::new();
        let ack_price = crossing_price
            .or(order.price)
            .unwrap_or_else(|| Price::new(0).unwrap());
        reports.push(ExecutionReport {
            client_order_id: order.client_order_id,
            status: OrderStatus::Accepted,
            filled_qty: Qty::new(0).unwrap(),
            last_fill_price: ack_price,
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol.clone(),
            side: order.side,
        });

        if let Some(fill_price) = crossing_price {
            reports.push(ExecutionReport {
                client_order_id: order.client_order_id,
                status: OrderStatus::Filled,
                filled_qty: order.qty,
                last_fill_price: fill_price,
                fee_ticks: self.taker_fee_ticks,
                ts_ns: self.next_ts(),
                symbol: order.symbol.clone(),
                side: order.side,
            });
        } else if order.order_type == OrderType::Limit {
            self.live_orders.insert(
                order.client_order_id,
                LiveOrder {
                    symbol: order.symbol.clone(),
                    side: order.side,
                    price: order.price,
                    qty: order.qty,
                },
            );
        }

        reports
    }

    fn handle_replace(
        &mut self,
        client_order_id: ClientOrderId,
        new_price: Price,
        new_qty: Qty,
    ) -> Vec<ExecutionReport> {
        let Some(mut order) = self.live_orders.remove(&client_order_id) else {
            return Vec::new();
        };

        order.price = Some(new_price);
        order.qty = new_qty;

        let (best_bid, best_ask) = {
            let book = self.book.borrow();
            (book.best_bid(), book.best_ask())
        };

        let crossing_price = match order.side {
            Side::Bid => best_ask.and_then(|(ask, _)| {
                if new_price.ticks() >= ask.ticks() {
                    Some(ask)
                } else {
                    None
                }
            }),
            Side::Ask => best_bid.and_then(|(bid, _)| {
                if new_price.ticks() <= bid.ticks() {
                    Some(bid)
                } else {
                    None
                }
            }),
        };

        let mut reports = Vec::new();
        reports.push(ExecutionReport {
            client_order_id,
            status: OrderStatus::Accepted,
            filled_qty: Qty::new(0).unwrap(),
            last_fill_price: new_price,
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol.clone(),
            side: order.side,
        });

        if let Some(fill_price) = crossing_price {
            reports.push(ExecutionReport {
                client_order_id,
                status: OrderStatus::Filled,
                filled_qty: new_qty,
                last_fill_price: fill_price,
                fee_ticks: self.taker_fee_ticks,
                ts_ns: self.next_ts(),
                symbol: order.symbol,
                side: order.side,
            });
        } else {
            self.live_orders.insert(client_order_id, order);
        }

        reports
    }

    fn handle_cancel(&mut self, client_order_id: ClientOrderId) -> Vec<ExecutionReport> {
        let Some(order) = self.live_orders.remove(&client_order_id) else {
            return Vec::new();
        };

        vec![ExecutionReport {
            client_order_id,
            status: OrderStatus::Canceled,
            filled_qty: Qty::new(0).unwrap(),
            last_fill_price: order.price.unwrap_or_else(|| Price::new(0).unwrap()),
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol,
            side: order.side,
        }]
    }

    fn rejected(&mut self, order: &trading_types::OrderRequest) -> ExecutionReport {
        ExecutionReport {
            client_order_id: order.client_order_id,
            status: OrderStatus::Rejected,
            filled_qty: Qty::new(0).unwrap(),
            last_fill_price: order.price.unwrap_or_else(|| Price::new(0).unwrap()),
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol.clone(),
            side: order.side,
        }
    }
}

impl ExecutionVenue for SimVenue {
    fn submit(&mut self, req: &OrderRequest) -> Vec<ExecutionReport> {
        match req {
            OrderRequest::Place(order) => self.handle_place(order),
            OrderRequest::Cancel {
                client_order_id, ..
            } => self.handle_cancel(*client_order_id),
            OrderRequest::Replace {
                client_order_id,
                new_price,
                new_qty,
                ..
            } => self.handle_replace(*client_order_id, *new_price, *new_qty),
        }
    }
}
