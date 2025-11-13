use oms::OrderRequest;
use trading_types::ExecutionReport;

pub trait ExecutionVenue {
    fn submit(&mut self, req: &OrderRequest, out: &mut Vec<ExecutionReport>);

    fn on_book_update(&mut self, _out: &mut Vec<ExecutionReport>) {}
}
