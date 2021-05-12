use crate::handle::Handle;
use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr;
use winapi::shared::minwindef::DWORD;
use winapi::um::fileapi;
use winapi::um::handleapi;
use winapi::um::namedpipeapi;
use winapi::um::winbase;
use winapi::um::winnt;

/// The pipe mode of a [Handle].
///
/// Set through [NamedPipeOptions::pipe_mode].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PipeMode {
    /// Data is written to the pipe as a stream of bytes. The pipe does not
    /// distinguish bytes written during different write operations.
    ///
    /// Corresponds to [PIPE_TYPE_BYTE][winapi::um::winbase::PIPE_TYPE_BYTE].
    Byte,
    /// Data is written to the pipe as a stream of messages. The pipe treats the
    /// bytes written during each write operation as a message unit. Any reading
    /// function on [Handle] returns [ERROR_MORE_DATA] when a message is not
    /// read completely.
    ///
    /// Corresponds to [PIPE_TYPE_MESSAGE][winapi::um::winbase::PIPE_TYPE_MESSAGE].
    ///
    /// [ERROR_MORE_DATA]: winapi::shared::winerror::ERROR_MORE_DATA
    Message,
}

// Helper to set a boolean flag as a bitfield.
macro_rules! bool_flag {
    ($f:expr, $t:expr, $flag:expr) => {{
        let current = $f;

        if $t {
            $f = current | $flag;
        } else {
            $f = current & !$flag;
        };
    }};
}

/// A builder structure for construct a named pipe with named pipe-specific
/// options. This is required to use for named pipe servers who wants to modify
/// pipe-related options.
///
/// See [CreatePipeOptions::create].
#[derive(Debug, Clone)]
pub struct CreatePipeOptions {
    open_mode: DWORD,
    pipe_mode: DWORD,
    max_instances: DWORD,
    out_buffer_size: DWORD,
    in_buffer_size: DWORD,
    default_timeout: DWORD,
}

impl CreatePipeOptions {
    /// Creates a new named pipe builder with the default settings.
    ///
    /// ```
    /// use tokio::net::windows::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-new";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let server = CreatePipeOptions::new()
    ///     .create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn new() -> CreatePipeOptions {
        CreatePipeOptions {
            open_mode: winbase::PIPE_ACCESS_DUPLEX | winbase::FILE_FLAG_OVERLAPPED,
            pipe_mode: winbase::PIPE_TYPE_BYTE | winbase::PIPE_REJECT_REMOTE_CLIENTS,
            max_instances: winbase::PIPE_UNLIMITED_INSTANCES,
            out_buffer_size: 65536,
            in_buffer_size: 65536,
            default_timeout: 0,
        }
    }

    /// The pipe mode.
    ///
    /// The default pipe mode is [PipeMode::Byte]. See [PipeMode] for
    /// documentation of what each mode means.
    ///
    /// This corresponding to specifying [dwPipeMode].
    ///
    /// [dwPipeMode]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn pipe_mode(&mut self, pipe_mode: PipeMode) -> &mut Self {
        self.pipe_mode = match pipe_mode {
            PipeMode::Byte => winbase::PIPE_TYPE_BYTE,
            PipeMode::Message => winbase::PIPE_TYPE_MESSAGE,
        };

        self
    }

    /// The flow of data in the pipe goes from client to server only.
    ///
    /// This corresponds to setting [PIPE_ACCESS_INBOUND].
    ///
    /// [PIPE_ACCESS_INBOUND]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_access_inbound
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    /// use tokio::net::windows::{CreatePipeClientOptions, CreatePipeOptions};
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-access-inbound";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// // Server side prevents connecting by denying inbound access, client errors
    /// // when attempting to create the connection.
    /// {
    ///     let _server = CreatePipeOptions::new()
    ///         .access_inbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let e = CreatePipeClientOptions::new()
    ///         .create(PIPE_NAME)
    ///         .unwrap_err();
    ///
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    ///
    ///     // Disabling writing allows a client to connect, but leads to runtime
    ///     // error if a write is attempted.
    ///     let mut client = CreatePipeClientOptions::new()
    ///         .write(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let e = client.write(b"ping").await.unwrap_err();
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// }
    ///
    /// // A functional, unidirectional server-to-client only communication.
    /// {
    ///     let mut server = CreatePipeOptions::new()
    ///         .access_inbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let mut client = CreatePipeClientOptions::new()
    ///         .write(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let write = server.write_all(b"ping");
    ///
    ///     let mut buf = [0u8; 4];
    ///     let read = client.read_exact(&mut buf);
    ///
    ///     let ((), read) = tokio::try_join!(write, read)?;
    ///
    ///     assert_eq!(read, 4);
    ///     assert_eq!(&buf[..], b"ping");
    /// }
    /// # Ok(()) }
    /// ```
    pub fn access_inbound(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.open_mode, allowed, winbase::PIPE_ACCESS_INBOUND);
        self
    }

    /// The flow of data in the pipe goes from server to client only.
    ///
    /// This corresponds to setting [PIPE_ACCESS_OUTBOUND].
    ///
    /// [PIPE_ACCESS_OUTBOUND]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_access_outbound
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    /// use tokio::net::windows::{CreatePipeClientOptions, CreatePipeOptions};
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-access-outbound";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// // Server side prevents connecting by denying outbound access, client errors
    /// // when attempting to create the connection.
    /// {
    ///     let _server = CreatePipeOptions::new()
    ///         .access_outbound(false)
    ///         .create(PIPE_NAME)?;
    ///
    ///     let e = CreatePipeClientOptions::new()
    ///         .create(PIPE_NAME)
    ///         .unwrap_err();
    ///
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    ///
    ///     // Disabling reading allows a client to connect, but leads to runtime
    ///     // error if a read is attempted.
    ///     let mut client = CreatePipeClientOptions::new().read(false).create(PIPE_NAME)?;
    ///
    ///     let mut buf = [0u8; 4];
    ///     let e = client.read(&mut buf).await.unwrap_err();
    ///     assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// }
    ///
    /// // A functional, unidirectional client-to-server only communication.
    /// {
    ///     let mut server = CreatePipeOptions::new().access_outbound(false).create(PIPE_NAME)?;
    ///     let mut client = CreatePipeClientOptions::new().read(false).create(PIPE_NAME)?;
    ///
    ///     // TODO: Explain why this test doesn't work without calling connect
    ///     // first.
    ///     //
    ///     // Because I have no idea -- udoprog
    ///     server.connect().await?;
    ///
    ///     let write = client.write_all(b"ping");
    ///
    ///     let mut buf = [0u8; 4];
    ///     let read = server.read_exact(&mut buf);
    ///
    ///     let ((), read) = tokio::try_join!(write, read)?;
    ///
    ///     println!("done reading and writing");
    ///
    ///     assert_eq!(read, 4);
    ///     assert_eq!(&buf[..], b"ping");
    /// }
    /// # Ok(()) }
    /// ```
    pub fn access_outbound(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.open_mode, allowed, winbase::PIPE_ACCESS_OUTBOUND);
        self
    }

    /// If you attempt to create multiple instances of a pipe with this flag,
    /// creation of the first instance succeeds, but creation of the next
    /// instance fails with [ERROR_ACCESS_DENIED].
    ///
    /// This corresponds to setting [FILE_FLAG_FIRST_PIPE_INSTANCE].
    ///
    /// [ERROR_ACCESS_DENIED]: winapi::shared::winerror::ERROR_ACCESS_DENIED
    /// [FILE_FLAG_FIRST_PIPE_INSTANCE]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_first_pipe_instance
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    /// use tokio::net::windows::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-first-instance";
    ///
    /// # #[tokio::main] async fn main() -> io::Result<()> {
    /// let mut builder = CreatePipeOptions::new();
    /// builder.first_pipe_instance(true);
    ///
    /// let server = builder.create(PIPE_NAME)?;
    /// let e = builder.create(PIPE_NAME).unwrap_err();
    /// assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    /// drop(server);
    ///
    /// // OK: since, we've closed the other instance.
    /// let _server2 = builder.create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn first_pipe_instance(&mut self, first: bool) -> &mut Self {
        bool_flag!(
            self.open_mode,
            first,
            winbase::FILE_FLAG_FIRST_PIPE_INSTANCE
        );
        self
    }

    /// Indicates whether this server can accept remote clients or not. This is
    /// enabled by default.
    ///
    /// This corresponds to setting [PIPE_REJECT_REMOTE_CLIENTS].
    ///
    /// [PIPE_REJECT_REMOTE_CLIENTS]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea#pipe_reject_remote_clients
    pub fn reject_remote_clients(&mut self, reject: bool) -> &mut Self {
        bool_flag!(self.pipe_mode, reject, winbase::PIPE_REJECT_REMOTE_CLIENTS);
        self
    }

    /// The maximum number of instances that can be created for this pipe. The
    /// first instance of the pipe can specify this value; the same number must
    /// be specified for other instances of the pipe. Acceptable values are in
    /// the range 1 through 254. The default value is unlimited.
    ///
    /// This corresponds to specifying [nMaxInstances].
    ///
    /// [nMaxInstances]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    /// [PIPE_UNLIMITED_INSTANCES]: winapi::um::winbase::PIPE_UNLIMITED_INSTANCES
    ///
    /// # Panics
    ///
    /// This function will panic if more than 254 instances are specified. If
    /// you do not wish to set an instance limit, leave it unspecified.
    ///
    /// ```should_panic
    /// use tokio::net::windows::CreatePipeOptions;
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let builder = CreatePipeOptions::new().max_instances(255);
    /// # Ok(()) }
    /// ```
    pub fn max_instances(&mut self, instances: usize) -> &mut Self {
        assert!(instances < 255, "cannot specify more than 254 instances");
        self.max_instances = instances as DWORD;
        self
    }

    /// The number of bytes to reserve for the output buffer.
    ///
    /// This corresponds to specifying [nOutBufferSize].
    ///
    /// [nOutBufferSize]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn out_buffer_size(&mut self, buffer: u32) -> &mut Self {
        self.out_buffer_size = buffer as DWORD;
        self
    }

    /// The number of bytes to reserve for the input buffer.
    ///
    /// This corresponds to specifying [nInBufferSize].
    ///
    /// [nInBufferSize]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    pub fn in_buffer_size(&mut self, buffer: u32) -> &mut Self {
        self.in_buffer_size = buffer as DWORD;
        self
    }

    /// Create the named pipe identified by `addr` for use as a server.
    ///
    /// This function will call the [CreateNamedPipe] function and return the
    /// result.
    ///
    /// [CreateNamedPipe]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea
    ///
    /// # Errors
    ///
    /// This errors if called outside of a [Tokio Runtime] which doesn't have
    /// [I/O enabled] or if any OS-specific I/O errors occur.
    ///
    /// [Tokio Runtime]: crate::runtime::Runtime
    /// [I/O enabled]: crate::runtime::Builder::enable_io
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::net::windows::CreatePipeOptions;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-create";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let server = CreatePipeOptions::new().create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn create(&self, addr: impl AsRef<OsStr>) -> io::Result<Handle> {
        // Safety: We're calling create_with_security_attributes w/ a null
        // pointer which disables it.
        unsafe { self.create_with_security_attributes(addr, ptr::null_mut()) }
    }

    /// Create the named pipe identified by `addr` for use as a server.
    ///
    /// This is the same as [create][CreatePipeOptions::create] except that it
    /// supports providing security attributes.
    ///
    /// # Errors
    ///
    /// This errors if called outside of a [Tokio Runtime] which doesn't have
    /// [I/O enabled] or if any OS-specific I/O errors occur.
    ///
    /// [Tokio Runtime]: crate::runtime::Runtime
    /// [I/O enabled]: crate::runtime::Builder::enable_io
    ///
    /// # Safety
    ///
    /// The caller must ensure that `attrs` points to an initialized instance of
    /// a [SECURITY_ATTRIBUTES] structure.
    ///
    /// [SECURITY_ATTRIBUTES]: [winapi::um::minwinbase::SECURITY_ATTRIBUTES]
    pub unsafe fn create_with_security_attributes(
        &self,
        addr: impl AsRef<OsStr>,
        attrs: *mut (),
    ) -> io::Result<Handle> {
        let addr = encode_addr(addr);

        let handle = namedpipeapi::CreateNamedPipeW(
            addr.as_ptr(),
            self.open_mode,
            self.pipe_mode,
            self.max_instances,
            self.out_buffer_size,
            self.in_buffer_size,
            self.default_timeout,
            attrs as *mut _,
        );

        if handle == handleapi::INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        Ok(Handle::from_raw(handle))
    }
}

/// A builder suitable for building and interacting with named pipes from the
/// client side.
///
/// See [CreatePipeClientOptions::create].
#[derive(Debug, Clone)]
pub struct CreatePipeClientOptions {
    desired_access: DWORD,
}

impl CreatePipeClientOptions {
    /// Creates a new named pipe builder with the default settings.
    ///
    /// ```
    /// use tokio::net::windows::{NamedPipeOptions, CreatePipeClientOptions};
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\tokio-named-pipe-client-new";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// // Server must be created in order for the client creation to succeed.
    /// let server = NamedPipeOptions::new().create(PIPE_NAME)?;
    /// let client = CreatePipeClientOptions::new().create(PIPE_NAME)?;
    /// # Ok(()) }
    /// ```
    pub fn new() -> Self {
        Self {
            desired_access: winnt::GENERIC_READ | winnt::GENERIC_WRITE,
        }
    }

    /// If the client supports reading data. This is enabled by default.
    ///
    /// This corresponds to setting [GENERIC_READ] in the call to [CreateFile].
    ///
    /// [GENERIC_READ]: https://docs.microsoft.com/en-us/windows/win32/secauthz/generic-access-rights
    /// [CreateFile]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew
    pub fn read(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.desired_access, allowed, winnt::GENERIC_READ);
        self
    }

    /// If the created pipe supports writing data. This is enabled by default.
    ///
    /// This corresponds to setting [GENERIC_WRITE] in the call to [CreateFile].
    ///
    /// [GENERIC_WRITE]: https://docs.microsoft.com/en-us/windows/win32/secauthz/generic-access-rights
    /// [CreateFile]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew
    pub fn write(&mut self, allowed: bool) -> &mut Self {
        bool_flag!(self.desired_access, allowed, winnt::GENERIC_WRITE);
        self
    }

    /// Open the named pipe identified by `addr`.
    ///
    /// This constructs the handle using [CreateFile].
    ///
    /// [CreateFile]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea
    ///
    /// # Errors
    ///
    /// This errors if called outside of a [Tokio Runtime] which doesn't have
    /// [I/O enabled] or if any OS-specific I/O errors occur.
    ///
    /// There are a few errors you should be aware of that you need to take into
    /// account when creating a named pipe on the client side:
    ///
    /// * [std::io::ErrorKind::NotFound] - This indicates that the named pipe
    ///   does not exist. Presumably the server is not up.
    /// * [ERROR_PIPE_BUSY] - which needs to be tested for through a constant in
    ///   [winapi]. This error is raised when the named pipe has been created,
    ///   but the server is not currently waiting for a connection.
    ///
    /// [ERROR_PIPE_BUSY]: crate::winapi::shared::winerror::ERROR_PIPE_BUSY
    /// [I/O enabled]: crate::runtime::Builder::enable_io
    /// [Tokio Runtime]: crate::runtime::Runtime
    /// [winapi]: crate::winapi
    ///
    /// A connect loop that waits until a socket becomes available looks like
    /// this:
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use tokio::net::windows::CreatePipeClientOptions;
    /// use tokio::time;
    /// use winapi::shared::winerror;
    ///
    /// const PIPE_NAME: &str = r"\\.\pipe\mynamedpipe";
    ///
    /// # #[tokio::main] async fn main() -> std::io::Result<()> {
    /// let client = loop {
    ///     match CreatePipeClientOptions::new().create(PIPE_NAME) {
    ///         Ok(client) => break client,
    ///         Err(e) if e.raw_os_error() == Some(winerror::ERROR_PIPE_BUSY as i32) => (),
    ///         Err(e) => return Err(e),
    ///     }
    ///
    ///     time::sleep(Duration::from_millis(50)).await;
    /// };
    ///
    /// // use the connected client.
    /// # Ok(()) }
    /// ```
    pub fn create(&self, addr: impl AsRef<OsStr>) -> io::Result<Handle> {
        // Safety: We're calling create_with_security_attributes w/ a null
        // pointer which disables it.
        unsafe { self.create_with_security_attributes(addr, ptr::null_mut()) }
    }

    /// Open the named pipe identified by `addr`.
    ///
    /// This is the same as [create][CreatePipeClientOptions::create] except that
    /// it supports providing security attributes.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `attrs` points to an initialized instance
    /// of a [SECURITY_ATTRIBUTES] structure.
    ///
    /// [SECURITY_ATTRIBUTES]: [crate::winapi::um::minwinbase::SECURITY_ATTRIBUTES]
    pub unsafe fn create_with_security_attributes(
        &self,
        addr: impl AsRef<OsStr>,
        attrs: *mut (),
    ) -> io::Result<Handle> {
        let addr = encode_addr(addr);

        // NB: We could use a platform specialized `OpenOptions` here, but since
        // we have access to winapi it ultimately doesn't hurt to use
        // `CreateFile` explicitly since it allows the use of our already
        // well-structured wide `addr` to pass into CreateFileW.
        let handle = fileapi::CreateFileW(
            addr.as_ptr(),
            self.desired_access,
            0,
            attrs as *mut _,
            fileapi::OPEN_EXISTING,
            winbase::FILE_FLAG_OVERLAPPED | winbase::SECURITY_IDENTIFICATION,
            ptr::null_mut(),
        );

        if handle == handleapi::INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        Ok(Handle::from_raw(handle))
    }
}

/// Encode an address so that it is a null-terminated wide string.
fn encode_addr(addr: impl AsRef<OsStr>) -> Box<[u16]> {
    addr.as_ref().encode_wide().chain(Some(0)).collect()
}
