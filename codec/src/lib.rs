use thiserror::Error;

#[cfg(feature = "bin")]
use serde::{Deserialize, Serialize};

use lob_core::MarketEvent;
#[cfg(feature = "bin")]
use lob_core::{LevelUpdate, Price, Qty, Symbol};

pub const BIN_RECORD_MAGIC: [u8; 4] = *b"LOB2";
pub const BIN_RECORD_VERSION: u8 = 1;
pub const BIN_RECORD_HEADER_LEN: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinRecordHeader {
    pub payload_len: usize,
    pub checksum: u32,
}

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
    #[error("binary record magic mismatch: got {0:?}")]
    BinaryMagicMismatch([u8; 4]),
    #[error("unsupported binary record version: {0}")]
    BinaryUnsupportedVersion(u8),
    #[error("binary length mismatch: expected {expected} bytes, got {actual}")]
    BinaryLengthMismatch { expected: usize, actual: usize },
    #[error("binary checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    BinaryChecksumMismatch { expected: u32, actual: u32 },
    #[error("binary payload too large: {0}")]
    BinaryLengthOverflow(usize),
    #[cfg(feature = "bin")]
    #[error("binary codec error: {0}")]
    Binary(#[from] bincode::Error),
}

pub fn encode_event_json_line(event: &MarketEvent) -> Result<String, CodecError> {
    Ok(serde_json::to_string(event)?)
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
        let checksum = crc32fast::hash(&payload);
        let mut out = Vec::with_capacity(BIN_RECORD_HEADER_LEN + payload.len());
        out.extend_from_slice(&BIN_RECORD_MAGIC);
        out.push(BIN_RECORD_VERSION);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&checksum.to_le_bytes());
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
        let header = decode_event_bin_header(record)?;
        let actual = record.len().saturating_sub(BIN_RECORD_HEADER_LEN);
        if actual != header.payload_len {
            return Err(CodecError::BinaryLengthMismatch {
                expected: header.payload_len,
                actual,
            });
        }

        let payload = &record[BIN_RECORD_HEADER_LEN..];
        let actual_checksum = crc32fast::hash(payload);
        if actual_checksum != header.checksum {
            return Err(CodecError::BinaryChecksumMismatch {
                expected: header.checksum,
                actual: actual_checksum,
            });
        }

        decode_event_bin_payload(payload)
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = record;
        Err(CodecError::BinaryUnsupported)
    }
}

pub fn decode_event_bin_header(header: &[u8]) -> Result<BinRecordHeader, CodecError> {
    #[cfg(feature = "bin")]
    {
        if header.len() < BIN_RECORD_HEADER_LEN {
            return Err(CodecError::BinaryRecordTooShort);
        }

        let magic = [header[0], header[1], header[2], header[3]];
        if magic != BIN_RECORD_MAGIC {
            return Err(CodecError::BinaryMagicMismatch(magic));
        }

        let version = header[4];
        if version != BIN_RECORD_VERSION {
            return Err(CodecError::BinaryUnsupportedVersion(version));
        }

        let payload_len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
        let checksum = u32::from_le_bytes([header[9], header[10], header[11], header[12]]);
        Ok(BinRecordHeader {
            payload_len,
            checksum,
        })
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = header;
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

        let line = encode_event_json_line(&event).unwrap();
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

        let line = encode_event_json_line(&event).unwrap();
        let decoded = decode_event_json_line(&line).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn invalid_line_returns_error() {
        assert!(decode_event_json_line("").is_err());
        assert!(decode_event_json_line("{not-json}").is_err());
    }

    #[cfg(feature = "bin")]
    #[test]
    fn round_trip_bin_record_with_header_and_crc() {
        let event = MarketEvent::L2Delta {
            ts_ns: 42,
            symbol: Symbol::new("BTC-USD").unwrap(),
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(3).unwrap(),
            }],
        };

        let record = encode_event_bin_record(&event).unwrap();
        assert_eq!(&record[..4], &BIN_RECORD_MAGIC);
        assert_eq!(record[4], BIN_RECORD_VERSION);

        let decoded = decode_event_bin_record(&record).unwrap();
        assert_eq!(decoded, event);
    }

    #[cfg(feature = "bin")]
    #[test]
    fn bin_record_crc_mismatch_is_rejected() {
        let event = MarketEvent::L2Delta {
            ts_ns: 7,
            symbol: Symbol::new("ETH-USD").unwrap(),
            updates: vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(101).unwrap(),
                qty: Qty::new(1).unwrap(),
            }],
        };

        let mut record = encode_event_bin_record(&event).unwrap();
        let payload_start = BIN_RECORD_HEADER_LEN;
        record[payload_start] ^= 0xFF;

        let err = decode_event_bin_record(&record).unwrap_err();
        assert!(matches!(err, CodecError::BinaryChecksumMismatch { .. }));
    }
}
