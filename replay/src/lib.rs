use std::fs::File;
use std::io::{BufRead, BufReader, Read};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayFormat {
    Jsonl,
    Bin,
}

pub struct ReplayReader {
    reader: BufReader<File>,
    format: ReplayFormat,
    buffer: String,
    bin_buf: Vec<u8>,
}

#[cfg(feature = "mmap")]
pub struct MmapReplayReader {
    mmap: memmap2::Mmap,
    pos: usize,
}

impl ReplayReader {
    pub fn open(path: &Path) -> Result<Self, ReplayError> {
        Self::open_with_format(path, ReplayFormat::Jsonl)
    }

    pub fn open_with_format(path: &Path, format: ReplayFormat) -> Result<Self, ReplayError> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::with_capacity(64 * 1024, file),
            format,
            buffer: String::with_capacity(4096),
            bin_buf: Vec::with_capacity(4096),
        })
    }

    pub fn next_event(&mut self) -> Result<Option<MarketEvent>, ReplayError> {
        match self.format {
            ReplayFormat::Jsonl => self.next_event_json(),
            ReplayFormat::Bin => self.next_event_bin(),
        }
    }

    fn next_event_json(&mut self) -> Result<Option<MarketEvent>, ReplayError> {
        self.buffer.clear();
        let bytes = self.reader.read_line(&mut self.buffer)?;
        if bytes == 0 {
            return Ok(None);
        }
        let event = codec::decode_event_json_line(&self.buffer)?;
        Ok(Some(event))
    }

    fn next_event_bin(&mut self) -> Result<Option<MarketEvent>, ReplayError> {
        let mut len_buf = [0u8; 4];
        let mut read = 0usize;
        while read < len_buf.len() {
            let n = self.reader.read(&mut len_buf[read..])?;
            if n == 0 {
                if read == 0 {
                    return Ok(None);
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "truncated binary length prefix",
                )
                .into());
            }
            read += n;
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        self.bin_buf.resize(len, 0);
        let mut read = 0usize;
        while read < len {
            let n = self.reader.read(&mut self.bin_buf[read..])?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "truncated binary payload",
                )
                .into());
            }
            read += n;
        }

        let event = codec::decode_event_bin_payload(&self.bin_buf)?;
        Ok(Some(event))
    }
}

#[cfg(feature = "mmap")]
impl MmapReplayReader {
    pub fn open(path: &Path) -> Result<Self, ReplayError> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        Ok(Self { mmap, pos: 0 })
    }

    pub fn next_event(&mut self) -> Result<Option<MarketEvent>, ReplayError> {
        if self.pos == self.mmap.len() {
            return Ok(None);
        }
        if self.mmap.len().saturating_sub(self.pos) < 4 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated binary length prefix",
            )
            .into());
        }

        let len = u32::from_le_bytes([
            self.mmap[self.pos],
            self.mmap[self.pos + 1],
            self.mmap[self.pos + 2],
            self.mmap[self.pos + 3],
        ]) as usize;
        self.pos += 4;

        if self.mmap.len().saturating_sub(self.pos) < len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated binary payload",
            )
            .into());
        }

        let payload = &self.mmap[self.pos..self.pos + len];
        self.pos += len;
        let event = codec::decode_event_bin_payload(payload)?;
        Ok(Some(event))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use lob_core::{LevelUpdate, Price, Qty, Side, Symbol};
    #[cfg(feature = "bin")]
    use orderbook::OrderBook;
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

    #[cfg(feature = "bin")]
    #[test]
    fn jsonl_and_bin_replay_match_final_state() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let json_path = dir.path().join("events.jsonl");
        let bin_path = dir.path().join("events.bin");

        let symbol = Symbol::new("TEST-USD")?;
        let events = vec![
            MarketEvent::L2Snapshot {
                ts_ns: 1,
                symbol: symbol.clone(),
                bids: vec![
                    (Price::new(100)?, Qty::new(1)?),
                    (Price::new(99)?, Qty::new(2)?),
                ],
                asks: vec![
                    (Price::new(101)?, Qty::new(1)?),
                    (Price::new(102)?, Qty::new(2)?),
                ],
            },
            MarketEvent::L2Delta {
                ts_ns: 2,
                symbol: symbol.clone(),
                updates: vec![LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(100)?,
                    qty: Qty::new(0)?,
                }],
            },
            MarketEvent::L2Delta {
                ts_ns: 3,
                symbol: symbol.clone(),
                updates: vec![LevelUpdate {
                    side: Side::Ask,
                    price: Price::new(100)?,
                    qty: Qty::new(3)?,
                }],
            },
            MarketEvent::L2Delta {
                ts_ns: 4,
                symbol: symbol.clone(),
                updates: vec![LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(98)?,
                    qty: Qty::new(4)?,
                }],
            },
        ];

        let mut json_file = File::create(&json_path)?;
        for event in &events {
            writeln!(json_file, "{}", codec::encode_event_json_line(event))?;
        }

        let mut bin_file = File::create(&bin_path)?;
        for event in &events {
            let record = codec::encode_event_bin_record(event)?;
            bin_file.write_all(&record)?;
        }

        let mut json_reader = ReplayReader::open_with_format(&json_path, ReplayFormat::Jsonl)?;
        let mut bin_reader = ReplayReader::open_with_format(&bin_path, ReplayFormat::Bin)?;

        let mut json_book = OrderBook::new(symbol.clone());
        let mut bin_book = OrderBook::new(symbol.clone());

        while let Some(event) = json_reader.next_event()? {
            json_book.apply(&event);
        }
        while let Some(event) = bin_reader.next_event()? {
            bin_book.apply(&event);
        }

        assert_eq!(format!("{:?}", json_book), format!("{:?}", bin_book));

        Ok(())
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn bin_mmap_and_bufread_match_event_streams() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        let bin_path = dir.path().join("events.bin");

        let symbol = Symbol::new("MMAP-USD")?;
        let events = vec![
            MarketEvent::L2Snapshot {
                ts_ns: 1,
                symbol: symbol.clone(),
                bids: vec![(Price::new(100)?, Qty::new(1)?)],
                asks: vec![(Price::new(101)?, Qty::new(2)?)],
            },
            MarketEvent::L2Delta {
                ts_ns: 2,
                symbol: symbol.clone(),
                updates: vec![LevelUpdate {
                    side: Side::Bid,
                    price: Price::new(99)?,
                    qty: Qty::new(3)?,
                }],
            },
            MarketEvent::L2Delta {
                ts_ns: 3,
                symbol: symbol.clone(),
                updates: vec![LevelUpdate {
                    side: Side::Ask,
                    price: Price::new(100)?,
                    qty: Qty::new(4)?,
                }],
            },
        ];

        let mut bin_file = File::create(&bin_path)?;
        for event in &events {
            let record = codec::encode_event_bin_record(event)?;
            bin_file.write_all(&record)?;
        }

        let mut buf_reader = ReplayReader::open_with_format(&bin_path, ReplayFormat::Bin)?;
        let mut mmap_reader = MmapReplayReader::open(&bin_path)?;

        let mut buf_events = Vec::new();
        while let Some(event) = buf_reader.next_event()? {
            buf_events.push(event);
        }

        let mut mmap_events = Vec::new();
        while let Some(event) = mmap_reader.next_event()? {
            mmap_events.push(event);
        }

        assert_eq!(buf_events, mmap_events);

        Ok(())
    }
}
