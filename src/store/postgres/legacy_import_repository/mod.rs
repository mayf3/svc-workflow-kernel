//! PostgreSQL implementation of the ADC legacy initial-import primitive.

mod receipt;
mod replay;
mod transaction;
mod validation;

pub use transaction::import;
