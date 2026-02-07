use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, Symbol};
use metrics::{LatencyStats, ThroughputTracker};
use orderbook::OrderBook;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use replay::ReplayReader;

const GEN_SEED_DEFAULT: u64 = 42;

#[derive(Parser)]
#[command(
    name = "orderbook-replay-lab-rs",
    version,
    about = "Low-latency LOB tools"
)]
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
    Gen {
        #[arg(long)]
        output: std::path::PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long)]
        events: u64,
        #[arg(long, default_value_t = GEN_SEED_DEFAULT)]
        seed: u64,
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
        Commands::Gen {
            output,
            symbol,
            events,
            seed,
        } => run_gen(&output, &symbol, events, seed),
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
    let mut total_events_read = 0u64;
    let mut events_applied = 0u64;
    let mut events_dropped = 0u64;

    while let Some(event) = reader.next_event()? {
        total_events_read += 1;
        let t0 = Instant::now();
        let applied = book.apply(&event);
        if applied {
            let ns = t0.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let ns = ns.max(1);
            latency.record(ns);
            throughput.record(1);
            events_applied += 1;
        } else {
            events_dropped += 1;
        }

        if let Some(limit) = limit {
            if total_events_read >= limit {
                break;
            }
        }
    }

    let elapsed = start.elapsed();
    let avg_throughput = if elapsed.as_secs_f64() > 0.0 {
        events_applied as f64 / elapsed.as_secs_f64()
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

    println!("total_events_read={}", total_events_read);
    println!("events_applied={}", events_applied);
    println!("events_dropped={}", events_dropped);
    println!("throughput={:.2} events/sec", throughput);
    println!("latency={}", latency.summary_string());
    println!("best_bid={} best_ask={}", best_bid, best_ask);

    Ok(())
}

fn run_gen(
    output: &Path,
    symbol: &str,
    events: u64,
    seed: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let symbol = Symbol::new(symbol)?;
    let file = std::fs::File::create(output)?;
    let mut writer = BufWriter::new(file);
    let mut rng = StdRng::seed_from_u64(seed);
    let mut mid: i64 = 100_000;

    for idx in 0..events {
        let drift: i64 = rng.gen_range(-1..=1);
        mid = (mid + drift).max(1);
        let side = if rng.gen_bool(0.5) {
            Side::Bid
        } else {
            Side::Ask
        };
        let offset: i64 = rng.gen_range(1..=5);
        let price_ticks = match side {
            Side::Bid => (mid - offset).max(1),
            Side::Ask => mid + offset,
        };
        let remove = rng.gen_bool(0.1);
        let qty_lots: i64 = if remove { 0 } else { rng.gen_range(1..=10) };

        let update = LevelUpdate {
            side,
            price: Price::new(price_ticks)?,
            qty: Qty::new(qty_lots)?,
        };
        let event = MarketEvent::L2Delta {
            ts_ns: idx,
            symbol: symbol.clone(),
            updates: vec![update],
        };
        let line = codec::encode_event_json_line(&event);
        writeln!(writer, "{}", line)?;
    }

    writer.flush()?;
    println!("generated={} output={}", events, output.display());
    Ok(())
}
