//! JMAP client for the gateway.
//!
//! - [`StalwartEndpoint::session`]: session discovery, which doubles as the
//!   credential check for incoming EAS requests.
//! - [`JmapClient`]: batched method calls, method errors, blob upload and
//!   download, behind a per-user concurrency limit ([`UserLimits`]).
//! - [`mail`]: mailbox listing, email queries, `/changes`, `/get` and `/set`.
//!
//! Still to come: the EventSource push stream, calendars and contacts.

mod client;
pub mod mail;
mod session;

pub use client::{Call, JmapClient, UserLimits};
pub use session::{capability, Credentials, JmapError, Session, StalwartEndpoint};
