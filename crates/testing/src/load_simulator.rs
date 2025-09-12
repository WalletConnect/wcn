use {
    crate::WcnClient,
    futures::{FutureExt as _, StreamExt as _, stream},
    futures_concurrency::future::Race as _,
    rand::seq::IndexedRandom,
    std::{
        cmp,
        collections::{BTreeMap, HashMap},
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
    wcn_client::MapPage,
    wcn_storage_api::{Record, RecordExpiration, RecordVersion},
};

const KV_KEYS: u32 = 5_000;
const MAP_KEYS: u32 = 50;

const KEY_SIZE: usize = 32;
const FIELD_SIZE: usize = 32;
const VALUE_SIZE: usize = 1024;

pub(super) async fn run(client: WcnClient, namespaces: &[wcn_client::Namespace]) {
    let stats = Stats::default();

    let mut namespaces: Vec<_> = namespaces
        .iter()
        .copied()
        .map(|ns| Namespace {
            namespace: ns,
            client: client.clone(),
            kv_storage: KvStorage::default(),
            map_storage: MapStorage::default(),
            stats: stats.clone(),
        })
        .collect();

    stream::iter(namespaces.iter_mut())
        .for_each_concurrent(5, Namespace::populate)
        .await;

    let fut = async move {
        stream::iter(std::iter::repeat(()))
            .for_each_concurrent(10, |_| {
                namespaces
                    .choose(&mut rand::rng())
                    .unwrap()
                    .execute_random_operation()
            })
            .await;
    };

    (fut, stats.logger()).race().await;
}

struct Namespace {
    namespace: wcn_client::Namespace,
    client: WcnClient,

    kv_storage: KvStorage,
    map_storage: MapStorage,

    stats: Stats,
}

impl Namespace {
    async fn populate(&mut self) {
        let namespace = self.namespace;
        let client = &self.client;

        let count = &AtomicUsize::new(0);
        let progress_logger = || async {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;
                tracing::info!("Populated {} keys", count.load(atomic::Ordering::Relaxed));
            }
        };

        let populate_kv_fut = stream::iter(0..KV_KEYS)
            .map(|key| async move {
                let record = random_record();

                client
                    .set(
                        namespace,
                        expand_key(key),
                        expand_value(record.value[0]),
                        record.expiration,
                    )
                    .await
                    .unwrap();

                count.fetch_add(1, atomic::Ordering::Relaxed);

                (key, RwLock::new(Some(record)))
            })
            .buffer_unordered(100)
            .collect();

        self.kv_storage.entries = (populate_kv_fut, progress_logger()).race().await;

        let populate_map_fut = stream::iter(0..MAP_KEYS)
            .map(|key| async move {
                let max_field: u8 = rand::random();

                let entries = stream::iter(0..=max_field)
                    .map(|field| async move {
                        let record = random_record();

                        client
                            .hset(
                                namespace,
                                expand_key(key),
                                expand_field(field),
                                expand_value(record.value[0]),
                                record.expiration,
                            )
                            .await
                            .unwrap();

                        (field, record)
                    })
                    .buffer_unordered(max_field as usize + 1)
                    .collect()
                    .await;

                count.fetch_add(1, atomic::Ordering::Relaxed);

                (key, RwLock::new(Map { entries }))
            })
            .buffer_unordered(1)
            .collect();

        count.store(0, atomic::Ordering::Relaxed);
        let logger_fut = progress_logger().map(|_| unreachable!());
        self.map_storage.entries = (populate_map_fut, logger_fut).race().await;
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
            // TODO: Still causes inconsistencies during migrations
            // 1 => self.execute_random_set_exp().await,
            1 => self.execute_random_del().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_map_write(&self) {
        match rand::random_range(0..2) {
            0 => self.execute_random_hset().await,
            // 1 => self.execute_random_hset_exp().await,
            1 => self.execute_random_hdel().await,
            _ => unreachable!(),
        }
    }

    async fn execute_random_get(&self) {
        let (key, expected_record) = self.kv_storage.get_random_entry().await;

        let got_record = self.get(key).await;

        assert_record("get", expected_record.as_ref(), got_record.as_ref());
    }

    async fn execute_random_set(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        let new_record = random_record();
        self.set(key, &new_record).await;

        *record = Some(new_record);
    }

    async fn execute_random_get_exp(&self) {
        let (key, expected_record) = self.kv_storage.get_random_entry().await;

        let got_ttl = self.get_exp(key).await;

        assert_exp("get_exp", expected_record.as_ref(), got_ttl);
    }

    #[allow(dead_code)]
    async fn execute_random_set_exp(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        if let Some(rec) = record.as_ref()
            && is_expired_or_about_to_expire(rec)
        {
            return;
        }

        let new_exp = random_record_expiration();
        self.set_exp(key, new_exp).await;

        if let Some(rec) = record.as_mut() {
            rec.expiration = new_exp;
        }
    }

    async fn execute_random_del(&self) {
        let (key, mut record) = self.kv_storage.get_random_entry_mut().await;

        self.del(key).await;

        *record = None;
    }

    async fn execute_random_hget(&self) {
        let (key, map) = self.map_storage.get_random_map().await;
        let (field, expected_record) = map.get_random_entry();

        let got_record = self.hget(key, field).await;

        assert_record("hget", expected_record, got_record.as_ref());
    }

    async fn execute_random_hset(&self) {
        let (key, mut map) = self.map_storage.get_random_map_mut().await;
        let field = rand::random();

        let new_record = random_record();
        self.hset(key, field, &new_record).await;

        let _ = map.entries.insert(field, new_record);
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

        if let Some(rec) = &record
            && is_expired_or_about_to_expire(rec)
        {
            return;
        }

        let new_exp = random_record_expiration();
        self.hset_exp(key, field, new_exp).await;

        if let Some(rec) = record {
            rec.expiration = new_exp;
        }
    }

    async fn execute_random_hdel(&self) {
        let (key, mut map) = self.map_storage.get_random_map_mut().await;
        let field = rand::random();

        self.hdel(key, field).await;

        let _ = map.entries.remove(&field);
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

        let page = self.hscan(key, count, cursor).await;

        let mut start_at = 0;
        if let Some(cur) = &cursor {
            if let Some(at) = cur.checked_add(1) {
                start_at = at;
            } else {
                // cursor is 255 (the end of the keyspace), no data should be present
                assert_eq!(page.entries, vec![]);
                return;
            }
        };

        let mut got_iter = page.entries.iter().peekable();
        let mut expected_iter = map.entries.range(start_at..);

        let mut actual_count = 0;

        let now = OffsetDateTime::now_utc().unix_timestamp();

        loop {
            let Some(got) = got_iter.peek() else {
                if actual_count < count {
                    let next =
                        expected_iter.find(|(_, record)| !is_expired_or_about_to_expire(record));

                    assert_eq!(
                        next, None,
                        "count: {count}, actual_count: {actual_count}, cursor: {cursor:?}, map: \
                         {map:?}, page: {page:?}, now: {now}"
                    );
                }

                return;
            };

            let (expected_field, expected_record) = expected_iter.next().unwrap_or_else(|| {
                panic!("Missing record | count: {count}, cursor: {cursor:?}, map: {map:?}")
            });
            let expected_field = vec![*expected_field];

            let got = match got.field.cmp(&expected_field) {
                cmp::Ordering::Less => panic!("hscan out of order"),
                cmp::Ordering::Equal => got_iter.next().unwrap(),
                cmp::Ordering::Greater if is_expired_or_about_to_expire(expected_record) => {
                    continue;
                }
                cmp::Ordering::Greater => panic!(
                    "MapPage missing {expected_field:?}->{expected_record:?} | count: {count}, \
                     cursor: {cursor:?}, map: {map:?}"
                ),
            };

            actual_count += 1;
            assert!(
                actual_count <= count,
                "hscan returned more records than requested(count: {count}, actual_count: {})",
                actual_count
            );

            assert_eq!(
                got.field, expected_field,
                "count: {count}, cursor: {cursor:?}, got_record: {:?}, expected_record: \
                 {expected_record:?}, map: {map:?}, page: {page:?}",
                got.record,
            );

            assert_record("hscan", Some(expected_record), Some(&got.record));
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
                expand_value(record.value[0]),
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
                expand_value(record.value[0]),
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

fn random_record() -> Record {
    Record {
        value: vec![rand::random()],
        expiration: random_record_expiration(),
        version: RecordVersion::now(),
    }
}

fn random_record_expiration() -> RecordExpiration {
    Duration::from_secs(rand::random_range(30..300)).into()
}

fn is_expired(record: &Record) -> bool {
    Duration::from(record.expiration).is_zero()
}

fn is_expired_or_about_to_expire(record: &Record) -> bool {
    is_expired(record) | is_almost_now(record.expiration)
}

fn is_almost_now(expiration: RecordExpiration) -> bool {
    let expires_at = expiration.to_unix_timestamp_secs();
    let now = OffsetDateTime::now_utc().unix_timestamp() as u64;

    expires_at.abs_diff(now) <= 5
}

#[derive(Default)]
struct KvStorage {
    entries: HashMap<u32, RwLock<Option<Record>>>,
}

impl KvStorage {
    async fn get_random_entry(&self) -> (u32, RwLockReadGuard<'_, Option<Record>>) {
        let key = rand::random_range(0..KV_KEYS);
        (key, self.entries.get(&key).unwrap().read().await)
    }

    async fn get_random_entry_mut(&self) -> (u32, RwLockWriteGuard<'_, Option<Record>>) {
        let key = rand::random_range(0..KV_KEYS);
        (key, self.entries.get(&key).unwrap().write().await)
    }
}

#[derive(Default)]
struct MapStorage {
    entries: HashMap<u32, RwLock<Map>>,
}

impl MapStorage {
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
    entries: BTreeMap<u8, Record>,
}

impl Map {
    fn get_random_entry(&self) -> (u8, Option<&Record>) {
        let field = rand::random();
        (field, self.entries.get(&field))
    }

    fn get_random_entry_mut(&mut self) -> (u8, Option<&mut Record>) {
        let field = rand::random();
        (field, self.entries.get_mut(&field))
    }

    fn expected_count(&self) -> RangeInclusive<u64> {
        self.entries
            .iter()
            .fold(
                (0u64, 0u64),
                |(mut count, mut uncertain_count), (_, rec)| {
                    if !is_expired(rec) {
                        count += 1;
                    }

                    if is_almost_now(rec.expiration) {
                        uncertain_count += 1;
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

fn expand_value(value: u8) -> Vec<u8> {
    std::iter::repeat_n(value, VALUE_SIZE).collect()
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

    for &byte in &value {
        assert_eq!(byte, value[0])
    }

    value.truncate(1);
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

fn assert_record(operation: &'static str, expected: Option<&Record>, got: Option<&Record>) {
    let got = got.as_ref();

    let fail = || panic!("Expected: {expected:?}, got: {got:?} ({operation})");

    let (expected, got) = match (expected, got) {
        (Some(a), Some(b)) => (a, b),
        (Some(expected), None) if is_expired_or_about_to_expire(expected) => {
            return;
        }
        (None, None) => return,
        _ => fail(),
    };

    let values_eq = expected.value == got.value;
    let expirations_eq = expected
        .expiration
        .to_unix_timestamp_secs()
        .abs_diff(got.expiration.to_unix_timestamp_secs())
        <= 5;

    if !(values_eq & expirations_eq) {
        fail();
    }
}

fn assert_exp(operation: &'static str, expected: Option<&Record>, got: Option<Duration>) {
    let fail = || panic!("Expected: {expected:?}, got: {got:?} ({operation})");

    let (expected, got) = match (expected, got) {
        (Some(a), Some(b)) => (a, b),
        (Some(expected), None) if is_expired_or_about_to_expire(expected) => {
            return;
        }
        (None, None) => return,
        _ => fail(),
    };

    if Duration::from(expected.expiration).abs_diff(got) > Duration::from_secs(5) {
        fail();
    }
}
