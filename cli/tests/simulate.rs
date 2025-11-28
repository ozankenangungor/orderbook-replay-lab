use std::fs::File;
use std::io::Write;
use std::process::Command;

use codec::encode_event_json_line;
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, SymbolTable};
use tempfile::tempdir;

#[test]
fn simulate_noop_outputs_stats() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("events.log");
    let mut symbols = SymbolTable::new();
    let symbol = symbols.try_intern("BTC-USD").expect("symbol");
    let other_symbol = symbols.try_intern("ETH-USD").expect("symbol");

    let events = vec![
        MarketEvent::L2Snapshot {
            ts_ns: 1,
            symbol,
            bids: vec![(Price::new(100).unwrap(), Qty::new(1).unwrap())],
            asks: vec![(Price::new(101).unwrap(), Qty::new(1).unwrap())],
        },
        MarketEvent::L2Delta {
            ts_ns: 2,
            symbol,
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(2).unwrap(),
            }],
        },
        MarketEvent::L2Delta {
            ts_ns: 3,
            symbol: other_symbol,
            updates: vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(999).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        },
    ];

    let mut file = File::create(&path).expect("create log");
    for event in events {
        writeln!(
            file,
            "{}",
            encode_event_json_line(&event, &symbols).expect("encode log")
        )
        .expect("write log");
    }

    let exe = env!("CARGO_BIN_EXE_orderbook-replay-lab-rs");
    let output = Command::new(exe)
        .args([
            "simulate",
            "--input",
            path.to_str().expect("path str"),
            "--symbol",
            "BTC-USD",
            "--strategy",
            "noop",
        ])
        .output()
        .expect("run cli");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains("events_read=3"));
    assert!(stdout.contains("events_applied_to_book=2"));
    assert!(stdout.contains("orders_sent=0"));
    assert!(stdout.contains("fills_count=0"));
    assert!(stdout.contains("final_position_lots=0"));
    assert!(stdout.contains("realized_pnl_ticks=0"));
    assert!(stdout.contains("fees_paid_ticks=0"));
    assert!(stdout.contains("throughput_windowed="));
    assert!(stdout.contains("throughput_overall="));
    assert!(stdout.contains("latency="));
}
