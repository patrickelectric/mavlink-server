mod data;
mod endpoints;
pub mod server;

use std::sync::Arc;

use anyhow::Result;
use mavlink_server::callbacks::{Callbacks, MessageCallback};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{broadcast, RwLock},
};
use tracing::*;

use crate::{
    drivers::{Driver, DriverInfo},
    protocol::Protocol,
    stats::driver::{DriverStats, DriverStatsInfo},
};

pub struct Rest {
    on_message_input: Callbacks<Arc<Protocol>>,
    on_message_output: Callbacks<Arc<Protocol>>,
    stats: Arc<RwLock<DriverStatsInfo>>,
}

pub struct RestBuilder(Rest);

impl RestBuilder {
    pub fn build(self) -> Rest {
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

impl Rest {
    #[instrument(level = "debug")]
    pub fn builder() -> RestBuilder {
        RestBuilder(Self {
            on_message_input: Callbacks::new(),
            on_message_output: Callbacks::new(),
            stats: Arc::new(RwLock::new(DriverStatsInfo::default())),
        })
    }

    /*
    #[instrument(level = "debug", skip(on_message_input))]
    async fn serial_receive_task(
        hub_sender: broadcast::Sender<Arc<Protocol>>,
        on_message_input: &Callbacks<Arc<Protocol>>,
    ) -> Result<()> {
        let mut buf = vec![0; 1024];

        loop {
            match port.lock().await.read(&mut buf).await {
                // We got something
                Ok(bytes_received) if bytes_received > 0 => {
                    read_all_messages("serial", &mut buf, |message| async {
                        let message = Arc::new(message);

                        for future in on_message_input.call_all(Arc::clone(&message)) {
                            if let Err(error) = future.await {
                                debug!("Dropping message: on_message_input callback returned error: {error:?}");
                                continue;
                            }
                        }

                        if let Err(error) = hub_sender.send(message) {
                            error!("Failed to send message to hub: {error:?}");
                        }
                    })
                    .await;
                }
                // We got nothing
                Ok(_) => {
                    break;
                }
                // We got problems
                Err(error) => {
                    error!("Failed to receive serial message: {error:?}");
                    break;
                }
            }
        }

        Ok(())
    }
    */

    #[instrument(level = "debug", skip(on_message_output))]
    async fn serial_send_task(
        mut hub_receiver: broadcast::Receiver<Arc<Protocol>>,
        on_message_output: &Callbacks<Arc<Protocol>>,
    ) -> Result<()> {
        loop {
            match hub_receiver.recv().await {
                Ok(message) => {
                    for future in on_message_output.call_all(Arc::clone(&message)) {
                        if let Err(error) = future.await {
                            debug!("Dropping message: on_message_output callback returned error: {error:?}");
                            continue;
                        }
                    }

                    let mut bytes =
                        mavlink::async_peek_reader::AsyncPeekReader::new(message.raw_bytes());
                    let (header, message): (
                        mavlink::MavHeader,
                        mavlink::ardupilotmega::MavMessage,
                    ) = mavlink::read_v2_msg_async(&mut bytes).await.unwrap();

                    data::update((header, message));
                }
                Err(error) => {
                    error!("Failed to receive message from hub: {error:?}");
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Driver for Rest {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        loop {
            let hub_sender = hub_sender.clone();
            let hub_receiver = hub_sender.subscribe();

            tokio::select! {
                result = Rest::serial_send_task(hub_receiver, &self.on_message_output) => {
                    if let Err(e) = result {
                        error!("Error in rest sender task: {e:?}");
                    }
                }
                /*
                result = Rest::serial_receive_task(hub_sender, &self.on_message_input) => {
                    if let Err(e) = result {
                        error!("Error in rest receive task: {e:?}");
                    }
                }
                */
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        Box::new(RestInfo)
    }
}

#[async_trait::async_trait]
impl DriverStats for Rest {
    async fn stats(&self) -> DriverStatsInfo {
        self.stats.read().await.clone()
    }

    async fn reset_stats(&self) {
        *self.stats.write().await = DriverStatsInfo {
            input: None,
            output: None,
        }
    }
}

pub struct RestInfo;
impl DriverInfo for RestInfo {
    fn name(&self) -> &str {
        "Rest"
    }

    fn valid_schemes(&self) -> Vec<String> {
        vec!["rest".to_string()]
    }

    fn cli_example_legacy(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}:<IP>:<PORT>"),
            format!("{first_schema}:0.0.0.0:8000"),
        ]
    }

    fn cli_example_url(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}://<IP>:<PORT>").to_string(),
            url::Url::parse(&format!("{first_schema}://0.0.0.0:8000"))
                .unwrap()
                .to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, url: &url::Url) -> Option<Arc<dyn Driver>> {
        let host = url.host_str().unwrap();
        let port = url.port().unwrap();
        Some(Arc::new(Rest::builder().build()))
    }
}
