use thiserror::Error;

use lob_core::{CoreError, MarketEvent, SymbolTable};

#[cfg(feature = "bin")]
use lob_core::{LevelUpdate, Price, Qty};
#[cfg(feature = "bin")]
use serde::{Deserialize, Serialize};

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
    #[error("core error: {0}")]
    Core(#[from] CoreError),
    #[error("unknown symbol id: {0}")]
    UnknownSymbolId(u32),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum JsonMarketEvent {
    L2Delta {
        ts_ns: u64,
        symbol: String,
        updates: Vec<lob_core::LevelUpdate>,
    },
    L2Snapshot {
        ts_ns: u64,
        symbol: String,
        bids: Vec<(lob_core::Price, lob_core::Qty)>,
        asks: Vec<(lob_core::Price, lob_core::Qty)>,
    },
}

impl JsonMarketEvent {
    fn from_core(event: &MarketEvent, symbols: &SymbolTable) -> Result<Self, CodecError> {
        match event {
            MarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => {
                let symbol = symbols
                    .try_resolve(*symbol)
                    .ok_or(CodecError::UnknownSymbolId(symbol.as_u32()))?
                    .to_string();
                Ok(Self::L2Delta {
                    ts_ns: *ts_ns,
                    symbol,
                    updates: updates.clone(),
                })
            }
            MarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => {
                let symbol = symbols
                    .try_resolve(*symbol)
                    .ok_or(CodecError::UnknownSymbolId(symbol.as_u32()))?
                    .to_string();
                Ok(Self::L2Snapshot {
                    ts_ns: *ts_ns,
                    symbol,
                    bids: bids.clone(),
                    asks: asks.clone(),
                })
            }
        }
    }

    fn into_core(self, symbols: &mut SymbolTable) -> Result<MarketEvent, CodecError> {
        match self {
            JsonMarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => Ok(MarketEvent::L2Delta {
                ts_ns,
                symbol: symbols.try_intern(&symbol)?,
                updates,
            }),
            JsonMarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => Ok(MarketEvent::L2Snapshot {
                ts_ns,
                symbol: symbols.try_intern(&symbol)?,
                bids,
                asks,
            }),
        }
    }
}

pub fn encode_event_json_line(
    event: &MarketEvent,
    symbols: &SymbolTable,
) -> Result<String, CodecError> {
    let wire = JsonMarketEvent::from_core(event, symbols)?;
    Ok(serde_json::to_string(&wire)?)
}

pub fn decode_event_json_line(
    line: &str,
    symbols: &mut SymbolTable,
) -> Result<MarketEvent, CodecError> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return Err(CodecError::EmptyLine);
    }

    let wire: JsonMarketEvent = serde_json::from_str(line)?;
    wire.into_core(symbols)
}

pub fn encode_event_bin_record(
    event: &MarketEvent,
    symbols: &SymbolTable,
) -> Result<Vec<u8>, CodecError> {
    #[cfg(feature = "bin")]
    {
        let payload = bincode::serialize(&BinMarketEvent::from_core(event, symbols)?)?;
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
        let _ = symbols;
        Err(CodecError::BinaryUnsupported)
    }
}

pub fn decode_event_bin_record(
    record: &[u8],
    symbols: &mut SymbolTable,
) -> Result<MarketEvent, CodecError> {
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

        decode_event_bin_payload(payload, symbols)
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = record;
        let _ = symbols;
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

pub fn decode_event_bin_payload(
    payload: &[u8],
    symbols: &mut SymbolTable,
) -> Result<MarketEvent, CodecError> {
    #[cfg(feature = "bin")]
    {
        let event: BinMarketEvent = bincode::deserialize(payload)?;
        event.into_core(symbols)
    }
    #[cfg(not(feature = "bin"))]
    {
        let _ = payload;
        let _ = symbols;
        Err(CodecError::BinaryUnsupported)
    }
}

#[cfg(feature = "bin")]
#[derive(Debug, Serialize, Deserialize)]
enum BinMarketEvent {
    L2Delta {
        ts_ns: u64,
        symbol: String,
        updates: Vec<LevelUpdate>,
    },
    L2Snapshot {
        ts_ns: u64,
        symbol: String,
        bids: Vec<(Price, Qty)>,
        asks: Vec<(Price, Qty)>,
    },
}

#[cfg(feature = "bin")]
impl BinMarketEvent {
    fn from_core(event: &MarketEvent, symbols: &SymbolTable) -> Result<Self, CodecError> {
        match event {
            MarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => {
                let symbol = symbols
                    .try_resolve(*symbol)
                    .ok_or(CodecError::UnknownSymbolId(symbol.as_u32()))?
                    .to_string();
                Ok(Self::L2Delta {
                    ts_ns: *ts_ns,
                    symbol,
                    updates: updates.clone(),
                })
            }
            MarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => {
                let symbol = symbols
                    .try_resolve(*symbol)
                    .ok_or(CodecError::UnknownSymbolId(symbol.as_u32()))?
                    .to_string();
                Ok(Self::L2Snapshot {
                    ts_ns: *ts_ns,
                    symbol,
                    bids: bids.clone(),
                    asks: asks.clone(),
                })
            }
        }
    }

    fn into_core(self, symbols: &mut SymbolTable) -> Result<MarketEvent, CodecError> {
        match self {
            BinMarketEvent::L2Delta {
                ts_ns,
                symbol,
                updates,
            } => Ok(MarketEvent::L2Delta {
                ts_ns,
                symbol: symbols.try_intern(&symbol)?,
                updates,
            }),
            BinMarketEvent::L2Snapshot {
                ts_ns,
                symbol,
                bids,
                asks,
            } => Ok(MarketEvent::L2Snapshot {
                ts_ns,
                symbol: symbols.try_intern(&symbol)?,
                bids,
                asks,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side, SymbolId};

    fn sample_event(symbol: SymbolId) -> MarketEvent {
        MarketEvent::L2Delta {
            ts_ns: 42,
            symbol,
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100).unwrap(),
                qty: Qty::new(3).unwrap(),
            }],
        }
    }

    #[test]
    fn round_trip_json_line() {
        let mut symbols = SymbolTable::new();
        let symbol = symbols.try_intern("BTC-USD").unwrap();
        let event = sample_event(symbol);

        let line = encode_event_json_line(&event, &symbols).unwrap();
        let decoded = decode_event_json_line(&line, &mut symbols).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn round_trip_json_line_snapshot() {
        let mut symbols = SymbolTable::new();
        let symbol = symbols.try_intern("ETH-USD").unwrap();
        let event = MarketEvent::L2Snapshot {
            ts_ns: 7,
            symbol,
            bids: vec![
                (Price::new(100).unwrap(), Qty::new(2).unwrap()),
                (Price::new(99).unwrap(), Qty::new(1).unwrap()),
            ],
            asks: vec![
                (Price::new(101).unwrap(), Qty::new(3).unwrap()),
                (Price::new(102).unwrap(), Qty::new(4).unwrap()),
            ],
        };

        let line = encode_event_json_line(&event, &symbols).unwrap();
        let decoded = decode_event_json_line(&line, &mut symbols).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn invalid_line_returns_error() {
        let mut symbols = SymbolTable::new();
        assert!(decode_event_json_line("", &mut symbols).is_err());
        assert!(decode_event_json_line("{not-json}", &mut symbols).is_err());
    }

    #[cfg(feature = "bin")]
    #[test]
    fn round_trip_bin_record_with_header_and_crc() {
        let mut symbols = SymbolTable::new();
        let symbol = symbols.try_intern("BTC-USD").unwrap();
        let event = sample_event(symbol);

        let record = encode_event_bin_record(&event, &symbols).unwrap();
        assert_eq!(&record[..4], &BIN_RECORD_MAGIC);
        assert_eq!(record[4], BIN_RECORD_VERSION);

        let decoded = decode_event_bin_record(&record, &mut symbols).unwrap();
        assert_eq!(decoded, event);
    }

    #[cfg(feature = "bin")]
    #[test]
    fn bin_record_crc_mismatch_is_rejected() {
        let mut symbols = SymbolTable::new();
        let symbol = symbols.try_intern("ETH-USD").unwrap();
        let event = sample_event(symbol);

        let mut record = encode_event_bin_record(&event, &symbols).unwrap();
        let payload_start = BIN_RECORD_HEADER_LEN;
        record[payload_start] ^= 0xFF;

        let err = decode_event_bin_record(&record, &mut symbols).unwrap_err();
        assert!(matches!(err, CodecError::BinaryChecksumMismatch { .. }));
    }
}
