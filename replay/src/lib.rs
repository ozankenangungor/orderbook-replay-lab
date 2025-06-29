use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use thiserror::Error;

use lob_core::MarketEvent;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode error: {0}")]
    Decode(#[from] codec::CodecError),
}

pub struct ReplayReader {
    reader: BufReader<File>,
    buffer: String,
}

impl ReplayReader {
    pub fn open(path: &Path) -> Result<Self, ReplayError> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            buffer: String::new(),
        })
    }

    pub fn next_event(&mut self) -> Result<Option<MarketEvent>, ReplayError> {
        self.buffer.clear();
        let bytes = self.reader.read_line(&mut self.buffer)?;
        if bytes == 0 {
            return Ok(None);
        }
        let event = codec::decode_event_json_line(&self.buffer)?;
        Ok(Some(event))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side, Symbol};
    use tempfile::tempdir;

    #[test]
    fn reads_in_order_and_handles_eof() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let path = dir.path().join("events.log");

        let event_one = MarketEvent::L2Delta {
            ts_ns: 1,
            symbol: Symbol::new("BTC-USD")?,
            updates: vec![LevelUpdate {
                side: Side::Bid,
                price: Price::new(100)?,
                qty: Qty::new(5)?,
            }],
        };

        let event_two = MarketEvent::L2Delta {
            ts_ns: 2,
            symbol: Symbol::new("ETH-USD")?,
            updates: vec![LevelUpdate {
                side: Side::Ask,
                price: Price::new(200)?,
                qty: Qty::new(1)?,
            }],
        };

        let mut file = File::create(&path)?;
        writeln!(file, "{}", codec::encode_event_json_line(&event_one))?;
        writeln!(file, "{}", codec::encode_event_json_line(&event_two))?;

        let mut reader = ReplayReader::open(&path)?;
        assert_eq!(reader.next_event()?.unwrap(), event_one);
        assert_eq!(reader.next_event()?.unwrap(), event_two);
        assert_eq!(reader.next_event()?, None);
        assert_eq!(reader.next_event()?, None);

        Ok(())
    }
}
