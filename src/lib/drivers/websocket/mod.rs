use std::sync::Arc;

use anyhow::Result;
use axum::{Router, extract::WebSocketUpgrade, routing::get};
use axum::{
    extract::ws::{self, WebSocket},
    response::Response,
};
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use mavlink_codec::{Packet, codec::MavlinkCodec, error::DecoderError};
use std::io;
use std::net::SocketAddr;
use tokio::sync::{RwLock, broadcast};
use tokio_util::codec::{Decoder, Encoder};
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::*;
use uuid::Uuid;

use crate::{
    callbacks::{Callbacks, MessageCallback},
    drivers::{Driver, DriverInfo, generic_tasks::SendReceiveContext},
    protocol::Protocol,
    stats::{
        accumulated::driver::{AccumulatedDriverStats, AccumulatedDriverStatsProvider},
        driver::DriverUuid,
    },
};

pub struct WebSocketMavlinkCodec {
    inner: MavlinkCodec<true, true, false, false, false, false>,
}

impl WebSocketMavlinkCodec {
    pub fn new() -> Self {
        Self {
            inner: MavlinkCodec::default(),
        }
    }
}

impl Decoder for WebSocketMavlinkCodec {
    type Item = Packet;
    type Error = DecoderError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.inner.decode(src) {
            Ok(Some(Ok(packet))) => Ok(Some(packet)),
            Ok(Some(Err(e))) => Err(e),
            Ok(None) => Ok(None),
            Err(e) => Err(DecoderError::Io(e)),
        }
    }
}

impl Encoder<Packet> for WebSocketMavlinkCodec {
    type Error = io::Error;

    fn encode(&mut self, item: Packet, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.inner
            .encode(item, dst)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }
}

pub struct WebSocketMavlinkAdapter {
    codec: WebSocketMavlinkCodec,
}

impl WebSocketMavlinkAdapter {
    pub fn new() -> Self {
        Self {
            codec: WebSocketMavlinkCodec::new(),
        }
    }

    pub async fn handle_websocket_connection(
        &mut self,
        socket: WebSocket,
        context: &SendReceiveContext,
        identifier: &str,
    ) -> Result<()> {
        let (mut ws_sender, mut ws_receiver) = socket.split();

        let context_clone = context.clone();
        let identifier_clone = identifier.to_string();
        let mut codec_clone = self.codec.clone();

        let receive_task = tokio::spawn(async move {
            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(ws::Message::Binary(data)) => {
                        let mut bytes_mut = BytesMut::from(&data[..]);
                        while let Ok(Some(packet)) = codec_clone.decode(&mut bytes_mut) {
                            let message = Arc::new(Protocol::new(&identifier_clone, packet));

                            context_clone
                                .stats
                                .write()
                                .await
                                .stats
                                .update_input(&message);

                            for future in context_clone.on_message_input.call_all(message.clone()) {
                                if let Err(error) = future.await {
                                    debug!(
                                        "Dropping message: on_message_input callback returned error: {error:?}"
                                    );
                                    continue;
                                }
                            }

                            if let Err(send_error) = context_clone.hub_sender.send(message) {
                                error!("Failed to send message to hub: {send_error:?}");
                                continue;
                            }
                        }
                    }
                    Ok(ws::Message::Close(_)) => {
                        debug!("WebSocket connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {e:?}");
                        break;
                    }
                    _ => {}
                }
            }
        });

        let mut hub_receiver = context.hub_sender.subscribe();
        let mut codec_send = self.codec.clone();
        let context_clone = context.clone();
        let identifier_clone = identifier.to_string();
        let send_task = tokio::spawn(async move {
            loop {
                let message = match hub_receiver.recv().await {
                    Ok(message) => message,
                    Err(broadcast::error::RecvError::Closed) => {
                        error!("Hub channel closed!");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        warn!("Channel lagged by {count} messages.");
                        continue;
                    }
                };

                if message.origin.eq(&identifier_clone) {
                    continue; // Don't do loopback
                }

                context_clone
                    .stats
                    .write()
                    .await
                    .stats
                    .update_output(&message);

                for future in context_clone.on_message_output.call_all(message.clone()) {
                    if let Err(error) = future.await {
                        debug!(
                            "Dropping message: on_message_output callback returned error: {error:?}"
                        );
                        continue;
                    }
                }

                let mut bytes_mut = BytesMut::new();
                if let Err(error) = codec_send.encode((**message).clone(), &mut bytes_mut) {
                    error!("Failed to encode message: {error:?}");
                    continue;
                }

                if let Err(error) = ws_sender
                    .send(ws::Message::Binary(bytes_mut.freeze()))
                    .await
                {
                    error!("Failed to send WebSocket message: {error:?}");
                    break;
                }
            }
        });

        tokio::select! {
            _ = receive_task => {},
            _ = send_task => {},
        }

        Ok(())
    }
}

impl Clone for WebSocketMavlinkCodec {
    fn clone(&self) -> Self {
        Self {
            inner: MavlinkCodec::default(),
        }
    }
}

#[derive(Debug)]
pub struct WebSocketDriver {
    name: arc_swap::ArcSwap<String>,
    uuid: DriverUuid,
    pub bind_addr: String,
    on_message_input: Callbacks<Arc<Protocol>>,
    on_message_output: Callbacks<Arc<Protocol>>,
    stats: Arc<RwLock<AccumulatedDriverStats>>,
}

pub struct WebSocketDriverBuilder(WebSocketDriver);

impl WebSocketDriverBuilder {
    pub fn build(self) -> WebSocketDriver {
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

impl WebSocketDriver {
    #[instrument(level = "debug")]
    pub fn builder(name: &str, bind_addr: &str) -> WebSocketDriverBuilder {
        let name = Arc::new(name.to_string());

        WebSocketDriverBuilder(Self {
            name: arc_swap::ArcSwap::new(name.clone()),
            uuid: Self::generate_uuid(&format!("websocket:{}", bind_addr)),
            bind_addr: bind_addr.to_string(),
            on_message_input: Callbacks::default(),
            on_message_output: Callbacks::default(),
            stats: Arc::new(RwLock::new(AccumulatedDriverStats::new(
                name,
                &WebSocketInfo,
            ))),
        })
    }
}

#[async_trait::async_trait]
impl Driver for WebSocketDriver {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let bind_addr = self.bind_addr.clone();

        let context = SendReceiveContext {
            direction: crate::drivers::Direction::Both,
            hub_sender,
            on_message_output: self.on_message_output.clone(),
            on_message_input: self.on_message_input.clone(),
            stats: self.stats.clone(),
        };

        debug!("Starting WebSocket server on {bind_addr}");

        async fn websocket_handler(ws: WebSocketUpgrade, context: SendReceiveContext) -> Response {
            ws.on_upgrade(move |socket| {
                let identifier = Uuid::new_v4();
                debug!("WS client connected with ID: {identifier}");

                async move {
                    let mut adapter = WebSocketMavlinkAdapter::new();
                    if let Err(error) = adapter
                        .handle_websocket_connection(socket, &context, &identifier.to_string())
                        .await
                    {
                        warn!("WebSocket connection {identifier} closed: {error:?}");
                    }
                    debug!("WS client {identifier} removed");
                }
            })
        }

        let app = Router::new()
            .route(
                "/",
                get(move |ws: WebSocketUpgrade| websocket_handler(ws, context.clone())),
            )
            .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));

        let addr: SocketAddr = bind_addr.parse()?;
        info!("WebSocket server listening on {bind_addr}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        Box::new(WebSocketInfo)
    }

    fn name(&self) -> Arc<String> {
        self.name.load_full()
    }

    fn uuid(&self) -> &DriverUuid {
        &self.uuid
    }
}

#[async_trait::async_trait]
impl AccumulatedDriverStatsProvider for WebSocketDriver {
    async fn stats(&self) -> AccumulatedDriverStats {
        self.stats.read().await.clone()
    }

    async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        stats.stats.input = None;
        stats.stats.output = None
    }
}

pub struct WebSocketInfo;
impl DriverInfo for WebSocketInfo {
    fn name(&self) -> &'static str {
        "WebSocket"
    }
    fn valid_schemes(&self) -> &'static [&'static str] {
        &["ws", "wss"]
    }

    fn cli_example_legacy(&self) -> Vec<String> {
        let first_schema = self.valid_schemes()[0];

        vec![
            format!("{first_schema}:<HOST>:<PORT>"),
            format!("{first_schema}:0.0.0.0:9988"),
            format!("{first_schema}:192.168.1.100:9000"),
        ]
    }

    fn cli_example_url(&self) -> Vec<String> {
        let first_schema = &self.valid_schemes()[0];
        vec![
            format!("{first_schema}://<HOST>:<PORT>").to_string(),
            url::Url::parse(&format!("{first_schema}://0.0.0.0:9988"))
                .unwrap()
                .to_string(),
            url::Url::parse(&format!("{first_schema}://192.168.1.100:9000"))
                .unwrap()
                .to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, url: &url::Url) -> Option<Arc<dyn Driver>> {
        let host = url.host_str().unwrap_or("0.0.0.0");
        let port = url.port().unwrap_or(9988);
        let bind_addr = format!("{}:{}", host, port);

        Some(Arc::new(
            WebSocketDriver::builder("WebSocket", &bind_addr).build(),
        ))
    }
}
