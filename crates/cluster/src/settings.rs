//! WCN cluster settings.

use {
    crate::smart_contract,
    serde::{Deserialize, Serialize},
    std::{ops::RangeInclusive, time::Duration},
    strum::{EnumDiscriminants, FromRepr, IntoDiscriminant, IntoStaticStr},
    time::OffsetDateTime,
};

#[allow(unused_imports)] // for doc comments
use crate::{Cluster, Node, NodeOperator, SmartContract};

/// WCN [`Cluster`] settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Maximum number of on-chain bytes stored for a single
    /// [`NodeOperator`].
    pub max_node_operator_data_bytes: u16,

    /// Maximum expected latency of propagating [`SmartContract`] [`Event`]s
    /// across the whole [`Cluster`].
    ///
    /// This should indicate the upper limit of the happy path scenario. If you
    /// specify `10s` it means that once an [`Event`] is emmitted on the
    /// blockchain it is expected that all [`Node`]s within the [`Cluster`]
    /// should receive that event in <= `10s`.
    ///
    /// Misconfiguring this won't result in any catastrophic failures.
    /// Inadequately small value may result in reduced success rate of storage
    /// operations during a data migration process, due
    /// to `KeyspaceVersionMistmatch` errors.
    /// Inadequately large value will result in data migrations being slower,
    /// due to a delay equal to the specified value.
    pub event_propagation_latency: Duration,

    /// Maximum expected local clock (time) skew between any 2 [`Node`]s in the
    /// [`Cluster`].
    ///
    /// Misconfiguring this won't result in any catastrophic failures.
    /// If the actual skew exceeds the specified value you may observe a reduced
    /// success rate of storage operations during a data migration process, due
    /// to `KeyspaceVersionMismatch` errors.
    pub clock_skew: Duration,

    /// Specifies how many shards are allowed to be pulled by a
    /// [`NodeOperator`] at the same time during data migration.
    pub migration_concurrency: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_node_operator_data_bytes: 4096,
            event_propagation_latency: Duration::from_secs(5),
            clock_skew: Duration::from_millis(500),
            migration_concurrency: 10,
        }
    }
}

impl Settings {
    pub(crate) fn event_propagation_time(&self, emitted_at: OffsetDateTime) -> OffsetDateTime {
        emitted_at + self.event_propagation_latency
    }

    pub(crate) fn has_event_propagated(&self, emitted_at: OffsetDateTime) -> bool {
        self.event_propagation_time(emitted_at) <= OffsetDateTime::now_utc()
    }

    pub(crate) fn clock_skew_time_frame(
        &self,
        time: OffsetDateTime,
    ) -> RangeInclusive<OffsetDateTime> {
        time - self.clock_skew..=time + self.clock_skew
    }

    fn extra(&self) -> [Setting; 3] {
        [
            Setting::EventPropagationLatency(self.event_propagation_latency),
            Setting::ClockSkew(self.clock_skew),
            Setting::MigrationConcurrency(self.migration_concurrency),
        ]
    }

    fn apply_extra_setting(&mut self, setting: Setting) {
        match setting {
            Setting::EventPropagationLatency(setting) => self.event_propagation_latency = setting,
            Setting::ClockSkew(setting) => self.clock_skew = setting,
            Setting::MigrationConcurrency(setting) => self.migration_concurrency = setting,
        };
    }
}

impl TryFrom<Settings> for smart_contract::Settings {
    type Error = TryIntoSmartContractError;

    fn try_from(settings: Settings) -> Result<Self, Self::Error> {
        let extra = ExtraV0::from(settings);

        let size = postcard::experimental::serialized_size(&extra)
            .map_err(TryIntoSmartContractError::from_postcard)?;

        // reserve first byte for versioning
        let mut buf = vec![0; size + 1];
        buf[0] = 0; // current schema version
        postcard::to_slice(&extra, &mut buf[1..])
            .map_err(TryIntoSmartContractError::from_postcard)?;

        Ok(smart_contract::Settings {
            max_node_operator_data_bytes: settings.max_node_operator_data_bytes,
            extra: buf,
        })
    }
}

impl TryFrom<smart_contract::Settings> for Settings {
    type Error = TryFromSmartContractError;

    fn try_from(sc_settings: smart_contract::Settings) -> Result<Self, Self::Error> {
        let bytes = sc_settings.extra;

        if bytes.is_empty() {
            return Err(TryFromSmartContractError::EmptyBuffer);
        }

        let schema_version = bytes[0];
        let bytes = &bytes[1..];

        let mut settings = match schema_version {
            0 => postcard::from_bytes::<ExtraV0>(bytes).map(Settings::from),
            1 => postcard::from_bytes::<ExtraV1>(bytes).map(Settings::from),
            ver => return Err(TryFromSmartContractError::UnknownSchemaVersion(ver)),
        }
        .map_err(TryFromSmartContractError::from_postcard)?;

        settings.max_node_operator_data_bytes = sc_settings.max_node_operator_data_bytes;

        Ok(settings)
    }
}

// NOTE: The on-chain serialization is non self-describing!
// This `struct` can not be changed, a `struct` with a new schema version should
// be created instead.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct ExtraV0 {
    pub event_propagation_latency_ms: u32,
    pub clock_skew_ms: u32,
}

impl From<Settings> for ExtraV0 {
    fn from(settings: Settings) -> Self {
        Self {
            event_propagation_latency_ms: settings
                .event_propagation_latency
                .as_millis()
                .try_into()
                .unwrap_or(u32::MAX),
            clock_skew_ms: settings
                .clock_skew
                .as_millis()
                .try_into()
                .unwrap_or(u32::MAX),
        }
    }
}

impl From<ExtraV0> for Settings {
    fn from(extra: ExtraV0) -> Self {
        Self {
            max_node_operator_data_bytes: 0, // will be overwritten
            event_propagation_latency: Duration::from_millis(
                extra.event_propagation_latency_ms.into(),
            ),
            clock_skew: Duration::from_millis(extra.clock_skew_ms.into()),
            migration_concurrency: 10,
        }
    }
}

// NOTE: The on-chain serialization is non self-describing!
// This `struct` can not be changed, a `struct` with a new schema version should
// be created instead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ExtraV1 {
    // NOTE: Or
    encoded: Vec<Vec<u8>>,
}

#[derive(EnumDiscriminants)]
#[strum_discriminants(name(SettingId))]
#[strum_discriminants(derive(IntoStaticStr, FromRepr))]
#[repr(u8)]
enum Setting {
    EventPropagationLatency(Duration) = 0,
    ClockSkew(Duration) = 1,
    MigrationConcurrency(u16) = 2,
}

impl SettingId {
    fn from_idx(idx: usize) -> Option<Self> {
        Self::from_repr(u8::try_from(idx).ok()?)
    }
}

impl Setting {
    fn encode(&self) -> Vec<u8> {
        let encode_duration = |value: &Duration| {
            u32::try_from(value.as_millis())
                .unwrap_or(u32::MAX)
                .to_be_bytes()
                .into()
        };

        match self {
            Self::EventPropagationLatency(latency) => encode_duration(latency),
            Self::ClockSkew(skew) => encode_duration(skew),
            Self::MigrationConcurrency(concurrency) => concurrency.to_be_bytes().into(),
        }
    }

    fn decode(id: SettingId, value: &[u8]) -> Option<Self> {
        let decode_duration = || {
            let millis = u32::from_be_bytes(value.try_into().ok()?);
            Some(Duration::from_millis(millis.into()))
        };

        let decode_u16 = || Some(u16::from_be_bytes(value.try_into().ok()?));

        Some(match id {
            SettingId::EventPropagationLatency => Self::EventPropagationLatency(decode_duration()?),
            SettingId::ClockSkew => Self::ClockSkew(decode_duration()?),
            SettingId::MigrationConcurrency => Self::MigrationConcurrency(decode_u16()?),
        })
    }
}

impl From<Settings> for ExtraV1 {
    fn from(settings: Settings) -> Self {
        let extra_settings = settings.extra();

        let highest_idx = extra_settings
            .iter()
            .map(|setting| setting.discriminant() as usize)
            .max()
            .unwrap_or_default();

        let mut encoded = vec![vec![]; highest_idx + 1];

        for setting in settings.extra() {
            encoded[setting.discriminant() as usize] = setting.encode();
        }

        ExtraV1 { encoded }
    }
}

impl From<ExtraV1> for Settings {
    fn from(extra: ExtraV1) -> Self {
        let mut settings = Settings::default();

        for (idx, value) in extra.encoded.iter().enumerate() {
            let Some(id) = SettingId::from_idx(idx) else {
                tracing::warn!("Unknown SettingId({idx}), ignoring");
                continue;
            };

            let Some(setting) = Setting::decode(id, value) else {
                let name: &'static str = id.into();
                tracing::warn!("Invalid value `{value:?}` for `{name}` setting, ignoring");
                continue;
            };

            settings.apply_extra_setting(setting);
        }

        settings
    }
}

/// Error of converting [`Settings`] into [`smart_contract::Settings`].
#[derive(Debug, thiserror::Error)]
pub enum TryIntoSmartContractError {
    #[error("Codec: {0}")]
    Codec(String),
}

impl TryIntoSmartContractError {
    fn from_postcard(err: postcard::Error) -> Self {
        Self::Codec(format!("{err:?}"))
    }
}

/// Error of converting [`smart_contract::Settings`] into [`Settings`].
#[derive(Debug, thiserror::Error)]
pub enum TryFromSmartContractError {
    #[error("Empty data buffer")]
    EmptyBuffer,

    #[error("Codec: {0}")]
    Codec(String),

    #[error("Unknown schema version: {0}")]
    UnknownSchemaVersion(u8),
}

impl TryFromSmartContractError {
    fn from_postcard(err: postcard::Error) -> Self {
        Self::Codec(format!("{err:?}"))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn extra_v1() {
        let max_node_operator_data_bytes = 10000;

        let expected_settings = Settings {
            max_node_operator_data_bytes,
            event_propagation_latency: Duration::from_secs(42),
            clock_skew: Duration::from_secs(10),
            migration_concurrency: 1000,
        };

        let mut settings: Settings = ExtraV1::from(expected_settings).into();
        // being set outside of `From` impl
        settings.max_node_operator_data_bytes = max_node_operator_data_bytes;

        assert_eq!(expected_settings, settings);
    }
}
