use oms::OrderRequest;
use trading_types::ExecutionReport;

pub trait ExecutionVenue {
    fn submit(&mut self, req: &OrderRequest) -> Vec<ExecutionReport>;

    fn on_book_update(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }
}
