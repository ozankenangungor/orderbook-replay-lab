use lob_core::{Price, Qty, Side, Symbol};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientOrderId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrderTag(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    Accepted,
    Working,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Price>,
    pub qty: Qty,
    pub tif: TimeInForce,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub client_order_id: ClientOrderId,
    pub status: OrderStatus,
    pub filled_qty: Qty,
    pub last_fill_price: Price,
    pub fee_ticks: i64,
    pub ts_ns: u64,
    pub symbol: Symbol,
    pub side: Side,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    PlaceLimit {
        symbol: Symbol,
        side: Side,
        price: Price,
        qty: Qty,
        tif: TimeInForce,
        tag: Option<OrderTag>,
    },
    Cancel {
        client_order_id: ClientOrderId,
    },
    Replace {
        client_order_id: ClientOrderId,
        new_price: Price,
        new_qty: Qty,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_serde_round_trip() {
        let intent = Intent::PlaceLimit {
            symbol: Symbol::new("BTC-USD").unwrap(),
            side: Side::Bid,
            price: Price::new(100).unwrap(),
            qty: Qty::new(2).unwrap(),
            tif: TimeInForce::Gtc,
            tag: Some(OrderTag("alpha".to_string())),
        };

        let json = serde_json::to_string(&intent).unwrap();
        let decoded: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, intent);
    }
}
