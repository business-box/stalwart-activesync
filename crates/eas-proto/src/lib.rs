//! Exchange ActiveSync protocol model, independent of HTTP and storage.
//!
//! - [`ProtocolVersion`] and [`Command`]: what the gateway speaks.
//! - [`RequestLine`]: the command and device data carried in the request URI,
//!   in either the plain or the base64 form (MS-ASHTTP 2.2.1.1.1).
//! - [`options`]: the headers of the `OPTIONS` response.
//! - [`autodiscover`]: the `mobilesync` autodiscover exchange.
//! - [`status`]: global status codes and where a command puts them.
//! - [`provision`]: the policy document handed out by `Provision`.

pub mod autodiscover;
mod command;
pub mod options;
pub mod provision;
mod request;
pub mod status;
mod version;

pub use command::Command;
pub use request::{QueryError, RequestLine};
pub use version::ProtocolVersion;

/// Path clients post EAS commands to.
pub const ENDPOINT_PATH: &str = "/Microsoft-Server-ActiveSync";
