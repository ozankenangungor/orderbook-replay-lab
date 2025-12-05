use std::cell::RefCell;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand, ValueEnum};
use engine::Engine;
use lob_core::{LevelUpdate, MarketEvent, Price, Qty, Side, SymbolId, SymbolTable};
use metrics::{LatencyStats, ThroughputTracker};
use oms::Oms;
use orderbook::OrderBook;
use portfolio::Portfolio;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use replay::ReplayReader;
use risk::RiskEngine;
use strategies::{MmStrategy, NoopStrategy, TwapStrategy};
use trading_types::OrderStatus;
use venue::ExecutionVenue;
use venue_sim::SimVenue;

const GEN_SEED_DEFAULT: u64 = 42;
const SIM_TIMER_INTERVAL_NS_DEFAULT: u64 = 1_000_000_000;
const MAX_TIMER_TICKS_PER_EVENT: usize = 1024;

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

#[derive(Copy, Clone, Debug, ValueEnum)]
enum LogFormat {
    Jsonl,
    Bin,
}

#[derive(Clone, Copy, Debug)]
struct SimulateStrategyConfig {
    twap_target: i64,
    twap_horizon: u64,
    twap_slice: i64,
    mm_half_spread: i64,
    mm_qty: i64,
    mm_skew_per_lot: i64,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum StrategyKind {
    Noop,
    Twap,
    Mm,
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
        #[arg(long, value_enum, default_value_t = LogFormat::Jsonl)]
        format: LogFormat,
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
        #[arg(long)]
        snapshot_first: bool,
        #[arg(long, value_enum, default_value_t = LogFormat::Jsonl)]
        format: LogFormat,
    },
    Simulate {
        #[arg(long)]
        input: std::path::PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long, value_enum, default_value_t = StrategyKind::Noop)]
        strategy: StrategyKind,
        #[arg(long, default_value_t = 10)]
        twap_target: i64,
        #[arg(long, default_value_t = 60)]
        twap_horizon: u64,
        #[arg(long, default_value_t = 1)]
        twap_slice: i64,
        #[arg(long, default_value_t = 1)]
        mm_half_spread: i64,
        #[arg(long, default_value_t = 1)]
        mm_qty: i64,
        #[arg(long, default_value_t = 1)]
        mm_skew_per_lot: i64,
        #[arg(long)]
        limit: Option<u64>,
        #[arg(long, default_value_t = SIM_TIMER_INTERVAL_NS_DEFAULT)]
        timer_interval_ns: u64,
        #[arg(long, value_enum, default_value_t = LogFormat::Jsonl)]
        format: LogFormat,
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
            format,
        } => run_replay(&input, &symbol, limit, format),
        Commands::Gen {
            output,
            symbol,
            events,
            seed,
            snapshot_first,
            format,
        } => run_gen(&output, &symbol, events, seed, snapshot_first, format),
        Commands::Simulate {
            input,
            symbol,
            strategy,
            twap_target,
            twap_horizon,
            twap_slice,
            mm_half_spread,
            mm_qty,
            mm_skew_per_lot,
            limit,
            timer_interval_ns,
            format,
        } => {
            let config = SimulateStrategyConfig {
                twap_target,
                twap_horizon,
                twap_slice,
                mm_half_spread,
                mm_qty,
                mm_skew_per_lot,
            };
            run_simulate(
                &input,
                &symbol,
                strategy,
                &config,
                limit,
                timer_interval_ns,
                format,
            )
        }
    }
}

fn run_replay(
    input: &Path,
    symbol: &str,
    limit: Option<u64>,
    format: LogFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let symbol_id = SymbolId::from_u32(0);
    let format = match format {
        LogFormat::Jsonl => replay::ReplayFormat::Jsonl,
        LogFormat::Bin => replay::ReplayFormat::Bin,
    };
    let mut reader =
        ReplayReader::open_with_format_and_predeclared_symbols(input, format, [symbol])?;
    let mut book = OrderBook::new(symbol_id);
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
    // Windowed throughput uses the recent tracker window; overall is total applied / elapsed.
    let throughput_windowed = throughput.events_per_sec().unwrap_or(0.0);
    let throughput_overall = if elapsed.as_secs_f64() > 0.0 {
        events_applied as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

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
    println!("throughput_windowed={:.2} events/sec", throughput_windowed);
    println!("throughput_overall={:.2} events/sec", throughput_overall);
    println!("latency={}", latency.summary_string());
    println!("best_bid={} best_ask={}", best_bid, best_ask);

    Ok(())
}

fn run_gen(
    output: &Path,
    symbol: &str,
    events: u64,
    seed: u64,
    snapshot_first: bool,
    format: LogFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let symbols = SymbolTable::try_from_symbols([symbol])?;
    let symbol = SymbolId::from_u32(0);
    let file = std::fs::File::create(output)?;
    let mut writer = BufWriter::new(file);
    let mut rng = StdRng::seed_from_u64(seed);
    let mut mid: i64 = 100_000;
    let mut ts_ns = 0u64;

    if snapshot_first {
        let mut bids = Vec::with_capacity(5);
        let mut asks = Vec::with_capacity(5);
        for level in 1..=5i64 {
            let bid_price = (mid - level).max(1);
            let ask_price = mid + level;
            let bid_qty = rng.gen_range(1..=10);
            let ask_qty = rng.gen_range(1..=10);
            bids.push((Price::new(bid_price)?, Qty::new(bid_qty)?));
            asks.push((Price::new(ask_price)?, Qty::new(ask_qty)?));
        }

        let snapshot = MarketEvent::L2Snapshot {
            ts_ns,
            symbol,
            bids,
            asks,
        };
        write_event(&mut writer, &snapshot, format, &symbols)?;
        ts_ns += 1;
    }

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
            ts_ns: ts_ns + idx,
            symbol,
            updates: vec![update],
        };
        write_event(&mut writer, &event, format, &symbols)?;
    }

    writer.flush()?;
    println!("generated={} output={}", events, output.display());
    Ok(())
}

fn run_simulate(
    input: &Path,
    symbol: &str,
    strategy: StrategyKind,
    config: &SimulateStrategyConfig,
    limit: Option<u64>,
    timer_interval_ns: u64,
    format: LogFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let symbol_id = SymbolId::from_u32(0);
    let format = match format {
        LogFormat::Jsonl => replay::ReplayFormat::Jsonl,
        LogFormat::Bin => replay::ReplayFormat::Bin,
    };

    let mut reader =
        ReplayReader::open_with_format_and_predeclared_symbols(input, format, [symbol])?;
    let shared_book = Rc::new(RefCell::new(OrderBook::new(symbol_id)));
    let sim_venue = SimVenue::new(shared_book.clone(), 0, 0);
    let counters = Rc::new(RefCell::new(VenueCounters::default()));
    let venue = CountingVenue::new(sim_venue, counters.clone());

    let mut engine = Engine::with_shared_book(
        shared_book,
        Portfolio::new(),
        Oms::new(),
        RiskEngine::new(),
        make_strategy(strategy, config),
        Box::new(venue),
    );

    let mut throughput = ThroughputTracker::new(Duration::from_secs(1));
    let start = Instant::now();
    let mut events_read = 0u64;
    let mut events_applied = 0u64;
    let timer_interval_ns = timer_interval_ns.max(1);
    let mut last_tick_ts_ns: Option<u64> = None;

    while let Some(event) = reader.next_event()? {
        let event_ts_ns = event_ts_ns(&event);
        if let Some(mut last_tick) = last_tick_ts_ns {
            let mut ticks_processed = 0usize;
            while event_ts_ns.saturating_sub(last_tick) >= timer_interval_ns {
                if ticks_processed >= MAX_TIMER_TICKS_PER_EVENT {
                    debug_assert!(
                        false,
                        "timer processing exceeded MAX_TIMER_TICKS_PER_EVENT; stopping to prevent churn"
                    );
                    break;
                }
                last_tick = last_tick.saturating_add(timer_interval_ns);
                engine.on_timer(last_tick, symbol_id);
                ticks_processed += 1;
            }
            last_tick_ts_ns = Some(last_tick);
        } else {
            last_tick_ts_ns = Some(event_ts_ns);
        }

        events_read += 1;
        if engine.on_market_event(&event) {
            events_applied += 1;
            throughput.record(1);
        }

        if let Some(limit) = limit {
            if events_read >= limit {
                break;
            }
        }
    }

    let elapsed = start.elapsed();
    let throughput_windowed = throughput.events_per_sec().unwrap_or(0.0);
    let throughput_overall = if elapsed.as_secs_f64() > 0.0 {
        events_applied as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    let counts = counters.borrow();
    println!("events_read={}", events_read);
    println!("events_applied_to_book={}", events_applied);
    println!("orders_sent={}", counts.orders_sent);
    println!("fills_count={}", counts.fills_count);
    println!("final_position_lots={}", engine.position_lots(symbol_id));
    println!(
        "realized_pnl_ticks={}",
        engine.realized_pnl_ticks(symbol_id)
    );
    println!("fees_paid_ticks={}", engine.fees_paid_ticks(symbol_id));
    println!("throughput_windowed={:.2} events/sec", throughput_windowed);
    println!("throughput_overall={:.2} events/sec", throughput_overall);
    println!("latency={}", engine.latency_stats().summary_string());

    Ok(())
}

fn make_strategy(
    strategy: StrategyKind,
    config: &SimulateStrategyConfig,
) -> Box<dyn strategy_api::Strategy> {
    match strategy {
        StrategyKind::Noop => Box::new(NoopStrategy),
        StrategyKind::Twap => Box::new(TwapStrategy::new(
            config.twap_target,
            config.twap_horizon,
            config.twap_slice,
        )),
        StrategyKind::Mm => Box::new(MmStrategy::new(
            config.mm_half_spread,
            config.mm_qty,
            config.mm_skew_per_lot,
        )),
    }
}

#[derive(Default)]
struct VenueCounters {
    orders_sent: u64,
    fills_count: u64,
}

struct CountingVenue<V: ExecutionVenue> {
    inner: V,
    counters: Rc<RefCell<VenueCounters>>,
}

impl<V: ExecutionVenue> CountingVenue<V> {
    fn new(inner: V, counters: Rc<RefCell<VenueCounters>>) -> Self {
        Self { inner, counters }
    }
}

impl<V: ExecutionVenue> ExecutionVenue for CountingVenue<V> {
    fn submit(&mut self, req: &oms::OrderRequest, out: &mut Vec<trading_types::ExecutionReport>) {
        {
            let mut counters = self.counters.borrow_mut();
            counters.orders_sent += 1;
        }

        self.inner.submit(req, out);
        let fills = out
            .iter()
            .filter(|report| {
                matches!(
                    report.status,
                    OrderStatus::Filled | OrderStatus::PartiallyFilled
                ) && !report.filled_qty.is_zero()
            })
            .count() as u64;

        if fills > 0 {
            let mut counters = self.counters.borrow_mut();
            counters.fills_count += fills;
        }
    }
}

fn write_event(
    writer: &mut BufWriter<std::fs::File>,
    event: &MarketEvent,
    format: LogFormat,
    symbols: &SymbolTable,
) -> Result<(), Box<dyn std::error::Error>> {
    match format {
        LogFormat::Jsonl => {
            let line = codec::encode_event_json_line(event, symbols)?;
            writeln!(writer, "{}", line)?;
        }
        LogFormat::Bin => {
            let record = codec::encode_event_bin_record(event, symbols)?;
            writer.write_all(&record)?;
        }
    }
    Ok(())
}

fn event_ts_ns(event: &MarketEvent) -> u64 {
    match event {
        MarketEvent::L2Delta { ts_ns, .. } => *ts_ns,
        MarketEvent::L2Snapshot { ts_ns, .. } => *ts_ns,
    }
}
