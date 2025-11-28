use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use lob_core::{Price, Qty, Side, SymbolId};
use oms::OrderRequest;
use orderbook::OrderBook;
use trading_types::{ClientOrderId, ExecutionReport, OrderStatus, OrderType};
use venue::ExecutionVenue;

const MAX_PASSIVE_FILLS_PER_EVENT: usize = 1024;

fn zero_price() -> Price {
    match Price::new(0) {
        Ok(price) => price,
        Err(_) => unreachable!("zero price must be valid"),
    }
}

fn zero_qty() -> Qty {
    match Qty::new(0) {
        Ok(qty) => qty,
        Err(_) => unreachable!("zero qty must be valid"),
    }
}

#[derive(Debug, Clone)]
struct LiveOrder {
    symbol: SymbolId,
    side: Side,
    price: Option<Price>,
    qty: Qty,
}

pub struct SimVenue {
    book: Rc<RefCell<OrderBook>>,
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

    fn handle_place(
        &mut self,
        order: &trading_types::OrderRequest,
        out: &mut Vec<ExecutionReport>,
    ) {
        let (best_bid, best_ask) = {
            let book = self.book.borrow();
            (book.best_bid(), book.best_ask())
        };

        let crossing_price = match order.order_type {
            OrderType::Limit => {
                let Some(limit) = order.price else {
                    out.push(self.rejected(order));
                    return;
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

        let ack_price = crossing_price.or(order.price).unwrap_or_else(zero_price);
        out.push(ExecutionReport {
            client_order_id: order.client_order_id,
            status: OrderStatus::Accepted,
            filled_qty: zero_qty(),
            last_fill_price: ack_price,
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol,
            side: order.side,
        });

        if let Some(fill_price) = crossing_price {
            out.push(ExecutionReport {
                client_order_id: order.client_order_id,
                status: OrderStatus::Filled,
                filled_qty: order.qty,
                last_fill_price: fill_price,
                fee_ticks: self.taker_fee_ticks,
                ts_ns: self.next_ts(),
                symbol: order.symbol,
                side: order.side,
            });
        } else if order.order_type == OrderType::Limit {
            self.live_orders.insert(
                order.client_order_id,
                LiveOrder {
                    symbol: order.symbol,
                    side: order.side,
                    price: order.price,
                    qty: order.qty,
                },
            );
        }
    }

    fn handle_replace(
        &mut self,
        client_order_id: ClientOrderId,
        new_price: Price,
        new_qty: Qty,
        out: &mut Vec<ExecutionReport>,
    ) {
        let Some(mut order) = self.live_orders.remove(&client_order_id) else {
            return;
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

        out.push(ExecutionReport {
            client_order_id,
            status: OrderStatus::Accepted,
            filled_qty: zero_qty(),
            last_fill_price: new_price,
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol,
            side: order.side,
        });

        if let Some(fill_price) = crossing_price {
            out.push(ExecutionReport {
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
    }

    fn handle_cancel(&mut self, client_order_id: ClientOrderId, out: &mut Vec<ExecutionReport>) {
        let Some(order) = self.live_orders.remove(&client_order_id) else {
            return;
        };

        out.push(ExecutionReport {
            client_order_id,
            status: OrderStatus::Canceled,
            filled_qty: zero_qty(),
            last_fill_price: order.price.unwrap_or_else(zero_price),
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol,
            side: order.side,
        });
    }

    fn rejected(&mut self, order: &trading_types::OrderRequest) -> ExecutionReport {
        ExecutionReport {
            client_order_id: order.client_order_id,
            status: OrderStatus::Rejected,
            filled_qty: zero_qty(),
            last_fill_price: order.price.unwrap_or_else(zero_price),
            fee_ticks: 0,
            ts_ns: self.next_ts(),
            symbol: order.symbol,
            side: order.side,
        }
    }

    fn passive_fill_price(
        side: Side,
        limit_price: Price,
        best_bid: Option<(Price, Qty)>,
        best_ask: Option<(Price, Qty)>,
    ) -> Option<Price> {
        match side {
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
        }
    }
}

impl ExecutionVenue for SimVenue {
    fn submit(&mut self, req: &OrderRequest, out: &mut Vec<ExecutionReport>) {
        match req {
            OrderRequest::Place(order) => self.handle_place(order, out),
            OrderRequest::Cancel {
                client_order_id, ..
            } => self.handle_cancel(*client_order_id, out),
            OrderRequest::Replace {
                client_order_id,
                new_price,
                new_qty,
                ..
            } => self.handle_replace(*client_order_id, *new_price, *new_qty, out),
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
            if out.len() >= MAX_PASSIVE_FILLS_PER_EVENT {
                break;
            }

            let Some(order) = self.live_orders.get(&client_order_id).cloned() else {
                continue;
            };
            let Some(limit_price) = order.price else {
                continue;
            };
            let Some(fill_price) =
                Self::passive_fill_price(order.side, limit_price, best_bid, best_ask)
            else {
                continue;
            };

            self.live_orders.remove(&client_order_id);
            out.push(ExecutionReport {
                client_order_id,
                status: OrderStatus::Filled,
                filled_qty: order.qty,
                last_fill_price: fill_price,
                fee_ticks: self.maker_fee_ticks,
                ts_ns: self.next_ts(),
                symbol: order.symbol,
                side: order.side,
            });
        }
    }
}
