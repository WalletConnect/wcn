use {
    crate::WcnClient,
    futures::{StreamExt, stream},
    futures_concurrency::future::Race as _,
    rand::seq::IndexedRandom,
    std::{
        array,
        collections::{BTreeMap, HashMap},
        fmt,
        iter,
        ops::RangeInclusive,
        sync::{
            Arc,
            atomic::{self, AtomicUsize},
        },
        time::{Duration, Instant},
    },
    tap::Pipe as _,
    time::OffsetDateTime,
    tokio::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard},
    tracing::Instrument,
    wcn_client::MapPage,
    wcn_rpc::server::ShutdownSignal,
    wcn_storage_api::{Record, RecordExpiration, RecordVersion},
};

const KV_KEYS: u32 = 50_000;
const MAP_KEYS: u32 = 500;

const KEY_SIZE: usize = 32;
const FIELD_SIZE: usize = 32;
const VALUE_SIZE: usize = 1024;

pub(super) struct LoadSimulator {
    namespaces: Vec<Namespace>,
    stats: Stats,
}

impl LoadSimulator {
    pub(super) fn new(client: WcnClient, namespaces: &[wcn_client::Namespace]) -> Self {
        let stats = Stats::default();

        Self {
            namespaces: namespaces
                .iter()
                .copied()
                .map(|ns| Namespace {
                    namespace: ns,
                    client: client.clone(),
                    kv_storage: KvStorage::new(),
                    map_storage: MapStorage::new(),
                    stats: stats.clone(),
                })
                .collect(),
            stats,
        }
    }

    pub(super) async fn populate_namespaces(&mut self) {
        stream::iter(self.namespaces.iter_mut())
            .for_each_concurrent(5, |ns| {
                let idx = ns.namespace.idx();
                ns.populate()
                    .instrument(tracing::info_span!("namespace", %idx))
            })
            .await;
    }

    pub(super) async fn run(&self, shutdown: ShutdownSignal) {
        let fut = async move {
            stream::iter(std::iter::repeat(()))
                .take_until(async move { shutdown.wait().await })
                .for_each_concurrent(10, |_| {
                    self.namespaces
                        .choose(&mut rand::rng())
                        .unwrap()
                        .execute_random_operation()
                })
                .await;
        };

        (fut, self.stats.clone().logger()).race().await;
    }

    pub(super) async fn validate_namespaces(&self) {
        stream::iter(self.namespaces.iter())
            .for_each_concurrent(5, |ns| {
                let idx = ns.namespace.idx();
                ns.validate()
                    .instrument(tracing::info_span!("namespace", %idx))
            })
            .await;
    }
}

struct Namespace {
    namespace: wcn_client::Namespace,
    client: WcnClient,

    kv_storage: KvStorage,
    map_storage: MapStorage,

    stats: Stats,
}

impl Namespace {
    async fn populate(&self) {
        let count = &AtomicUsize::new(0);
        let progress_logger = || async {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;
                tracing::info!("Populated {} keys", count.load(atomic::Ordering::Relaxed));
            }
        };

        let populate_kv_fut = stream::iter(0..(KV_KEYS / 2)).for_each_concurrent(100, |_| async {
            self.execute_random_set().await;
            count.fetch_add(1, atomic::Ordering::Relaxed);
        });

        (populate_kv_fut, progress_logger())
            .race()
            .instrument(tracing::info_span!("KV"))
            .await;

        count.store(0, atomic::Ordering::Relaxed);

        let populate_map_fut =
            stream::iter(0..(MAP_KEYS * 256 / 2)).for_each_concurrent(100, |_| async {
                self.execute_random_hset().await;
                count.fetch_add(1, atomic::Ordering::Relaxed);
            });

        (populate_map_fut, progress_logger())
            .race()
            .instrument(tracing::info_span!("Map"))
            .await;
    }

    async fn validate(&self) {
        let count = &AtomicUsize::new(0);
        let progress_logger = || async {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;
                tracing::info!("Validated {} keys", count.load(atomic::Ordering::Relaxed));
            }
        };

        let validate_kv_fut = stream::iter(self.kv_storage.entries.iter()).for_each_concurrent(
            10,
            |(key, record)| async {
                self.execute_get(*key, &*record.read().await).await;
                count.fetch_add(1, atomic::Ordering::Relaxed);
            },
        );

        (validate_kv_fut, progress_logger())
            .race()
            .instrument(tracing::info_span!("KV"))
            .await;

        let validate_map_fut =
            stream::iter(self.map_storage.entries.iter()).for_each(|(key, map)| async {
                self.execute_hscan(*key, &*map.read().await, 300, None)
                    .await;
                count.fetch_add(1, atomic::Ordering::Relaxed);
            });

        (validate_map_fut, progress_logger())
            .race()
            .instrument(tracing::info_span!("Map"))
            .await;
    }

    async fn execute_random_operation(&self) {
        // 5 to 1 read/write ratio
        match rand::random_range(0..5) {
            0 => self.execute_random_write().await,
            _ => self.execute_random_read().await,
        }
    }

    async fn execute_random_read(&self) {
        // 5 to 1 map/kv ratio
        match rand::random_range(0..5) {
            0 => self.execute_random_kv_read().await,
            _ => self.execute_random_map_read().await,
        }
    }

    async fn execute_random_write(&self) {
        // 5 to 1 map/kv ratio
        match rand::random_range(0..5) {
            0 => self.execute_random_kv_write().await,
            _ => self.execute_random_map_write().await,
        }
    }

    async fn execute_random_kv_read(&self) {
        match rand::random_range(0..2) {
            0 => self.execute_random_get().await,
            1 => self.execute_random_get_exp().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_map_read(&self) {
        match rand::random_range(0..4) {
            0 => self.execute_random_hget().await,
            1 => self.execute_random_hget_exp().await,
            2 => self.execute_random_hcard().await,
            3 => self.execute_random_hscan().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_kv_write(&self) {
        match rand::random_range(0..2) {
            0 => self.execute_random_set().await,
            // TODO: Fix
            // 1 => self.execute_random_set_exp().await,
            1 => self.execute_random_del().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_map_write(&self) {
        match rand::random_range(0..2) {
            0 => self.execute_random_hset().await,
            // TODO: Fix
            // 1 => self.execute_random_hset_exp().await,
            1 => self.execute_random_hdel().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_get(&self) {
        let (key, expected_record) = self.kv_storage.get_random_entry().await;
        self.execute_get(key, &expected_record).await
    }

    async fn execute_get(&self, key: u32, expected_record: &TestRecord) {
        let got_record = self.get(key).await;
        assert_record("get", key, None, expected_record, got_record.as_ref());
    }

    async fn execute_random_set(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        let new_record = random_record(self.namespace.idx(), key, 0);
        self.set(key, &new_record).await;

        record.set(new_record);
    }

    async fn execute_random_get_exp(&self) {
        let (key, expected_record) = self.kv_storage.get_random_entry().await;

        let got_ttl = self.get_exp(key).await;

        assert_exp("get_exp", &expected_record, got_ttl);
    }

    #[allow(dead_code)]
    async fn execute_random_set_exp(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        if !record.definetely_exists() {
            return;
        }

        let new_exp = random_record_expiration();
        self.set_exp(key, new_exp).await;

        record.set_exp(new_exp);
    }

    async fn execute_random_del(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        self.del(key).await;

        record.del();
    }

    async fn execute_random_hget(&self) {
        let (key, map) = self.map_storage.get_random_map().await;
        let (field, expected_record) = map.get_random_entry();

        let got_record = self.hget(key, field).await;

        assert_record(
            "hget",
            key,
            Some(field),
            expected_record,
            got_record.as_ref(),
        );
    }

    async fn execute_random_hset(&self) {
        let (key, mut map) = self.map_storage.get_random_map_mut().await;
        let (field, record) = map.get_random_entry_mut();

        let new_record = random_record(self.namespace.idx(), key, field);
        self.hset(key, field, &new_record).await;

        record.set(new_record);
    }

    async fn execute_random_hget_exp(&self) {
        let (key, map) = self.map_storage.get_random_map().await;
        let (field, expected_record) = map.get_random_entry();

        let got_ttl = self.hget_exp(key, field).await;

        assert_exp("hget_exp", expected_record, got_ttl);
    }

    #[allow(dead_code)]
    async fn execute_random_hset_exp(&self) {
        let (key, mut map) = self.map_storage.get_random_map_mut().await;
        let (field, record) = map.get_random_entry_mut();

        if !record.definetely_exists() {
            return;
        }

        let new_exp = random_record_expiration();
        self.hset_exp(key, field, new_exp).await;

        record.set_exp(new_exp);
    }

    async fn execute_random_hdel(&self) {
        let (key, mut map) = self.map_storage.get_random_map_mut().await;
        let field = rand::random();

        self.hdel(key, field).await;

        map.entries.get_mut(&field).unwrap().del();
    }

    async fn execute_random_hcard(&self) {
        let (key, map) = self.map_storage.get_random_map().await;

        let expected = map.expected_count();
        let got = self.hcard(key).await;

        assert!(
            expected.contains(&got),
            "expected: {expected:?}, got: {got}",
        );
    }

    async fn execute_random_hscan(&self) {
        let (key, map) = self.map_storage.get_random_map().await;

        let count = (rand::random::<u8>() as u32) + 1;
        let cursor = rand::random::<bool>().then(rand::random);
        self.execute_hscan(key, &map, count, cursor).await
    }

    async fn execute_hscan(&self, key: u32, map: &Map, count: u32, cursor: Option<u8>) {
        let page = self.hscan(key, count, cursor).await;

        let mut guard = HScanTestCaseGuard {
            count,
            cursor,
            map,
            page: &page,
            disarmed: false,
        };

        let mut start_at = 0;
        if let Some(cur) = &cursor {
            if let Some(at) = cur.checked_add(1) {
                start_at = at;
            } else {
                // cursor is 255 (the end of the keyspace), no data should be present
                assert!(page.entries.is_empty(), "map should be empty");

                guard.disarmed = true;
                return;
            }
        };

        let got_count = page.entries.len() as u32;

        if page.has_next {
            assert_eq!(
                count, got_count,
                "has_next is present, but counts do not match"
            )
        }

        let mut got_iter = page.entries.iter().peekable();
        let mut expected_iter = map
            .entries
            .range(start_at..)
            .filter(|(_, rec)| rec.maybe_exists());

        let mut actual_count = 0;

        loop {
            let Some(got) = got_iter.peek() else {
                if actual_count < count {
                    let next = expected_iter.find(|(_, record)| record.definetely_exists());
                    assert!(
                        next.is_none(),
                        "MapPage incomplete, expected next: {next:?}"
                    );
                }

                guard.disarmed = true;
                return;
            };

            let (expected_field, expected_record) = expected_iter.next().unwrap_or_else(|| {
                panic!("Map missing record");
            });

            let expected_field = vec![*expected_field];

            if got.field > expected_field && expected_record.maybe_expired() {
                continue;
            }

            let got = got_iter.next().unwrap();

            assert_eq!(
                expected_field, got.field,
                "Expected: {expected_record:?}, got: {:?}",
                got.record
            );

            actual_count += 1;
            assert!(
                actual_count <= count,
                "hscan returned more records than requested",
            );

            assert_record(
                "hscan",
                key,
                Some(expected_field[0]),
                expected_record,
                Some(&got.record),
            );
        }
    }

    async fn get(&self, key: u32) -> Option<Record> {
        self.client
            .get(self.namespace, expand_key(key))
            .pipe(|fut| self.stats.record(fut, "get"))
            .await
            .unwrap()
            .map(shrink_record)
    }

    async fn set(&self, key: u32, record: &Record) {
        self.client
            .set(
                self.namespace,
                expand_key(key),
                expand_value(self.namespace.idx(), key, 0, *record.value.last().unwrap()),
                record.expiration,
            )
            .pipe(|fut| self.stats.record(fut, "set"))
            .await
            .unwrap();
    }

    async fn del(&self, key: u32) {
        self.client
            .del(self.namespace, expand_key(key))
            .pipe(|fut| self.stats.record(fut, "del"))
            .await
            .unwrap();
    }

    async fn get_exp(&self, key: u32) -> Option<Duration> {
        self.client
            .get_exp(self.namespace, expand_key(key))
            .pipe(|fut| self.stats.record(fut, "get_exp"))
            .await
            .unwrap()
    }

    #[allow(dead_code)]
    async fn set_exp(&self, key: u32, exp: RecordExpiration) {
        self.client
            .set_exp(self.namespace, expand_key(key), exp)
            .pipe(|fut| self.stats.record(fut, "set_exp"))
            .await
            .unwrap();
    }

    async fn hget(&self, key: u32, field: u8) -> Option<Record> {
        self.client
            .hget(self.namespace, expand_key(key), expand_field(field))
            .pipe(|fut| self.stats.record(fut, "hget"))
            .await
            .unwrap()
            .map(shrink_record)
    }

    async fn hset(&self, key: u32, field: u8, record: &Record) {
        self.client
            .hset(
                self.namespace,
                expand_key(key),
                expand_field(field),
                expand_value(
                    self.namespace.idx(),
                    key,
                    field,
                    *record.value.last().unwrap(),
                ),
                record.expiration,
            )
            .pipe(|fut| self.stats.record(fut, "hset"))
            .await
            .unwrap();
    }

    async fn hdel(&self, key: u32, field: u8) {
        self.client
            .hdel(self.namespace, expand_key(key), expand_field(field))
            .pipe(|fut| self.stats.record(fut, "hdel"))
            .await
            .unwrap();
    }

    async fn hget_exp(&self, key: u32, field: u8) -> Option<Duration> {
        self.client
            .hget_exp(self.namespace, expand_key(key), expand_field(field))
            .pipe(|fut| self.stats.record(fut, "hget_exp"))
            .await
            .unwrap()
    }

    #[allow(dead_code)]
    async fn hset_exp(&self, key: u32, field: u8, exp: RecordExpiration) {
        self.client
            .hset_exp(self.namespace, expand_key(key), expand_field(field), exp)
            .pipe(|fut| self.stats.record(fut, "hset_exp"))
            .await
            .unwrap();
    }

    async fn hcard(&self, key: u32) -> u64 {
        self.client
            .hcard(self.namespace, expand_key(key))
            .pipe(|fut| self.stats.record(fut, "hcard"))
            .await
            .unwrap()
    }

    async fn hscan(&self, key: u32, count: u32, cursor: Option<u8>) -> MapPage {
        self.client
            .hscan(
                self.namespace,
                expand_key(key),
                count,
                cursor.map(expand_field),
            )
            .pipe(|fut| self.stats.record(fut, "hscan"))
            .await
            .map(shrink_map_page)
            .unwrap()
    }
}

#[derive(Clone, Default)]
struct Stats {
    operation: Arc<Mutex<HashMap<&'static str, OperationStats>>>,
}

impl Stats {
    async fn record<F: Future>(&self, fut: F, op_name: &'static str) -> F::Output {
        let start_time = Instant::now();
        let output = fut.await;
        let duration = start_time.elapsed();

        self.operation
            .lock()
            .await
            .entry(op_name)
            .or_default()
            .pipe(|stats| {
                stats.executions += 1;
                stats.total_duration += duration;
                stats.max_duration = stats.max_duration.max(duration);
            });

        output
    }

    async fn logger(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            self.log().await;
        }
    }

    async fn log(&self) {
        let operation = self.operation.lock().await.clone();
        let mut s = "Operation stats: ".to_string();
        for name in [
            "get", "set", "del", "get_exp", "set_exp", "hget", "hset", "hdel", "hget_exp",
            "hset_exp", "hcard", "hscan",
        ] {
            let Some(stats) = operation.get(name) else {
                continue;
            };

            let executions = stats.executions;
            let avg_duration = stats.avg_duration().as_millis();
            let max_duration = stats.max_duration.as_millis();
            s.push_str(&format!(
                "{name}({executions}|{avg_duration}ms|{max_duration}ms) "
            ));
        }

        tracing::info!("{}", s);
    }
}

#[derive(Clone, Default)]
struct OperationStats {
    executions: usize,
    total_duration: Duration,
    max_duration: Duration,
}

impl OperationStats {
    fn avg_duration(&self) -> Duration {
        self.total_duration / self.executions as u32
    }
}

fn random_record(ns: u8, key: u32, field: u8) -> Record {
    Record {
        value: iter::once(ns)
            .chain(key.to_be_bytes())
            .chain(iter::once(field))
            .chain(iter::once(rand::random()))
            .collect(),
        expiration: random_record_expiration(),
        version: RecordVersion::now(),
    }
}

fn random_record_expiration() -> RecordExpiration {
    Duration::from_secs(rand::random_range(30..300)).into()
}

#[derive(Debug, Default)]
struct TestRecord {
    inner: Option<Record>,
    change_log: Vec<RecordChange>,
}

impl TestRecord {
    fn definetely_exists(&self) -> bool {
        self.expires_at() > now() + 5
    }

    fn maybe_exists(&self) -> bool {
        self.expires_at() >= now() - 5
    }

    fn maybe_expired(&self) -> bool {
        self.expires_at() <= now() + 5
    }

    fn expires_at(&self) -> u64 {
        self.inner
            .as_ref()
            .map(|rec| rec.expiration.to_unix_timestamp_secs())
            .unwrap_or_default()
    }

    fn push_change(&mut self, change: RecordChange) {
        if self.change_log.len() == 5 {
            self.change_log.remove(0);
        }

        self.change_log.push(change);
    }

    fn set(&mut self, record: Record) {
        self.inner = Some(record.clone());
        self.push_change(RecordChange::Set(record));
    }

    fn set_exp(&mut self, expiration: RecordExpiration) {
        self.inner.as_mut().unwrap().expiration = expiration;
        self.push_change(RecordChange::SetExp(expiration));
    }

    fn del(&mut self) {
        self.inner = None;
        self.push_change(RecordChange::Del);
    }
}

impl From<Record> for TestRecord {
    fn from(rec: Record) -> Self {
        Self {
            inner: Some(rec.clone()),
            change_log: vec![RecordChange::Set(rec)],
        }
    }
}

#[allow(dead_code)]
enum RecordChange {
    Set(Record),
    SetExp(RecordExpiration),
    Del,
}

impl fmt::Debug for RecordChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set(rec) => f.debug_tuple("Set").field(&DebugRecord(rec)).finish(),
            Self::SetExp(exp) => f
                .debug_tuple("SetExp")
                .field(&exp.to_unix_timestamp_secs())
                .finish(),
            Self::Del => write!(f, "Del"),
        }
    }
}

struct DebugRecord<'a>(&'a Record);

impl<'a> fmt::Debug for DebugRecord<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Record")
            .field(self.0.value.last().unwrap())
            .field(&self.0.expiration.to_unix_timestamp_secs())
            .field(&(self.0.version.to_unix_timestamp_micros() / 1_000_000))
            .finish()
    }
}

struct KvStorage {
    entries: HashMap<u32, RwLock<TestRecord>>,
}

impl KvStorage {
    fn new() -> Self {
        Self {
            entries: (0..KV_KEYS)
                .map(|key| (key, RwLock::new(TestRecord::default())))
                .collect(),
        }
    }

    async fn get_random_entry(&self) -> (u32, RwLockReadGuard<'_, TestRecord>) {
        let key = rand::random_range(0..KV_KEYS);
        (key, self.entries.get(&key).unwrap().read().await)
    }

    async fn get_random_entry_mut(&self) -> (u32, RwLockWriteGuard<'_, TestRecord>) {
        let key = rand::random_range(0..KV_KEYS);
        (key, self.entries.get(&key).unwrap().write().await)
    }
}

struct MapStorage {
    entries: HashMap<u32, RwLock<Map>>,
}

impl MapStorage {
    fn new() -> Self {
        Self {
            entries: (0..MAP_KEYS)
                .map(|key| (key, RwLock::new(Map::new())))
                .collect(),
        }
    }

    async fn get_random_map(&self) -> (u32, RwLockReadGuard<'_, Map>) {
        let key = rand::random_range(0..MAP_KEYS);
        (key, self.entries.get(&key).unwrap().read().await)
    }

    async fn get_random_map_mut(&self) -> (u32, RwLockWriteGuard<'_, Map>) {
        let key = rand::random_range(0..MAP_KEYS);
        (key, self.entries.get(&key).unwrap().write().await)
    }
}

#[derive(Debug)]
struct Map {
    entries: BTreeMap<u8, TestRecord>,
}

impl Map {
    fn new() -> Self {
        Self {
            entries: (0..=u8::MAX)
                .map(|field| (field, TestRecord::default()))
                .collect(),
        }
    }

    fn get_random_entry(&self) -> (u8, &TestRecord) {
        let field = rand::random();
        (field, self.entries.get(&field).unwrap())
    }

    fn get_random_entry_mut(&mut self) -> (u8, &mut TestRecord) {
        let field = rand::random();
        (field, self.entries.get_mut(&field).unwrap())
    }

    fn expected_count(&self) -> RangeInclusive<u64> {
        self.entries
            .iter()
            .fold(
                (0u64, 0u64),
                |(mut count, mut uncertain_count), (_, rec)| {
                    if let Some(rec) = &rec.inner {
                        let now = now();
                        let expires_at = rec.expiration.to_unix_timestamp_secs();

                        if expires_at > now + 5 {
                            count += 1
                        } else if now.abs_diff(expires_at) <= 5 {
                            uncertain_count += 1
                        }
                    }

                    (count, uncertain_count)
                },
            )
            .pipe(|(count, uncertain_count)| {
                count.checked_sub(uncertain_count).unwrap_or_default()..=count + uncertain_count
            })
    }
}

fn expand_key(key: u32) -> Vec<u8> {
    std::iter::repeat(key.to_be_bytes())
        .flatten()
        .take(KEY_SIZE)
        .collect()
}

fn expand_field(field: u8) -> Vec<u8> {
    std::iter::repeat_n(field, FIELD_SIZE).collect()
}

fn expand_value(ns: u8, key: u32, field: u8, value: u8) -> Vec<u8> {
    iter::once(ns)
        .chain(key.to_be_bytes())
        .chain(iter::once(field))
        .chain(iter::repeat(value))
        .take(VALUE_SIZE)
        .collect()
}

fn shrink_field(mut field: Vec<u8>) -> Vec<u8> {
    assert_eq!(field.len(), FIELD_SIZE);

    for &byte in &field {
        assert_eq!(byte, field[0])
    }

    field.truncate(1);
    field
}

fn shrink_value(mut value: Vec<u8>) -> Vec<u8> {
    assert_eq!(value.len(), VALUE_SIZE);

    let last = *value.last().unwrap();

    for &byte in value.iter().skip(6) {
        assert_eq!(byte, last)
    }

    value.truncate(7);
    value
}

fn shrink_map_page(mut page: MapPage) -> MapPage {
    page.entries = page
        .entries
        .into_iter()
        .map(|mut entry| {
            entry.field = shrink_field(entry.field);
            entry.record.value = shrink_value(entry.record.value);
            entry
        })
        .collect();

    page
}

fn shrink_record(mut record: Record) -> Record {
    record.value = shrink_value(record.value);
    record
}

fn assert_record(
    operation: &'static str,
    key: u32,
    field: Option<u8>,
    expected: &TestRecord,
    got: Option<&Record>,
) {
    let got = got.as_ref();

    let fail = || panic!("{key}:{field:?} | Expected: {expected:?}, got: {got:?} ({operation})");

    let (expected, got) = match (&expected.inner, got) {
        (Some(a), Some(b)) => (a, b),
        (Some(_), None) if expected.maybe_expired() => {
            return;
        }
        (None, None) => return,
        _ => fail(),
    };

    let values_eq = expected.value[0] == got.value[0];
    let expirations_eq = expected
        .expiration
        .to_unix_timestamp_secs()
        .abs_diff(got.expiration.to_unix_timestamp_secs())
        <= 5;

    if !(values_eq & expirations_eq) {
        fail();
    }
}

fn assert_exp(operation: &'static str, expected: &TestRecord, got: Option<Duration>) {
    let fail = || panic!("Expected: {expected:?}, got: {got:?} ({operation})");

    let (expected, got) = match (&expected.inner, got) {
        (Some(a), Some(b)) => (a, b),
        (Some(_), None) if expected.maybe_expired() => {
            return;
        }
        (None, None) => return,
        _ => fail(),
    };

    if Duration::from(expected.expiration).abs_diff(got) > Duration::from_secs(5) {
        fail();
    }
}

fn now() -> u64 {
    OffsetDateTime::now_utc().unix_timestamp() as u64
}

struct HScanTestCaseGuard<'a> {
    count: u32,
    cursor: Option<u8>,
    map: &'a Map,
    page: &'a MapPage,

    disarmed: bool,
}

impl<'a> Drop for HScanTestCaseGuard<'a> {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }

        dbg!(
            self.count,
            self.cursor,
            self.page.entries.len(),
            self.page.has_next,
            now()
        );

        let mut rows: [(&TestRecord, Option<&Record>); 256] =
            array::from_fn(|idx| (self.map.entries.get(&(idx as u8)).unwrap(), None));

        for entry in &self.page.entries {
            rows[entry.field[0] as usize].1 = Some(&entry.record);
        }

        for (idx, row) in rows.iter().enumerate() {
            let expected = row.0.inner.as_ref().map(DebugRecord);
            let got = row.1.map(DebugRecord);

            eprintln!("{idx} | {expected:?} | {got:?} | {:?}", &row.0.change_log);
        }
    }
}
