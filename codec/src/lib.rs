use thiserror::Error;

#[cfg(feature = "bin")]
use serde::{Deserialize, Serialize};

use lob_core::MarketEvent;
#[cfg(feature = "bin")]
use lob_core::{LevelUpdate, Price, Qty, Symbol};

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("empty input line")]
    EmptyLine,
    #[error("json decode error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("binary format support disabled; enable codec/bin feature")]
    BinaryUnsupported,
    #[error("binary record too short")]
    BinaryRecordTooShort,
    #[error("binary length mismatch: expected {expected} bytes, got {actual}")]
    BinaryLengthMismatch { expected: usize, actual: usize },
    #[error("binary payload too large: {0}")]
    BinaryLengthOverflow(usize),
    #[cfg(feature = "bin")]
    #[error("binary codec error: {0}")]
    Binary(#[from] bincode::Error),
}

pub fn encode_event_json_line(event: &MarketEvent) -> String {
    serde_json::to_string(event).expect("MarketEvent serialization should not fail")
}

pub fn decode_event_json_line(line: &str) -> Result<MarketEvent, CodecError> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return Err(CodecError::EmptyLine);
    }
    Ok(serde_json::from_str(line)?)
}

pub fn encode_event_bin_record(event: &MarketEvent) -> Result<Vec<u8>, CodecError> {
    #[cfg(feature = "bin")]
    {
        // bincode provides compact serde serialization without JSON parsing overhead.
        let payload = bincode::serialize(&BinMarketEvent::from(event))?;
        let len = u32::try_from(payload.len())
            .map_err(|_| CodecError::BinaryLengthOverflow(payload.len()))?;
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = event;
        Err(CodecError::BinaryUnsupported)
    }
}

pub fn decode_event_bin_record(record: &[u8]) -> Result<MarketEvent, CodecError> {
    #[cfg(feature = "bin")]
    {
        if record.len() < 4 {
            return Err(CodecError::BinaryRecordTooShort);
        }
        let len = u32::from_le_bytes([record[0], record[1], record[2], record[3]]) as usize;
        let actual = record.len() - 4;
        if actual != len {
            return Err(CodecError::BinaryLengthMismatch {
                expected: len,
                actual,
            });
        }
        decode_event_bin_payload(&record[4..])
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = record;
        Err(CodecError::BinaryUnsupported)
    }
}

pub fn decode_event_bin_payload(payload: &[u8]) -> Result<MarketEvent, CodecError> {
    #[cfg(feature = "bin")]
    {
        let event: BinMarketEvent = bincode::deserialize(payload)?;
        Ok(event.into())
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = payload;
        Err(CodecError::BinaryUnsupported)
    }
}

#[cfg(feature = "bin")]
#[derive(Debug, Serialize, Deserialize)]
enum BinMarketEvent {
    L2Delta {
        ts_ns: u64,
        symbol: Symbol,
        updates: Vec<LevelUpdate>,
    },
    L2Snapshot {
        ts_ns: u64,
        symbol: Symbol,
        bids: Vec<(Price, Qty)>,
        asks: Vec<(Price, Qty)>,
    },
}

#[cfg(feature = "bin")]
impl From<&MarketEvent> for BinMarketEvent {
    fn from(event: &MarketEvent) -> Self {
        match event {
            MarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => BinMarketEvent::L2Delta {
                ts_ns: *ts_ns,
                symbol: symbol.clone(),
                updates: updates.clone(),
            },
            MarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => BinMarketEvent::L2Snapshot {
                ts_ns: *ts_ns,
                symbol: symbol.clone(),
                bids: bids.clone(),
                asks: asks.clone(),
            },
        }
    }
}

#[cfg(feature = "bin")]
impl From<BinMarketEvent> for MarketEvent {
    fn from(event: BinMarketEvent) -> Self {
        match event {
            BinMarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => MarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            },
            BinMarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => MarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side, Symbol};

    #[test]
    fn round_trip_json_line() {
        let event = MarketEvent::L2Delta {
            ts_ns: 42,
            symbol: Symbol::new("BTC-USD").unwrap(),
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(3).unwrap(),
            }],
        };

        let line = encode_event_json_line(&event);
        let decoded = decode_event_json_line(&line).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn round_trip_json_line_snapshot() {
        let event = MarketEvent::L2Snapshot {
            ts_ns: 7,
            symbol: Symbol::new("ETH-USD").unwrap(),
            bids: vec![
                (Price::new(100).unwrap(), Qty::new(2).unwrap()),
                (Price::new(99).unwrap(), Qty::new(1).unwrap()),
            ],
            asks: vec![
                (Price::new(101).unwrap(), Qty::new(3).unwrap()),
                (Price::new(102).unwrap(), Qty::new(4).unwrap()),
            ],
        };

        let line = encode_event_json_line(&event);
        let decoded = decode_event_json_line(&line).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn invalid_line_returns_error() {
        assert!(decode_event_json_line("").is_err());
        assert!(decode_event_json_line("{not-json}").is_err());
    }
}
