use std::sync::Arc;

use anyhow::Result;
use mavlink_server::callbacks::{Callbacks, MessageCallback};
use tokio::{net::TcpStream, sync::broadcast};
use tracing::*;

use crate::{
    drivers::{
        tcp::{tcp_receive_task, tcp_send_task},
        Driver, DriverInfo,
    },
    protocol::Protocol,
};

pub struct TcpClient {
    pub remote_addr: String,
    on_message: Callbacks<Arc<Protocol>>,
}

pub struct TcpClientBuilder(TcpClient);

impl TcpClientBuilder {
    pub fn build(self) -> TcpClient {
        self.0
    }

    pub fn on_message<C>(self, callback: C) -> Self
    where
        C: MessageCallback<Arc<Protocol>>,
    {
        self.0.on_message.add_callback(callback.into_boxed());
        self
    }
}

impl TcpClient {
    #[instrument(level = "debug")]
    pub fn builder(remote_addr: &str) -> TcpClientBuilder {
        TcpClientBuilder(Self {
            remote_addr: remote_addr.to_string(),
            on_message: Callbacks::new(),
        })
    }
}

#[async_trait::async_trait]
impl Driver for TcpClient {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let server_addr = &self.remote_addr;
        let hub_sender = Arc::new(hub_sender);

        loop {
            debug!("Trying to connect to {server_addr:?}...");
            let (read, write) = match TcpStream::connect(server_addr).await {
                Ok(socket) => socket.into_split(),
                Err(error) => {
                    error!("Failed connecting to {server_addr:?}: {error:?}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            debug!("TcpClient successfully connected to {server_addr:?}");

            let hub_receiver = hub_sender.subscribe();
            let hub_sender_cloned = Arc::clone(&hub_sender);

            tokio::select! {
                result = tcp_receive_task(read, server_addr, hub_sender_cloned, &self.on_message) => {
                    if let Err(e) = result {
                        error!("Error in TCP receive task: {e:?}");
                    }
                }
                result = tcp_send_task(write, server_addr, hub_receiver, &self.on_message) => {
                    if let Err(e) = result {
                        error!("Error in TCP send task: {e:?}");
                    }
                }
            }

            debug!("Restarting TCP Client connection loop...");
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        return Box::new(TcpClientInfo);
    }
}

pub struct TcpClientInfo;
impl DriverInfo for TcpClientInfo {
    fn name(&self) -> &str {
        "TcpClient"
    }

    fn valid_schemes(&self) -> Vec<String> {
        vec!["tcpc".to_string(), "tcpclient".to_string()]
    }

    fn create_endpoint_from_url(&self, url: &url::Url) -> Option<Arc<dyn Driver>> {
        let host = url.host_str().unwrap();
        let port = url.port().unwrap();
        Some(Arc::new(
            TcpClient::builder(&format!("{host}:{port}")).build(),
        ))
    }
}
