/// Same as [tracing::trace!][tracing::trace].
#[cfg(feature = "tracing")]
macro_rules! trace {
    ($($tt:tt)*) => {tracing::trace!($($tt)*)}
}

/// Tracing disabled.
#[cfg(not(feature = "tracing"))]
macro_rules! trace {
    ($($tt:tt)*) => {};
}
