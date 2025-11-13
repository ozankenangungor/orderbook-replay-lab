use std::time::Duration;

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, Symbol};
use orderbook::OrderBook;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const BASE_BID: i64 = 100_000;
const BASE_ASK: i64 = 100_001;

fn seed_book(symbol: &Symbol, levels: usize) -> OrderBook {
    let mut book = OrderBook::new(symbol.clone());
    let bids = levels / 2;
    let asks = levels - bids;
    let mut updates = Vec::with_capacity(levels);

    for i in 0..bids {
        updates.push(LevelUpdate {
            side: Side::Bid,
            price: Price::new(BASE_BID - i as i64).unwrap(),
            qty: Qty::new(1).unwrap(),
        });
    }
    for i in 0..asks {
        updates.push(LevelUpdate {
            side: Side::Ask,
            price: Price::new(BASE_ASK + i as i64).unwrap(),
            qty: Qty::new(1).unwrap(),
        });
    }

    book.apply(&MarketEvent::L2Delta {
        ts_ns: 0,
        symbol: symbol.clone(),
        updates,
    });
    book
}

fn generate_events(symbol: &Symbol, levels: usize, updates: usize, seed: u64) -> Vec<MarketEvent> {
    let bids = levels / 2;
    let asks = levels - bids;
    let mut bid_prices: Vec<i64> = (0..bids).map(|i| BASE_BID - i as i64).collect();
    let mut ask_prices: Vec<i64> = (0..asks).map(|i| BASE_ASK + i as i64).collect();
    let mut next_bid = BASE_BID - bids as i64 - 1;
    let mut next_ask = BASE_ASK + asks as i64;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut events = Vec::with_capacity(updates);

    for idx in 0..updates as u64 {
        let is_bid = rng.gen_bool(0.5);
        let action = rng.gen_range(0u8..100);
        let (prices, next_price) = if is_bid {
            (&mut bid_prices, &mut next_bid)
        } else {
            (&mut ask_prices, &mut next_ask)
        };

        let (price_ticks, qty_lots) = if action < 50 {
            if prices.is_empty() {
                let price = *next_price;
                if is_bid {
                    *next_price -= 1;
                } else {
                    *next_price += 1;
                }
                prices.push(price);
                (price, rng.gen_range(1..=10))
            } else {
                let price = prices[rng.gen_range(0..prices.len())];
                (price, rng.gen_range(1..=10))
            }
        } else if action < 75 || prices.is_empty() {
            let price = *next_price;
            if is_bid {
                *next_price -= 1;
            } else {
                *next_price += 1;
            }
            prices.push(price);
            (price, rng.gen_range(1..=10))
        } else {
            let idx = rng.gen_range(0..prices.len());
            let price = prices.swap_remove(idx);
            (price, 0)
        };

        let update = LevelUpdate {
            side: if is_bid { Side::Bid } else { Side::Ask },
            price: Price::new(price_ticks).unwrap(),
            qty: Qty::new(qty_lots).unwrap(),
        };
        events.push(MarketEvent::L2Delta {
            ts_ns: idx,
            symbol: symbol.clone(),
            updates: vec![update],
        });
    }

    events
}

fn bench_orderbook(c: &mut Criterion) {
    let mut group = c.benchmark_group("orderbook_apply");
    for (label, levels, updates) in [("small", 200, 1000), ("medium", 2000, 2000)] {
        let symbol = Symbol::new("BENCH-USD").unwrap();
        let events = generate_events(&symbol, levels, updates, levels as u64);

        group.throughput(Throughput::Elements(events.len() as u64));
        group.bench_with_input(BenchmarkId::new("updates", label), &levels, |b, _| {
            b.iter_batched(
                || seed_book(&symbol, levels),
                |mut book| {
                    for event in &events {
                        book.apply(event);
                    }
                },
                BatchSize::SmallInput,
            );
        });

        group.bench_with_input(
            BenchmarkId::new("updates_with_top_of_book", label),
            &levels,
            |b, _| {
                b.iter_batched(
                    || seed_book(&symbol, levels),
                    |mut book| {
                        for event in &events {
                            book.apply(event);
                            black_box(book.best_bid());
                            black_box(book.best_ask());
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).measurement_time(Duration::from_secs(1));
    targets = bench_orderbook
}
criterion_main!(benches);
