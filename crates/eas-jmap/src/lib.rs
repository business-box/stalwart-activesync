//! JMAP client for the gateway.
//!
//! - [`StalwartEndpoint::session`]: session discovery, which doubles as the
//!   credential check for incoming EAS requests.
//! - [`JmapClient`]: batched method calls, method errors, blob upload and
//!   download, behind a per-user concurrency limit ([`UserLimits`]).
//! - [`mail`]: mailbox listing, email queries, `/changes`, `/get` and `/set`.
//! - [`push`]: the EventSource stream that tells the gateway when to look.
//!
//! Still to come: calendars and contacts.

mod client;
pub mod mail;
pub mod push;
mod session;

pub use client::{Call, JmapClient, SetOp, SetOutcome, UserLimits};
pub use session::{capability, Credentials, JmapError, Session, StalwartEndpoint};
