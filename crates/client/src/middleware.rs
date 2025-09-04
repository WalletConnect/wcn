pub use {
    encryption::{
        Error as EncryptionError,
        Key as EncryptionKey,
        WithEncryption,
        WithEncryptionBuilder,
    },
    observer::{
        OperationName,
        RequestMetadata,
        RequestObserver,
        WithObserver,
        WithObserverBuilder,
    },
    retries::{WithRetries, WithRetriesBuilder},
};

use crate::{Client, Config, Error, NodeData};

mod encryption;
mod observer;
mod retries;

pub trait MiddlewareBuilder {
    type NodeData: NodeData;

    type Inner<D>
    where
        D: NodeData;

    fn build<D>(
        self,
        config: Config,
    ) -> impl Future<Output = Result<Self::Inner<D>, Error>> + Send + Sync
    where
        D: NodeData;
}

pub struct BaseClientBuilder;

impl MiddlewareBuilder for BaseClientBuilder {
    type NodeData = ();

    type Inner<D>
        = Client<D>
    where
        D: NodeData;

    async fn build<D>(self, config: Config) -> Result<Self::Inner<D>, Error>
    where
        D: NodeData,
    {
        Client::new(config).await
    }
}
