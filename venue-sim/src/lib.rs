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
    order_scan_ids: Vec<ClientOrderId>,
    fill_candidates: Vec<(ClientOrderId, Price)>,
}

impl SimVenue {
    pub fn new(book: Rc<RefCell<OrderBook>>, maker_fee_ticks: i64, taker_fee_ticks: i64) -> Self {
        Self {
            book,
            maker_fee_ticks,
            taker_fee_ticks,
            next_ts_ns: 1,
            live_orders: HashMap::new(),
            order_scan_ids: Vec::new(),
            fill_candidates: Vec::new(),
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

        self.order_scan_ids.clear();
        self.order_scan_ids.extend(self.live_orders.keys().copied());
        self.order_scan_ids.sort_unstable_by_key(|id| id.0);

        self.fill_candidates.clear();
        for client_order_id in &self.order_scan_ids {
            if self.fill_candidates.len() >= MAX_PASSIVE_FILLS_PER_EVENT {
                break;
            }
            let Some(order) = self.live_orders.get(client_order_id) else {
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
            self.fill_candidates.push((*client_order_id, fill_price));
        }

        let mut fill_candidates = std::mem::take(&mut self.fill_candidates);
        for (client_order_id, fill_price) in fill_candidates.drain(..) {
            let Some(order) = self.live_orders.remove(&client_order_id) else {
                continue;
            };
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
        self.fill_candidates = fill_candidates;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{LevelUpdate, MarketEvent};
    use oms::OrderRequest as OmsOrderRequest;
    use trading_types::{OrderRequest as NewOrderRequest, TimeInForce};

    fn place_req(
        client_order_id: u64,
        symbol: SymbolId,
        side: Side,
        price_ticks: i64,
        qty_lots: i64,
    ) -> OmsOrderRequest {
        OmsOrderRequest::Place(NewOrderRequest {
            client_order_id: ClientOrderId(client_order_id),
            symbol,
            side,
            order_type: OrderType::Limit,
            price: Some(Price::new(price_ticks).expect("price")),
            qty: Qty::new(qty_lots).expect("qty"),
            tif: TimeInForce::Gtc,
        })
    }

    #[test]
    fn passive_fills_are_emitted_in_client_order_id_order() {
        let symbol = SymbolId::from_u32(1);
        let book = Rc::new(RefCell::new(OrderBook::new(symbol)));
        let mut venue = SimVenue::new(book.clone(), 0, 0);

        assert!(book.borrow_mut().apply(&MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol,
            bids: vec![(Price::new(99).expect("price"), Qty::new(1).expect("qty"))],
            asks: vec![(Price::new(110).expect("price"), Qty::new(1).expect("qty"))],
        }));

        let mut out = Vec::new();
        venue.submit(&place_req(20, symbol, Side::Bid, 105, 1), &mut out);
        venue.submit(&place_req(10, symbol, Side::Bid, 106, 1), &mut out);
        out.clear();

        assert!(book.borrow_mut().apply(&MarketEvent::L2Delta {
            ts_ns: 2,
            symbol,
            updates: vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(104).expect("price"),
                qty: Qty::new(1).expect("qty"),
            }],
        }));

        venue.on_book_update(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].client_order_id, ClientOrderId(10));
        assert_eq!(out[1].client_order_id, ClientOrderId(20));
        assert!(out.iter().all(|r| r.status == OrderStatus::Filled));
    }
}
