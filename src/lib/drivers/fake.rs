use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, RwLock};
use tracing::*;

use crate::{
    callbacks::{Callbacks, MessageCallback},
    drivers::{Driver, DriverInfo},
    protocol::{read_all_messages, Protocol},
    stats::{
        accumulated::driver::{AccumulatedDriverStats, AccumulatedDriverStatsProvider},
        driver::DriverUuid,
    },
};

pub struct FakeSink {
    name: arc_swap::ArcSwap<String>,
    uuid: DriverUuid,
    on_message_input: Callbacks<Arc<Protocol>>,
    print: bool,
    stats: Arc<RwLock<AccumulatedDriverStats>>,
}

impl FakeSink {
    pub fn builder(name: &str) -> FakeSinkBuilder {
        let name = Arc::new(name.to_string());

        FakeSinkBuilder(Self {
            name: arc_swap::ArcSwap::new(name.clone()),
            uuid: Self::generate_uuid(&name),
            on_message_input: Callbacks::new(),
            print: false,
            stats: Arc::new(RwLock::new(AccumulatedDriverStats::new(
                name,
                &FakeSinkInfo,
            ))),
        })
    }
}

pub struct FakeSinkBuilder(FakeSink);

impl FakeSinkBuilder {
    pub fn build(self) -> FakeSink {
        self.0
    }

    pub fn print(mut self) -> Self {
        self.0.print = true;
        self
    }

    pub fn on_message_input<C>(self, callback: C) -> Self
    where
        C: MessageCallback<Arc<Protocol>>,
    {
        self.0.on_message_input.add_callback(callback.into_boxed());
        self
    }
}

#[async_trait::async_trait]
impl Driver for FakeSink {
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let mut hub_receiver = hub_sender.subscribe();

        while let Ok(message) = hub_receiver.recv().await {
            self.stats.write().await.stats.update_input(&message);

            for future in self.on_message_input.call_all(message.clone()) {
                if let Err(error) = future.await {
                    debug!("Dropping message: on_message_input callback returned error: {error:?}");
                    continue;
                }
            }

            let mut bytes = mavlink::async_peek_reader::AsyncPeekReader::new(message.raw_bytes());
            let (header, message): (mavlink::MavHeader, mavlink::ardupilotmega::MavMessage) =
                mavlink::read_v2_msg_async(&mut bytes).await.unwrap();
            if self.print {
                println!("Message received: {header:?} {message:?}");
            } else {
                trace!("Message received: {header:?} {message:?}");
            }
        }

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        Box::new(FakeSinkInfo)
    }

    fn name(&self) -> Arc<String> {
        self.name.load_full()
    }

    fn uuid(&self) -> &DriverUuid {
        &self.uuid
    }
}

#[async_trait::async_trait]
impl AccumulatedDriverStatsProvider for FakeSink {
    async fn stats(&self) -> AccumulatedDriverStats {
        self.stats.read().await.clone()
    }

    async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        stats.stats.input = None;
        stats.stats.output = None
    }
}

pub struct FakeSinkInfo;
impl DriverInfo for FakeSinkInfo {
    fn name(&self) -> &'static str {
        "FakeSink"
    }

    fn valid_schemes(&self) -> &'static [&'static str] {
        &["fakeclient", "fakesink", "fakec"]
    }

    fn cli_example_legacy(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}:<MODE>"),
            format!("{first_schema}:debug"),
        ]
    }

    fn cli_example_url(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}://<MODE>").to_string(),
            url::Url::parse(&format!("{first_schema}://debug"))
                .unwrap()
                .to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, _url: &url::Url) -> Option<Arc<dyn Driver>> {
        Some(Arc::new(FakeSink::builder("Unnamed").print().build()))
    }
}

pub struct FakeSource {
    name: arc_swap::ArcSwap<String>,
    uuid: DriverUuid,
    period: std::time::Duration,
    on_message_output: Callbacks<Arc<Protocol>>,
    stats: Arc<RwLock<AccumulatedDriverStats>>,
}

impl FakeSource {
    pub fn builder(name: &str, period: std::time::Duration) -> FakeSourceBuilder {
        let name = Arc::new(name.to_string());

        FakeSourceBuilder(Self {
            name: arc_swap::ArcSwap::new(name.clone()),
            uuid: Self::generate_uuid(&name),
            period,
            on_message_output: Callbacks::new(),
            stats: Arc::new(RwLock::new(AccumulatedDriverStats::new(
                name,
                &FakeSourceInfo,
            ))),
        })
    }
}

pub struct FakeSourceBuilder(FakeSource);

impl FakeSourceBuilder {
    pub fn build(self) -> FakeSource {
        self.0
    }
    pub fn on_message_output<C>(self, callback: C) -> Self
    where
        C: MessageCallback<Arc<Protocol>>,
    {
        self.0.on_message_output.add_callback(callback.into_boxed());
        self
    }
}

#[async_trait::async_trait]
impl Driver for FakeSource {
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let mut sequence = 0;

        let mut buf: Vec<u8> = Vec::with_capacity(280);

        use mavlink::ardupilotmega::{
            MavAutopilot, MavMessage, MavModeFlag, MavState, MavType, HEARTBEAT_DATA,
        };

        loop {
            let header = mavlink::MavHeader {
                sequence,
                system_id: 1,
                component_id: 2,
            };
            let data = MavMessage::HEARTBEAT(HEARTBEAT_DATA {
                custom_mode: 5,
                mavtype: MavType::MAV_TYPE_QUADROTOR,
                autopilot: MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
                base_mode: MavModeFlag::MAV_MODE_FLAG_MANUAL_INPUT_ENABLED
                    | MavModeFlag::MAV_MODE_FLAG_STABILIZE_ENABLED
                    | MavModeFlag::MAV_MODE_FLAG_GUIDED_ENABLED
                    | MavModeFlag::MAV_MODE_FLAG_CUSTOM_MODE_ENABLED,
                system_status: MavState::MAV_STATE_STANDBY,
                mavlink_version: 3,
            });
            sequence = sequence.overflowing_add(1).0;

            buf.clear();
            mavlink::write_v2_msg(&mut buf, header, &data).expect("Failed to write message");

            read_all_messages("FakeSource", &mut buf, {
                let hub_sender = hub_sender.clone();
                move |message| {
                    let message = Arc::new(message);
                    let hub_sender = hub_sender.clone();

                    async move {
                        trace!("Fake message created: {message:?}");

                        self.stats
                            .write()
                            .await
                            .stats
                            .update_output(&message);

                        for future in self.on_message_output.call_all(message.clone()) {
                            if let Err(error) = future.await {
                                debug!(
                                    "Dropping message: on_message_input callback returned error: {error:?}"
                                );
                                continue;
                            }
                        }

                        if let Err(error) = hub_sender.send(message) {
                            error!("Failed to send message to hub: {error:?}");
                        }
                    }
            }})
            .await;

            tokio::time::sleep(self.period).await;
        }
    }

    fn info(&self) -> Box<dyn DriverInfo> {
        Box::new(FakeSourceInfo)
    }

    fn name(&self) -> Arc<String> {
        self.name.load_full()
    }

    fn uuid(&self) -> &DriverUuid {
        &self.uuid
    }
}

#[async_trait::async_trait]
impl AccumulatedDriverStatsProvider for FakeSource {
    async fn stats(&self) -> AccumulatedDriverStats {
        self.stats.read().await.clone()
    }

    async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        stats.stats.input = None;
        stats.stats.output = None
    }
}

pub struct FakeSourceInfo;
impl DriverInfo for FakeSourceInfo {
    fn name(&self) -> &'static str {
        "FakeSource"
    }

    fn valid_schemes(&self) -> &'static [&'static str] {
        &["fakesource", "fakeserver", "fakesrc", "fakes"]
    }

    fn cli_example_legacy(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}:<MODE>"),
            format!("{first_schema}:heartbeat"),
        ]
    }

    fn cli_example_url(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}://<MODE>?period=<MILLISECONDS?>").to_string(),
            url::Url::parse(&format!("{first_schema}://heartbeat?period=10"))
                .unwrap()
                .to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, _url: &url::Url) -> Option<Arc<dyn Driver>> {
        Some(Arc::new(FakeSink::builder("Unnamed").print().build()))
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use anyhow::Result;
    use tokio::sync::{broadcast, RwLock};

    use super::*;

    #[tokio::test]
    async fn loopback_test() -> Result<()> {
        let (hub_sender, _) = broadcast::channel(10000);

        let number_of_messages = 800;
        let message_period = tokio::time::Duration::from_micros(1);
        let timeout_time = tokio::time::Duration::from_secs(1);

        let source_messages = Arc::new(RwLock::new(Vec::<Arc<Protocol>>::with_capacity(1000)));
        let sink_messages = Arc::new(RwLock::new(Vec::<Arc<Protocol>>::with_capacity(1000)));

        // FakeSink and task
        let sink = FakeSink::builder("test")
            .on_message_input({
                let sink_messages = sink_messages.clone();

                move |message: Arc<Protocol>| {
                    let sink_messages = sink_messages.clone();

                    async move {
                        sink_messages.write().await.push(message);
                        Ok(())
                    }
                }
            })
            .build();
        let sink_task = tokio::spawn({
            let hub_sender = hub_sender.clone();

            async move { sink.run(hub_sender).await }
        });

        // FakeSource and task
        let source = FakeSource::builder("test", message_period)
            .on_message_output({
                let source_messages = source_messages.clone();
                move |message: Arc<Protocol>| {
                    let source_messages = source_messages.clone();

                    async move {
                        source_messages.write().await.push(message);
                        Ok(())
                    }
                }
            })
            .build();
        let source_task = tokio::spawn({
            let hub_sender = hub_sender.clone();

            async move { source.run(hub_sender).await }
        });

        // Monitoring task to wait the
        let sink_monitor_task = tokio::spawn({
            let sink_messages = sink_messages.clone();
            async move {
                loop {
                    if sink_messages.read().await.len() >= number_of_messages {
                        break;
                    }

                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            }
        });
        let _ = tokio::time::timeout(timeout_time, sink_monitor_task)
            .await
            .expect(format!("sink messages: {:?}", sink_messages.read().await.len()).as_str());

        source_task.abort();
        sink_task.abort();

        // Compare the messages
        let source_messages = source_messages.read().await.clone();
        let sink_messages = sink_messages.read().await.clone();

        assert!(source_messages.len() >= number_of_messages);
        assert!(sink_messages.len() >= number_of_messages);
        assert_eq!(source_messages, sink_messages);

        Ok(())
    }
}