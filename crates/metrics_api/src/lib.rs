#[cfg(any(feature = "rpc_client", feature = "rpc_server"))]
pub mod rpc;

/// API for fetching metrics.
pub trait MetricsApi: Sized + Clone + Send + Sync + 'static {
    /// Gets metrics of the specified `target`.
    fn get_metrics(&self, target: &str) -> impl Future<Output = Result<String>> + Send;
}

/// [`MetricsApi`] factory.
pub trait Factory<Args>: Clone + Send + Sync + 'static {
    /// [`MetricsApi`] types produced by this factory.
    type MetricsApi: MetricsApi;

    /// Creates a new [`MetricsApi`].
    fn new_metrics_api(&self, args: Args) -> FactoryResult<Self::MetricsApi>;
}

/// [`MetricsApi`] result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub type FactoryResult<T, E = FactoryError> = Result<T, E>;

/// [`MetricsApi`] error.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("{kind:?}({message:?})")]
pub struct Error<K = ErrorKind> {
    kind: K,
    message: Option<String>,
}

/// [`Factory`] error.
pub type FactoryError = Error<FactoryErrorKind>;

/// [`Error`] kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Internal error.
    Internal,

    /// Unable to find metrics for the specified target.
    NotFound,

    /// Operation timeout.
    Timeout,

    /// Transport error.
    Transport,

    /// Unable to determine [`ErrorKind`] of an [`Error`].
    Unknown,
}

/// [`FactoryError`] kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryErrorKind {
    /// Internal error.
    Internal,

    /// Unauthorized to access [`MetricsApi`].
    Unauthorized,

    /// Unable to determine [`ErrorKind`] of a [`FactoryError`].
    Unknown,
}

impl<K> Error<K> {
    /// Creates a new [`Error`].
    pub fn new(kind: K) -> Self {
        Self {
            kind,
            message: None,
        }
    }
}

impl Error {
    /// Creates a a new [`Error`] with [`ErrorKind::NotFound`].
    pub fn not_found() -> Self {
        Self::new(ErrorKind::NotFound)
    }

    /// Specifies error message.
    pub fn with_message(mut self, message: impl ToString) -> Self {
        self.message = Some(message.to_string());
        self
    }

    /// Returns [`ErrorKind`] of this [`Error`].
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl FactoryError {
    /// Creates a a new [`FactoryError`] with
    /// [`FactoryErrorKind::Unauthorized`].
    pub fn unauthorized() -> Self {
        Self::new(FactoryErrorKind::Unauthorized)
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self {
            kind,
            message: None,
        }
    }
}
