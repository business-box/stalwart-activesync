//! JMAP client for the gateway.
//!
//! Implemented so far: session discovery, which doubles as the credential
//! check for incoming EAS requests. Still to come (phase 2): batched method
//! calls with back-references, `/changes`, blob upload and download, the
//! EventSource push stream and the per-user concurrency limit.

mod session;

pub use session::{capability, Credentials, JmapError, Session, StalwartEndpoint};
