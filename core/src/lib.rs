use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid side: {0}")]
    InvalidSide(String),
    #[error("invalid symbol: {0}")]
    InvalidSymbol(String),
    #[error("price must be non-negative, got {0}")]
    InvalidPrice(i64),
    #[error("qty must be non-negative, got {0}")]
    InvalidQty(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Bid => "bid",
            Side::Ask => "ask",
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Side {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("bid") {
            Ok(Side::Bid)
        } else if trimmed.eq_ignore_ascii_case("ask") {
            Ok(Side::Ask)
        } else {
            Err(CoreError::InvalidSide(s.to_string()))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(u32);

impl SymbolId {
    pub const fn from_u32(raw: u32) -> Self {
        Self(raw)
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Default, Clone)]
pub struct SymbolTable {
    by_text: HashMap<Arc<str>, SymbolId>,
    by_id: Vec<Arc<str>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(symbol_capacity: usize) -> Self {
        Self {
            by_text: HashMap::with_capacity(symbol_capacity),
            by_id: Vec::with_capacity(symbol_capacity),
        }
    }

    pub fn try_from_symbols<I, S>(symbols: I) -> Result<Self, CoreError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut table = Self::new();
        for symbol in symbols {
            table.try_intern(symbol.as_ref())?;
        }
        Ok(table)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn reserve(&mut self, additional: usize) {
        self.by_text.reserve(additional);
        self.by_id.reserve(additional);
    }

    pub fn intern(&mut self, value: &str) -> SymbolId {
        let trimmed = value.trim();
        if let Some(symbol_id) = self.by_text.get(trimmed).copied() {
            return symbol_id;
        }

        let interned: Arc<str> = Arc::from(trimmed);
        let symbol_id = SymbolId(self.by_id.len() as u32);
        self.by_text.insert(Arc::clone(&interned), symbol_id);
        self.by_id.push(interned);
        symbol_id
    }

    pub fn try_intern(&mut self, value: &str) -> Result<SymbolId, CoreError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(CoreError::InvalidSymbol(value.to_string()));
        }
        Ok(self.intern(trimmed))
    }

    pub fn try_resolve(&self, id: SymbolId) -> Option<&str> {
        self.by_id.get(id.as_u32() as usize).map(Arc::as_ref)
    }

    pub fn resolve(&self, id: SymbolId) -> &str {
        self.try_resolve(id).unwrap_or("<unknown>")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Price(i64);

impl Price {
    /// Integer ticks keep ordering deterministic and avoid floating-point rounding.
    pub fn new(ticks: i64) -> Result<Self, CoreError> {
        if ticks < 0 {
            Err(CoreError::InvalidPrice(ticks))
        } else {
            Ok(Self(ticks))
        }
    }

    pub fn ticks(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Qty(i64);

impl Qty {
    /// Integer lots avoid floating-point rounding for size updates.
    pub fn new(lots: i64) -> Result<Self, CoreError> {
        if lots < 0 {
            Err(CoreError::InvalidQty(lots))
        } else {
            Ok(Self(lots))
        }
    }

    pub fn lots(self) -> i64 {
        self.0
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelUpdate {
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
}

impl LevelUpdate {
    pub fn is_remove(&self) -> bool {
        self.qty.is_zero()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum MarketEvent {
    L2Delta {
        ts_ns: u64,
        symbol: SymbolId,
        updates: Vec<LevelUpdate>,
    },
    L2Snapshot {
        ts_ns: u64,
        symbol: SymbolId,
        bids: Vec<(Price, Qty)>,
        asks: Vec<(Price, Qty)>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_parsing() {
        assert_eq!(Side::from_str("bid").unwrap(), Side::Bid);
        assert_eq!(Side::from_str("ASK").unwrap(), Side::Ask);
        assert!(Side::from_str("mid").is_err());
    }

    #[test]
    fn qty_zero_means_remove_level() {
        let update = LevelUpdate {
            side: Side::Bid,
            price: Price::new(100).unwrap(),
            qty: Qty::new(0).unwrap(),
        };
        assert!(update.is_remove());

        let update = LevelUpdate {
            side: Side::Ask,
            price: Price::new(101).unwrap(),
            qty: Qty::new(5).unwrap(),
        };
        assert!(!update.is_remove());
    }

    #[test]
    fn symbol_table_interning_is_deterministic() {
        let mut table = SymbolTable::new();
        let btc = table.try_intern("BTC-USD").unwrap();
        let btc_again = table.try_intern(" BTC-USD ").unwrap();
        let eth = table.try_intern("ETH-USD").unwrap();
        assert_eq!(btc, btc_again);
        assert_ne!(btc, eth);
        assert_eq!(table.resolve(btc), "BTC-USD");
        assert_eq!(table.resolve(eth), "ETH-USD");
    }

    #[test]
    fn symbol_table_rejects_empty_symbols() {
        let mut table = SymbolTable::new();
        assert!(table.try_intern("   ").is_err());
    }

    #[test]
    fn symbol_table_try_from_symbols_preserves_first_seen_order() {
        let table = SymbolTable::try_from_symbols(["ETH-USD", "BTC-USD", "ETH-USD"]).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.resolve(SymbolId::from_u32(0)), "ETH-USD");
        assert_eq!(table.resolve(SymbolId::from_u32(1)), "BTC-USD");
    }

    #[test]
    fn symbol_table_reserve_and_is_empty() {
        let mut table = SymbolTable::with_capacity(4);
        assert!(table.is_empty());
        table.reserve(8);
        let id = table.try_intern("SOL-USD").unwrap();
        assert_eq!(id, SymbolId::from_u32(0));
        assert!(!table.is_empty());
    }

    #[test]
    fn price_and_qty_validate_non_negative() {
        assert!(Price::new(-1).is_err());
        assert!(Qty::new(-10).is_err());
    }
}
