use std::path::Path;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use lob_core::Symbol;
use metrics::{LatencyStats, ThroughputTracker};
use orderbook::OrderBook;
use replay::ReplayReader;

#[derive(Parser)]
#[command(name = "rust-latency-lob", version, about = "Low-latency LOB tools")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Replay {
        #[arg(long)]
        input: std::path::PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long)]
        limit: Option<u64>,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Replay {
            input,
            symbol,
            limit,
        } => run_replay(&input, &symbol, limit),
    }
}

fn run_replay(
    input: &Path,
    symbol: &str,
    limit: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let symbol = Symbol::new(symbol)?;
    let mut reader = ReplayReader::open(input)?;
    let mut book = OrderBook::new(symbol.clone());
    let mut latency = LatencyStats::new();
    let mut throughput = ThroughputTracker::new(Duration::from_secs(1));

    let start = Instant::now();
    let mut count = 0u64;

    while let Some(event) = reader.next_event()? {
        let t0 = Instant::now();
        book.apply(&event);
        let ns = t0.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let ns = ns.max(1);
        latency.record(ns);
        throughput.record(1);
        count += 1;

        if let Some(limit) = limit {
            if count >= limit {
                break;
            }
        }
    }

    let elapsed = start.elapsed();
    let avg_throughput = if elapsed.as_secs_f64() > 0.0 {
        count as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let throughput = throughput.events_per_sec().unwrap_or(avg_throughput);

    let best_bid = book
        .best_bid()
        .map(|(price, qty)| format!("{}@{}", price.ticks(), qty.lots()))
        .unwrap_or_else(|| "None".to_string());
    let best_ask = book
        .best_ask()
        .map(|(price, qty)| format!("{}@{}", price.ticks(), qty.lots()))
        .unwrap_or_else(|| "None".to_string());

    println!("count={}", count);
    println!("throughput={:.2} events/sec", throughput);
    println!("latency={}", latency.summary_string());
    println!("best_bid={} best_ask={}", best_bid, best_ask);

    Ok(())
}
