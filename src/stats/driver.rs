use std::sync::Arc;

use indexmap::IndexMap;
use serde::ser::{Serialize, Serializer, SerializeStruct};
use serde::Serialize as SerdeSerialize;

use super::StatsInner;

pub type DriverUuid = uuid::Uuid;

pub type DriversStats = IndexMap<DriverUuid, DriverStats>;

#[derive(Debug, Clone)]
pub struct DriverStats {
    pub name: Arc<String>,
    pub driver_type: &'static str,
    pub stats: DriverStatsInner,
}

impl Serialize for DriverStats {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("Color", 3)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("driver_type", &self.driver_type)?;
        state.serialize_field("stats", &self.stats)?;
        state.end()
    }
}

#[derive(SerdeSerialize, Debug, Clone)]
pub struct DriverStatsInner {
    pub input: Option<StatsInner>,
    pub output: Option<StatsInner>,
}
