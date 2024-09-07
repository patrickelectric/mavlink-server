use std::sync::Arc;

use anyhow::Result;
use tokio::{net::UdpSocket, sync::broadcast};
use tracing::*;

use crate::{
    drivers::{Driver, DriverInfo},
    protocol::{read_all_messages, Protocol},
};

pub struct UdpClient {
    pub remote_addr: String,
}

impl UdpClient {
    #[instrument(level = "debug")]
    pub fn new(remote_addr: &str) -> Self {
        Self {
            remote_addr: remote_addr.to_string(),
        }
    }

    #[instrument(level = "debug", skip(socket))]
    async fn udp_receive_task(
        socket: Arc<UdpSocket>,
        hub_sender: Arc<broadcast::Sender<Arc<Protocol>>>,
    ) -> Result<()> {
        let mut buf = Vec::with_capacity(1024);

        loop {
            match socket.recv_buf_from(&mut buf).await {
                Ok((bytes_received, client_addr)) if bytes_received > 0 => {
                    let client_addr = &client_addr.to_string();

                    read_all_messages(client_addr, &mut buf, |message| async {
                        if let Err(error) = hub_sender.send(Arc::new(message)) {
                            error!("Failed to send message to hub: {error:?}");
                        }
                    })
                    .await;
                }
                Ok((_, client_addr)) => {
                    warn!("UDP connection closed by {client_addr}.");
                    break;
                }
                Err(error) => {
                    error!("Failed to receive UDP message: {error:?}");
                    break;
                }
            }
        }

        debug!("UdpClient Receiver task finished");
        Ok(())
    }

    #[instrument(level = "debug", skip(socket))]
    async fn udp_send_task(
        socket: Arc<UdpSocket>,
        mut hub_receiver: broadcast::Receiver<Arc<Protocol>>,
    ) -> Result<()> {
        loop {
            match hub_receiver.recv().await {
                Ok(message) => {
                    if message.origin.eq(&socket.peer_addr()?.to_string()) {
                        continue; // Don't do loopback
                    }

                    match socket.send(message.raw_bytes()).await {
                        Ok(_) => {
                            // Message sent successfully
                        }
                        Err(ref error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                            // error!("UDP connection refused: {error:?}");
                            continue;
                        }
                        Err(error) => {
                            error!("Failed to send UDP message: {error:?}");
                            break;
                        }
                    }
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
impl Driver for UdpClient {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Arc<Protocol>>) -> Result<()> {
        let local_addr = "0.0.0.0:0";
        let remote_addr = self.remote_addr.clone();

        loop {
            let socket = match UdpSocket::bind(local_addr).await {
                Ok(socket) => Arc::new(socket),
                Err(error) => {
                    error!("Failed binding UdpClient to address {local_addr:?}: {error:?}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            debug!("UdpClient successfully bound to {local_addr}. Connecting UdpClient to {remote_addr:?}...");

            if let Err(error) = socket.connect(&remote_addr).await {
                error!("Failed connecting UdpClient to {remote_addr:?}: {error:?}");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                continue;
            };

            debug!("UdpClient successfully connected to {remote_addr:?}");

            let hub_sender = Arc::new(hub_sender.clone());
            let hub_receiver = hub_sender.subscribe();

            tokio::select! {
                result = UdpClient::udp_receive_task(socket.clone(), hub_sender) => {
                    if let Err(error) = result {
                        error!("Error in receiving UDP messages: {error:?}");
                    }
                }
                result = UdpClient::udp_send_task(socket, hub_receiver) => {
                    if let Err(error) = result {
                        error!("Error in sending UDP messages: {error:?}");
                    }
                }
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> Box<dyn DriverInfo> {
        return Box::new(UdpClientInfo);
    }
}

pub struct UdpClientInfo;
impl DriverInfo for UdpClientInfo {
    fn name(&self) -> &str {
        "UdpServer"
    }

    fn valid_schemes(&self) -> Vec<String> {
        vec![
            "udpc".to_string(),
            "udpclient".to_string(),
            "udpout".to_string(),
        ]
    }

    fn create_endpoint_from_url(&self, url: &url::Url) -> Option<Arc<dyn Driver>> {
        let host = url.host_str().unwrap();
        let port = url.port().unwrap();
        Some(Arc::new(UdpClient::new(&format!("{host}:{port}"))))
    }
}
