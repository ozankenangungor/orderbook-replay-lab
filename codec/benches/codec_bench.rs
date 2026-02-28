use criterion::{black_box, criterion_group, criterion_main, Criterion};
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, SymbolId, SymbolTable};

fn sample_delta(symbol: SymbolId, levels: i64) -> MarketEvent {
    let mut updates = Vec::with_capacity(levels as usize);
    for i in 0..levels {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let price_ticks = if side == Side::Bid {
            1000 - i
        } else {
            1001 + i
        };
        let qty_lots = (i % 8) + 1;
        updates.push(LevelUpdate {
            side,
            price: Price::new(price_ticks).expect("price"),
            qty: Qty::new(qty_lots).expect("qty"),
        });
    }
    MarketEvent::L2Delta {
        ts_ns: 1,
        symbol,
        updates,
    }
}

fn sample_snapshot(symbol: SymbolId, levels: i64) -> MarketEvent {
    let mut bids = Vec::with_capacity(levels as usize);
    let mut asks = Vec::with_capacity(levels as usize);
    for i in 0..levels {
        bids.push((
            Price::new(1000 - i).expect("price"),
            Qty::new(1 + i).expect("qty"),
        ));
        asks.push((
            Price::new(1001 + i).expect("price"),
            Qty::new(1 + i).expect("qty"),
        ));
    }
    MarketEvent::L2Snapshot {
        ts_ns: 1,
        symbol,
        bids,
        asks,
    }
}

fn bench_json(c: &mut Criterion) {
    let symbol = SymbolId::from_u32(0);
    let symbols = SymbolTable::try_from_symbols(["BTC-USD"]).expect("symbol table");
    let delta = sample_delta(symbol, 32);
    let snapshot = sample_snapshot(symbol, 16);
    let line = codec::encode_event_json_line(&delta, &symbols).expect("encode json");

    c.bench_function("codec/json_encode_delta_32", |b| {
        b.iter(|| {
            codec::encode_event_json_line(black_box(&delta), black_box(&symbols)).expect("encode")
        })
    });
    c.bench_function("codec/json_encode_snapshot_16", |b| {
        b.iter(|| {
            codec::encode_event_json_line(black_box(&snapshot), black_box(&symbols))
                .expect("encode")
        })
    });

    let mut decode_symbols = SymbolTable::try_from_symbols(["BTC-USD"]).expect("symbol table");
    c.bench_function("codec/json_decode_delta_32", |b| {
        b.iter(|| {
            codec::decode_event_json_line(black_box(&line), black_box(&mut decode_symbols))
                .expect("decode")
        })
    });
}

#[cfg(feature = "bin")]
fn bench_bin(c: &mut Criterion) {
    let symbol = SymbolId::from_u32(0);
    let symbols = SymbolTable::try_from_symbols(["BTC-USD"]).expect("symbol table");
    let delta = sample_delta(symbol, 32);
    let record = codec::encode_event_bin_record(&delta, &symbols).expect("encode bin");

    c.bench_function("codec/bin_encode_delta_32", |b| {
        b.iter(|| {
            codec::encode_event_bin_record(black_box(&delta), black_box(&symbols)).expect("encode")
        })
    });

    let mut decode_symbols = SymbolTable::try_from_symbols(["BTC-USD"]).expect("symbol table");
    c.bench_function("codec/bin_decode_delta_32", |b| {
        b.iter(|| {
            codec::decode_event_bin_record(black_box(&record), black_box(&mut decode_symbols))
                .expect("decode")
        })
    });
}

fn bench_codec(c: &mut Criterion) {
    bench_json(c);
    #[cfg(feature = "bin")]
    bench_bin(c);
}

criterion_group!(benches, bench_codec);
criterion_main!(benches);
