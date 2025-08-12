use std::fs::File;
use std::io::Write;
use std::process::Command;

use codec::encode_event_json_line;
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, Symbol};
use tempfile::tempdir;

fn event(symbol: &Symbol, ts_ns: u64, updates: Vec<LevelUpdate>) -> MarketEvent {
    MarketEvent::L2Delta {
        ts_ns,
        symbol: symbol.clone(),
        updates,
    }
}

#[test]
fn replay_command_processes_events() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("events.log");
    let symbol = Symbol::new("BTC-USD").expect("symbol");
    let other_symbol = Symbol::new("ETH-USD").expect("symbol");

    let events = vec![
        event(
            &symbol,
            1,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        ),
        event(
            &symbol,
            2,
            vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(105).unwrap(),
                qty: Qty::new(2).unwrap(),
            }],
        ),
        event(
            &symbol,
            3,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(102).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        ),
        event(
            &symbol,
            4,
            vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(103).unwrap(),
                qty: Qty::new(2).unwrap(),
            }],
        ),
        event(
            &symbol,
            5,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(0).unwrap(),
            }],
        ),
        event(
            &other_symbol,
            6,
            vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(999).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        ),
    ];

    let mut file = File::create(&path).expect("create log");
    for event in events {
        writeln!(file, "{}", encode_event_json_line(&event)).expect("write log");
    }

    let exe = env!("CARGO_BIN_EXE_orderbook-replay-lab-rs");
    let output = Command::new(exe)
        .args([
            "replay",
            "--input",
            path.to_str().expect("path str"),
            "--symbol",
            "BTC-USD",
        ])
        .output()
        .expect("run cli");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains("total_events_read=6"));
    assert!(stdout.contains("events_applied=5"));
    assert!(stdout.contains("events_dropped=1"));
    assert!(stdout.contains("throughput="));
    assert!(stdout.contains("latency="));
    assert!(stdout.contains("best_bid=102@1"));
    assert!(stdout.contains("best_ask=103@2"));
}
