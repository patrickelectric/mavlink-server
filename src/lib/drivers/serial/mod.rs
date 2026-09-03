use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use mavlink_codec::codec::MavlinkCodec;
use tokio::sync::{RwLock, broadcast};
use tokio_serial::{self, SerialPortBuilderExt};
use tokio_util::codec::Framed;
use tracing::*;

use crate::{
    callbacks::{Callbacks, MessageCallback},
    drivers::{
        Driver, DriverInfo,
        generic_tasks::{SendReceiveContext, default_send_receive_run},
    },
    protocol::Protocol,
    stats::{
        accumulated::driver::{AccumulatedDriverStats, AccumulatedDriverStatsProvider},
        driver::DriverUuid,
    },
};

#[derive(Debug)]
pub struct Serial {
    name: arc_swap::ArcSwap<String>,
    uuid: DriverUuid,
    pub port_name: String,
    pub baud_rate: u32,
    on_message_input: Callbacks<Arc<Protocol>>,
    on_message_output: Callbacks<Arc<Protocol>>,
    stats: Arc<RwLock<AccumulatedDriverStats>>,
}

pub struct SerialBuilder(Serial);

impl SerialBuilder {
    pub fn build(self) -> Serial {
        self.0
    }

    pub fn on_message_input<C>(self, callback: C) -> Self
    where
        C: MessageCallback<Arc<Protocol>>,
    {
        self.0.on_message_input.add_callback(callback.into_boxed());
        self
    }

    pub fn on_message_output<C>(self, callback: C) -> Self
    where
        C: MessageCallback<Arc<Protocol>>,
    {
        self.0.on_message_output.add_callback(callback.into_boxed());
        self
    }
}

impl Serial {
    #[instrument(level = "debug")]
    pub fn builder(name: &str, port_name: &str, baud_rate: u32) -> SerialBuilder {
        let name = Arc::new(name.to_string());

        SerialBuilder(Self {
            name: arc_swap::ArcSwap::new(name.clone()),
            uuid: Self::generate_uuid(&format!("{port_name}:{baud_rate}")),
            port_name: port_name.to_string(),
            baud_rate,
            on_message_input: Callbacks::default(),
            on_message_output: Callbacks::default(),
            stats: Arc::new(RwLock::new(AccumulatedDriverStats::new(
                name,
                &SerialInfo,
                &format!("{port_name}:{baud_rate}"),
            ))),
        })
    }
}

#[async_trait::async_trait]
impl Driver for Serial {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let port_name = self.port_name.clone();

        let context = SendReceiveContext {
            direction: crate::drivers::Direction::Both,
            hub_sender,
            on_message_output: self.on_message_output.clone(),
            on_message_input: self.on_message_input.clone(),
            stats: self.stats.clone(),
        };

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
        let mut first = true;
        loop {
            if first {
                first = false;
            } else {
                interval.tick().await;
            }

            debug!("Trying to connect...");

            let stream = match tokio_serial::new(&port_name, self.baud_rate)
                .timeout(tokio::time::Duration::from_secs(1))
                .open_native_async()
            {
                Ok(stream) => stream,
                Err(error) => {
                    error!("Failed to open serial port {port_name:?}: {error:?}");
                    continue;
                }
            };

            debug!("Successfully connected");

            let codec = MavlinkCodec::<true, true, false, false, false, false>::default();
            let (writer, reader) = Framed::new(stream, codec).split();

            if let Err(reason) =
                default_send_receive_run(writer, reader, &port_name, &context).await
            {
                warn!("Driver send/receive tasks closed: {reason}");
            }

            debug!("Restarting connection loop...");
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        Box::new(SerialInfo)
    }

    fn name(&self) -> Arc<String> {
        self.name.load_full()
    }

    fn uuid(&self) -> &DriverUuid {
        &self.uuid
    }
}

#[async_trait::async_trait]
impl AccumulatedDriverStatsProvider for Serial {
    async fn stats(&self) -> AccumulatedDriverStats {
        self.stats.read().await.clone()
    }

    async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        stats.stats.input = None;
        stats.stats.output = None
    }
}

pub struct SerialInfo;
impl DriverInfo for SerialInfo {
    fn name(&self) -> &'static str {
        "Serial"
    }
    fn valid_schemes(&self) -> &'static [&'static str] {
        &["serial"]
    }

    fn cli_example_legacy(&self) -> Vec<String> {
        let first_schema = self.valid_schemes()[0];

        vec![
            format!("{first_schema}:<PORT>:<BAUDRATE>"),
            format!("{first_schema}:/dev/ttyACM0:115200"),
            format!("{first_schema}:COM:57600"),
        ]
    }

    fn cli_example_url(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}://<PORT>?baudrate=<BAUDRATE?>").to_string(),
            url::Url::parse(&format!("{first_schema}:///dev/ttyACM0?baudrate=115200"))
                .unwrap()
                .to_string(),
            url::Url::parse(&format!("{first_schema}://COM1?baudrate=57600"))
                .unwrap()
                .to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, url: &url::Url) -> Option<Arc<dyn Driver>> {
        let (port_name, baud_rate) = port_and_baud_from_url(url);
        Some(Arc::new(
            Serial::builder("Serial", &port_name, baud_rate).build(),
        ))
    }
}

fn port_and_baud_from_url(url: &url::Url) -> (String, u32) {
    let mut port_name = url.path().to_string();
    // Keep the slash on unix paths like /dev/ttyACM0. Strip it on COM
    // ports: serialport prepends \\.\ unless the path already starts
    // with \, so /COM3 fails to open.
    if port_name.is_empty() || port_name == "/" {
        port_name = url.host_str().unwrap_or("").to_string();
    } else if port_name
        .trim_start_matches('/')
        .to_ascii_uppercase()
        .starts_with("COM")
    {
        port_name = port_name.trim_start_matches('/').to_string();
    }

    let baud_rate = url
        .query_pairs()
        .find_map(|(key, value)| {
            if key == "baudrate" || key == "arg2" {
                value.parse().ok()
            } else {
                None
            }
        })
        .or_else(|| url.port().map(u32::from))
        .unwrap_or(115200); // Commun baudrate between flight controllers

    (port_name, baud_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::{DriverDescriptionLegacy, DriverInfo, Type};

    #[test]
    fn port_and_baud_from_url_parses_windows_and_unix() {
        let cases = [
            ("serial://COM3:57600", "COM3", 57600),
            ("serial://COM1?baudrate=57600", "COM1", 57600),
            ("serial://COM1?arg2=115200", "COM1", 115200),
            ("serial:///COM3?baudrate=57600", "COM3", 57600),
            (
                "serial:///dev/ttyACM0?baudrate=115200",
                "/dev/ttyACM0",
                115200,
            ),
        ];

        for (input, expected_port, expected_baud) in cases {
            let url = url::Url::parse(input).unwrap();
            let (port, baud) = port_and_baud_from_url(&url);
            assert_eq!(port, expected_port, "{input}");
            assert_eq!(baud, expected_baud, "{input}");
        }
    }

    #[test]
    fn port_and_baud_from_legacy_com() {
        let url = SerialInfo
            .url_from_legacy(DriverDescriptionLegacy {
                typ: Type::Serial,
                arg1: "COM3".to_string(),
                arg2: Some("57600".to_string()),
            })
            .unwrap();
        assert_eq!(port_and_baud_from_url(&url), ("COM3".to_string(), 57600));
    }
}
