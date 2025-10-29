use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(u32);

impl SymbolId {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    id: SymbolId,
    value: Arc<str>,
}

#[derive(Default)]
struct SymbolInterner {
    by_text: HashMap<Arc<str>, SymbolId>,
    by_id: Vec<Arc<str>>,
}

impl SymbolInterner {
    fn intern(&mut self, value: &str) -> (SymbolId, Arc<str>) {
        if let Some(symbol_id) = self.by_text.get(value).copied() {
            let index = symbol_id.as_u32() as usize;
            if let Some(interned) = self.by_id.get(index) {
                return (symbol_id, Arc::clone(interned));
            }
        }

        let interned: Arc<str> = Arc::from(value);
        let symbol_id = SymbolId(self.by_id.len() as u32);
        self.by_text.insert(Arc::clone(&interned), symbol_id);
        self.by_id.push(Arc::clone(&interned));
        (symbol_id, interned)
    }
}

fn with_symbol_interner<R>(f: impl FnOnce(&mut SymbolInterner) -> R) -> R {
    static SYMBOL_INTERNER: OnceLock<RwLock<SymbolInterner>> = OnceLock::new();
    let interner = SYMBOL_INTERNER.get_or_init(|| RwLock::new(SymbolInterner::default()));
    let mut guard = match interner.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

impl Symbol {
    pub fn new<S: AsRef<str>>(value: S) -> Result<Self, CoreError> {
        let value = value.as_ref();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(CoreError::InvalidSymbol(value.to_string()));
        }

        let (id, interned) = with_symbol_interner(|interner| interner.intern(trimmed));
        Ok(Self {
            id,
            value: interned,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn id(&self) -> SymbolId {
        self.id
    }
}

impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Symbol {}

impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

impl FromStr for Symbol {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Symbol::new(s)
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

impl Serialize for Symbol {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Symbol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Cow::<str>::deserialize(deserializer)?;
        Symbol::new(value.as_ref()).map_err(serde::de::Error::custom)
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
    fn symbol_requires_non_empty() {
        assert!(Symbol::new("BTC-USD").is_ok());
        assert!(Symbol::new("   ").is_err());
    }

    #[test]
    fn symbol_interning_reuses_symbol_id() {
        let a = Symbol::new("BTC-USD").unwrap();
        let b = Symbol::new(" BTC-USD ").unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a, b);
    }

    #[test]
    fn price_and_qty_validate_non_negative() {
        assert!(Price::new(-1).is_err());
        assert!(Qty::new(-10).is_err());
    }
}
