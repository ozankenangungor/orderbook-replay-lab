use oms::OrderRequest;
use trading_types::ExecutionReport;

pub trait ExecutionVenue {
    fn submit(&mut self, req: &OrderRequest) -> Vec<ExecutionReport>;
}
