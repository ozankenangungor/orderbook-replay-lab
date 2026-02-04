use std::collections::BTreeMap;

use lob_core::{MarketEvent, Price, Qty, Side, Symbol};

#[derive(Debug, Clone)]
pub struct OrderBook {
    symbol: Symbol,
    bids: BTreeMap<Price, Qty>,
    asks: BTreeMap<Price, Qty>,
}

impl OrderBook {
    pub fn new(symbol: Symbol) -> Self {
        Self {
            symbol,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    pub fn apply(&mut self, delta: &MarketEvent) {
        let MarketEvent::L2Delta {
            symbol, updates, ..
        } = delta;

        if symbol != &self.symbol {
            return;
        }

        for update in updates {
            let book = match update.side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
            };

            if update.qty.is_zero() {
                book.remove(&update.price);
            } else {
                book.insert(update.price, update.qty);
            }
        }
    }

    pub fn best_bid(&self) -> Option<(Price, Qty)> {
        self.bids.iter().next_back().map(|(p, q)| (*p, *q))
    }

    pub fn best_ask(&self) -> Option<(Price, Qty)> {
        self.asks.iter().next().map(|(p, q)| (*p, *q))
    }

    pub fn spread(&self) -> Option<Price> {
        let (ask, _) = self.best_ask()?;
        let (bid, _) = self.best_bid()?;
        let ask_ticks = ask.ticks();
        let bid_ticks = bid.ticks();
        if ask_ticks > bid_ticks {
            Price::new(ask_ticks - bid_ticks).ok()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, Symbol};

    fn delta(symbol: &Symbol, updates: Vec<LevelUpdate>) -> MarketEvent {
        MarketEvent::L2Delta {
            ts_ns: 1,
            symbol: symbol.clone(),
            updates,
        }
    }

    #[test]
    fn insert_update_remove_levels() {
        let symbol = Symbol::new("BTC-USD").unwrap();
        let mut book = OrderBook::new(symbol.clone());

        book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        ));
        assert_eq!(
            book.best_bid(),
            Some((Price::new(100).unwrap(), Qty::new(1).unwrap()))
        );

        book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(5).unwrap(),
            }],
        ));
        assert_eq!(
            book.best_bid(),
            Some((Price::new(100).unwrap(), Qty::new(5).unwrap()))
        );

        book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(0).unwrap(),
            }],
        ));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn best_bid_ask_correctness() {
        let symbol = Symbol::new("ETH-USD").unwrap();
        let mut book = OrderBook::new(symbol.clone());

        book.apply(&delta(
            &symbol,
            vec![
                LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(100).unwrap(),
                    qty: Qty::new(1).unwrap(),
                },
                LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(101).unwrap(),
                    qty: Qty::new(2).unwrap(),
                },
                LevelUpdate {
                    side: Side::Ask,
                    price: Price::new(105).unwrap(),
                    qty: Qty::new(3).unwrap(),
                },
                LevelUpdate {
                    side: Side::Ask,
                    price: Price::new(104).unwrap(),
                    qty: Qty::new(4).unwrap(),
                },
            ],
        ));

        assert_eq!(
            book.best_bid(),
            Some((Price::new(101).unwrap(), Qty::new(2).unwrap()))
        );
        assert_eq!(
            book.best_ask(),
            Some((Price::new(104).unwrap(), Qty::new(4).unwrap()))
        );
        assert_eq!(book.spread(), Some(Price::new(3).unwrap()));
    }

    #[test]
    fn best_bid_less_than_best_ask_invariant() {
        let symbol = Symbol::new("SOL-USD").unwrap();
        let mut book = OrderBook::new(symbol.clone());

        book.apply(&delta(
            &symbol,
            vec![
                LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(99).unwrap(),
                    qty: Qty::new(1).unwrap(),
                },
                LevelUpdate {
                    side: Side::Ask,
                    price: Price::new(101).unwrap(),
                    qty: Qty::new(1).unwrap(),
                },
            ],
        ));

        let (bid, _) = book.best_bid().unwrap();
        let (ask, _) = book.best_ask().unwrap();
        assert!(bid.ticks() < ask.ticks());
    }
}
