use thiserror::Error;

use lob_core::MarketEvent;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("empty input line")]
    EmptyLine,
    #[error("json decode error: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode_event_json_line(event: &MarketEvent) -> String {
    serde_json::to_string(event).expect("MarketEvent serialization should not fail")
}

pub fn decode_event_json_line(line: &str) -> Result<MarketEvent, CodecError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(CodecError::EmptyLine);
    }
    Ok(serde_json::from_str(trimmed)?)
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
    fn invalid_line_returns_error() {
        assert!(decode_event_json_line("").is_err());
        assert!(decode_event_json_line("{not-json}").is_err());
    }
}
