use {
    crate::Error,
    futures::FutureExt as _,
    futures_concurrency::future::Race as _,
    std::{net::SocketAddrV4, sync::atomic::AtomicUsize},
    wcn_rpc::{
        Client,
        PeerId,
        client::{Api, Connection},
    },
};

#[derive(Clone)]
pub(crate) struct Connector<API: Api> {
    public_conn: Connection<API>,
    private_conn: Option<Connection<API>>,
}

impl<API: Api> Connector<API> {
    pub(crate) fn new(public_conn: Connection<API>, private_conn: Option<Connection<API>>) -> Self {
        Self {
            public_conn,
            private_conn,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        let public_open = !self.public_conn.is_closed();
        let private_open = self.is_private_open();

        public_open || private_open
    }

    fn is_private_open(&self) -> bool {
        self.private_conn
            .as_ref()
            .map(|conn| !conn.is_closed())
            .unwrap_or(false)
    }

    pub(crate) async fn wait_open(&self) -> &Connection<API> {
        // Prefer private connection if it's available.
        if let Some(conn) = &self.private_conn
            && !conn.is_closed()
        {
            return conn;
        }

        let public_fut = self.public_conn.wait_open().map(|_| &self.public_conn);

        if let Some(private) = &self.private_conn {
            let private_fut = private.wait_open().map(|_| private);

            (public_fut, private_fut).race().await
        } else {
            public_fut.await
        }
    }
}

pub(crate) struct ConnectionPool<API: Api> {
    conn: Vec<Connector<API>>,
    counter: AtomicUsize,
}

impl<API> ConnectionPool<API>
where
    API: Api<ConnectionParameters = ()>,
{
    pub(crate) fn new(
        client: &Client<API>,
        id: &PeerId,
        public_addr: SocketAddrV4,
        private_addr: Option<SocketAddrV4>,
        size: usize,
    ) -> Result<Self, Error> {
        let mut conn = Vec::new();

        for _ in 0..size {
            let public_conn = client.new_connection(public_addr, id, ());
            let private_conn = private_addr.map(|addr| client.new_connection(addr, id, ()));

            conn.push(Connector::new(public_conn, private_conn))
        }

        if conn.is_empty() {
            Err(Error::InvalidPoolSize)
        } else {
            Ok(Self {
                conn,
                counter: 0.into(),
            })
        }
    }

    pub(crate) fn next(&self) -> &Connector<API> {
        let next_idx = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let num_conns = self.conn.len();

        &self.conn[next_idx % num_conns]
    }
}
