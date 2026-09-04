//! Native collaboration runtime for Macro document surfaces.
//!
//! The wire format intentionally mirrors
//! `macro/services/sync-service/bebop/schema.bop`. Local edits enter the
//! SQLite write-ahead log before they are offered to the websocket, so a
//! process crash or reconnect cannot silently discard them.

mod awareness;
mod document;
mod history;
mod persistence;
mod protocol;
mod session;
mod transport;

pub use awareness::*;
pub use document::*;
pub use history::*;
pub use persistence::*;
pub use protocol::*;
pub use session::*;
pub use transport::*;
