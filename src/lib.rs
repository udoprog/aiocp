//! [<img alt="github" src="https://img.shields.io/badge/github-udoprog/aiocp-8da0cb?style=for-the-badge&logo=github" height="20">](https://github.com/udoprog/aiocp)
//! [<img alt="crates.io" src="https://img.shields.io/crates/v/iocp.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/iocp)
//! [<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-iocp-66c2a5?style=for-the-badge&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K" height="20">](https://docs.rs/iocp)
//!
//! Experimental asynchronous I/O Completion Port driver for Windows.
//!
//! This is an attempt to build a safe and sound async driver with minimal overhead
//! around completion ports.
//!
//! <br>
//!
//! ## Limitations
//!
//! * Each wrapped resource can only have one pending I/O operation at a time.
//!
//! <br>
//!
//! ## Examples
//!
//! ```rust,no_run
//! use std::fs::OpenOptions;
//! use std::io;
//! use std::os::windows::fs::OpenOptionsExt as _;
//!
//! #[tokio::main]
//! async fn main() -> io::Result<()> {
//!     let (port, background) = iocp::setup(1)?;
//!
//!     let output = OpenOptions::new()
//!         .read(true)
//!         .custom_flags(iocp::flags::FILE_FLAG_OVERLAPPED)
//!         .open("read.txt")?;
//!
//!     let mut io = port.register_handle(output, Default::default())?;
//!     let mut buf = [1u8; 128];
//!     let n = io.read(&mut buf).await?;
//!     dbg!(std::str::from_utf8(&buf[..n]).unwrap());
//!
//!     port.shutdown()?;
//!     background.join()?;
//!     Ok(())
//! }
//! ```

#![allow(clippy::arc_with_non_send_sync)]

#[macro_use]
mod macros;

mod atomic_waker;

pub mod flags;

mod errors;

mod completion_port;
pub use self::completion_port::{
    CompletionOutcome, CompletionPoll, CompletionPort, CompletionStatus, RegisterOptions,
};

mod io;
pub use self::io::{Overlapped, OverlappedResult, OverlappedState};

mod arc_handle;
pub use self::arc_handle::ArcHandle;

mod task;

mod handle;
pub use self::handle::Handle;

mod socket;
pub use self::socket::Socket;

mod pool;
pub use self::pool::BufferPool;

pub mod pipe;

pub mod ioctl;

#[cfg(feature = "background")]
mod background;
#[cfg(feature = "background")]
pub use self::background::{setup, BackgroundThread};
