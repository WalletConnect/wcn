use {
    derive_more::derive::TryFrom,
    serde::{Deserialize, Serialize},
    std::time::Duration,
    strum::IntoStaticStr,
    wcn_rpc::{ApiName, Message, RpcImpl, transport::JsonCodec},
};

#[cfg(feature = "rpc_client")]
pub mod client;
#[cfg(feature = "rpc_client")]
pub use client::Client;
#[cfg(feature = "rpc_server")]
pub mod server;

#[derive(Clone, Copy, Debug, TryFrom, IntoStaticStr)]
#[try_from(repr)]
#[repr(u8)]
pub enum Id {
    GetMetrics = 0,
}

impl From<Id> for u8 {
    fn from(id: Id) -> Self {
        id as u8
    }
}

#[derive(Clone, Copy, Default)]
pub struct MetricsApi<S = ()> {
    rpc_timeout: Option<Duration>,
    state: S,
}

impl MetricsApi {
    /// Creates a new [`MetricsApi`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds RPC timeout to this [`MetricsApi`].
    pub fn with_rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = Some(timeout);
        self
    }

    /// Adds `state` to this [`MetricsApi`].
    pub fn with_state<S>(self, state: S) -> MetricsApi<S> {
        MetricsApi {
            rpc_timeout: self.rpc_timeout,
            state,
        }
    }
}

impl<S> wcn_rpc::Api for MetricsApi<S>
where
    S: Clone + Send + Sync + 'static,
{
    const NAME: ApiName = ApiName::new("Metrics");
    type RpcId = Id;

    fn rpc_timeout(&self, rpc_id: Id) -> Option<Duration> {
        match rpc_id {
            Id::GetMetrics => self.rpc_timeout,
        }
    }
}

type Rpc<const ID: u8, Req, Resp> = RpcImpl<ID, Req, Resp, JsonCodec>;

type GetMetrics = Rpc<{ Id::GetMetrics as u8 }, String, Result<String>>;

#[derive(Clone, Debug, Serialize, Deserialize, Message, thiserror::Error)]
#[error("code = {code}, message = {message:?}")]
struct Error {
    code: u8,
    message: Option<String>,
}

impl Error {
    fn new(code: ErrorCode, details: Option<String>) -> Self {
        Self {
            code: code as u8,
            message: details,
        }
    }
}

impl From<ErrorCode> for Error {
    fn from(code: ErrorCode) -> Self {
        Self::new(code, None)
    }
}

#[derive(TryFrom, IntoStaticStr)]
#[try_from(repr)]
#[repr(u8)]
enum ErrorCode {
    Internal = 0,
    NotFound = 1,
}

type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for crate::Error {
    fn from(err: Error) -> Self {
        use crate::ErrorKind;

        let Ok(code) = ErrorCode::try_from(err.code) else {
            return Self::new(crate::ErrorKind::Unknown)
                .with_message(format!("Unexpected error code: {}", err.code));
        };

        let kind = match code {
            ErrorCode::NotFound => ErrorKind::NotFound,
            ErrorCode::Internal => ErrorKind::Internal,
        };

        Self {
            kind,
            message: err.message,
        }
    }
}

impl From<crate::Error> for Error {
    fn from(err: crate::Error) -> Self {
        use crate::ErrorKind;

        let code = match err.kind {
            ErrorKind::NotFound => ErrorCode::NotFound,

            ErrorKind::Internal
            | ErrorKind::Timeout
            | ErrorKind::Transport
            | ErrorKind::Unknown => ErrorCode::Internal,
        };

        Error::new(code, err.message)
    }
}

impl wcn_rpc::metrics::ErrorResponse for Error {
    fn kind(&self) -> &'static str {
        ErrorCode::try_from(self.code)
            .map(Into::into)
            .unwrap_or("Unknown")
    }
}

#[derive(TryFrom)]
#[try_from(repr)]
#[repr(u8)]
enum ConnectionRejectionReason {
    Internal = 0,
    Unauthorized = 1,
}

impl From<ConnectionRejectionReason> for crate::FactoryError {
    fn from(err: ConnectionRejectionReason) -> Self {
        use crate::FactoryErrorKind as Kind;

        let kind = match err {
            ConnectionRejectionReason::Internal => Kind::Internal,
            ConnectionRejectionReason::Unauthorized => Kind::Unauthorized,
        };

        Self {
            kind,
            message: None,
        }
    }
}

impl From<crate::FactoryError> for ConnectionRejectionReason {
    fn from(err: crate::FactoryError) -> Self {
        use crate::FactoryErrorKind as Kind;

        match err.kind {
            Kind::Internal | Kind::Unknown => Self::Internal,
            Kind::Unauthorized => Self::Unauthorized,
        }
    }
}
