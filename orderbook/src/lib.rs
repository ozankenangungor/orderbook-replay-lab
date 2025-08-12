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

    pub fn apply(&mut self, event: &MarketEvent) -> bool {
        match event {
            MarketEvent::L2Delta {
                symbol, updates, ..
            } => {
                if symbol != &self.symbol {
                    return false;
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
                true
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
    use std::collections::BTreeMap;

    use proptest::prelude::*;

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

        assert!(book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        )));
        assert_eq!(
            book.best_bid(),
            Some((Price::new(100).unwrap(), Qty::new(1).unwrap()))
        );

        assert!(book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(5).unwrap(),
            }],
        )));
        assert_eq!(
            book.best_bid(),
            Some((Price::new(100).unwrap(), Qty::new(5).unwrap()))
        );

        assert!(book.apply(&delta(
            &symbol,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(0).unwrap(),
            }],
        )));
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn best_bid_ask_correctness() {
        let symbol = Symbol::new("ETH-USD").unwrap();
        let mut book = OrderBook::new(symbol.clone());

        assert!(book.apply(&delta(
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
        )));

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

        assert!(book.apply(&delta(
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
        )));

        let (bid, _) = book.best_bid().unwrap();
        let (ask, _) = book.best_ask().unwrap();
        assert!(bid.ticks() < ask.ticks());
    }

    fn update_strategy() -> impl Strategy<Value = (bool, i64, i64)> {
        any::<bool>().prop_flat_map(|is_bid| {
            // Keep bid/ask price ranges disjoint so the strict invariant holds.
            let price_range = if is_bid { 1i64..=99 } else { 101i64..=200 };
            (Just(is_bid), price_range, 0i64..=5)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            max_shrink_iters: 256,
            .. ProptestConfig::default()
        })]

        #[test]
        fn prop_best_levels_follow_updates(
            updates in proptest::collection::vec(update_strategy(), 0..=128),
        ) {
            let symbol = Symbol::new("PROP-USD").unwrap();
            let mut book = OrderBook::new(symbol.clone());
            let mut bids: BTreeMap<i64, i64> = BTreeMap::new();
            let mut asks: BTreeMap<i64, i64> = BTreeMap::new();

            for (is_bid, price_ticks, qty_lots) in updates {
                let side = if is_bid { Side::Bid } else { Side::Ask };
                let update = LevelUpdate {
                    side,
                    price: Price::new(price_ticks).unwrap(),
                    qty: Qty::new(qty_lots).unwrap(),
                };
                let applied = book.apply(&MarketEvent::L2Delta {
                    ts_ns: 1,
                    symbol: symbol.clone(),
                    updates: vec![update],
                });
                prop_assert!(applied);

                let book_side = if is_bid { &mut bids } else { &mut asks };
                if qty_lots == 0 {
                    book_side.remove(&price_ticks);
                } else {
                    book_side.insert(price_ticks, qty_lots);
                }
            }

            let ref_best_bid = bids.iter().next_back().map(|(p, q)| (*p, *q));
            let ref_best_ask = asks.iter().next().map(|(p, q)| (*p, *q));

            let best_bid = book
                .best_bid()
                .map(|(price, qty)| (price.ticks(), qty.lots()));
            let best_ask = book
                .best_ask()
                .map(|(price, qty)| (price.ticks(), qty.lots()));

            prop_assert_eq!(best_bid, ref_best_bid);
            prop_assert_eq!(best_ask, ref_best_ask);

            if let (Some((bid, _)), Some((ask, _))) = (best_bid, best_ask) {
                prop_assert!(bid < ask);
            }
        }
    }
}
