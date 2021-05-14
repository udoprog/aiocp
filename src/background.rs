use crate::completion_port::{CompletionOutcome, CompletionPort, CompletionPortPoll};
use std::io;
use std::sync::Arc;
use std::thread;

/// A handle to the background thread.
#[derive(Debug)]
pub struct BackgroundThread {
    thread: thread::JoinHandle<io::Result<()>>,
}

impl BackgroundThread {
    /// Join the background thread.
    pub fn join(self) -> io::Result<()> {
        match self.thread.join() {
            Ok(result) => result,
            Err(..) => Err(io::Error::new(
                io::ErrorKind::Other,
                "background thread panicked",
            )),
        }
    }
}

/// Setup a background thread, return a reference to the thread and a handle
/// that can be used to register file handles.
pub fn setup(threads: u32) -> io::Result<(Arc<CompletionPort>, BackgroundThread)> {
    let handle = Arc::new(CompletionPort::create(threads)?);
    let handle2 = handle.clone();

    let thread = std::thread::spawn(move || {
        let pending = loop {
            match handle2.poll()? {
                CompletionPortPoll::Status(status) => match status.outcome {
                    CompletionOutcome::Cancelled => {
                        status.unlock();
                    }
                    _ => {
                        status.release();
                    }
                },
                CompletionPortPoll::Shutdown(shutdown) => {
                    break shutdown.pending;
                }
            }
        };

        trace!(pending = pending, "shutting down");

        while pending > 0 {
            let status = handle2.poll_during_shutdown()?;
            status.release();
        }

        Ok::<_, io::Error>(())
    });

    let background = BackgroundThread { thread };

    Ok((handle, background))
}
