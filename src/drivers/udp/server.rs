use anyhow::Result;
use mavlink::ardupilotmega::MavMessage;
use std::{collections::HashMap, sync::Arc};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, RwLock};
use tracing::*;

use crate::drivers::{Driver, DriverInfo};
use crate::protocol::Protocol;

pub struct UdpServer {
    pub local_addr: String,
    clients: Arc<RwLock<HashMap<(u8, u8), String>>>,
}

impl UdpServer {
    #[instrument(level = "debug")]
    pub fn new(local_addr: &str) -> Self {
        Self {
            local_addr: local_addr.to_string(),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[instrument(level = "debug", skip(socket, hub_sender, clients))]
    async fn udp_receive_task(
        socket: Arc<UdpSocket>,
        hub_sender: Arc<broadcast::Sender<Protocol>>,
        clients: Arc<RwLock<HashMap<(u8, u8), String>>>,
    ) -> Result<()> {
        let mut buf = Vec::with_capacity(1024);

        loop {
            buf.clear();

            match socket.recv_buf_from(&mut buf).await {
                Ok((bytes_received, client_addr)) if bytes_received > 0 => {
                    let client_addr = client_addr.to_string();

                    let message = match mavlink::read_v2_raw_message_async::<MavMessage, _>(
                        &mut (&buf[..bytes_received]),
                    )
                    .await
                    {
                        Ok(message) => message,
                        Err(error) => {
                            error!("Failed to parse MAVLink message: {error:?}");
                            continue; // Skip this iteration on error
                        }
                    };

                    let message = Protocol::new(&client_addr, message);

                    // Update clients
                    let header_buf = message.header();
                    let sysid = header_buf[4];
                    let compid = header_buf[5];
                    if clients
                        .write()
                        .await
                        .insert((sysid, compid), client_addr.clone())
                        .is_none()
                    {
                        debug!("Client added: ({sysid},{compid}) -> {client_addr:?}");
                    }

                    trace!("Received UDP message: {message:?}");
                    if let Err(error) = hub_sender.send(message) {
                        error!("Failed to send message to hub: {error:?}");
                    }
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

        debug!("UdpServer Receiver task finished");
        Ok(())
    }

    #[instrument(level = "debug", skip(socket, hub_receiver, clients))]
    async fn udp_send_task(
        socket: Arc<UdpSocket>,
        mut hub_receiver: broadcast::Receiver<Protocol>,
        clients: Arc<RwLock<HashMap<(u8, u8), String>>>,
    ) -> Result<()> {
        loop {
            match hub_receiver.recv().await {
                Ok(message) => {
                    for ((_, _), client_addr) in clients.read().await.iter() {
                        if message.origin.eq(client_addr) {
                            continue; // Don't do loopback
                        }

                        match socket.send(message.raw_bytes()).await {
                            Ok(_) => {
                                // Message sent successfully
                            }
                            Err(ref error)
                                if error.kind() == std::io::ErrorKind::ConnectionRefused =>
                            {
                                // error!("UDP connection refused: {error:?}");
                                continue;
                            }
                            Err(error) => {
                                error!("Failed to send UDP message: {error:?}");
                                break;
                            }
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
impl Driver for UdpServer {
    #[instrument(level = "debug", skip(self, hub_sender))]
    async fn run(&self, hub_sender: broadcast::Sender<Protocol>) -> Result<()> {
        let local_addr = &self.local_addr;
        let clients = self.clients.clone();

        loop {
            let socket = match UdpSocket::bind(local_addr).await {
                Ok(socket) => Arc::new(socket),
                Err(error) => {
                    error!("Failed binding UdpServer to address {local_addr:?}: {error:?}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            let hub_sender = Arc::new(hub_sender.clone());
            let hub_receiver = hub_sender.subscribe();

            tokio::select! {
                result = UdpServer::udp_receive_task(socket.clone(), hub_sender, clients.clone()) => {
                    if let Err(error) = result {
                        error!("Error in receiving UDP messages: {error:?}");
                    }
                }
                result = UdpServer::udp_send_task(socket, hub_receiver, clients.clone()) => {
                    if let Err(error) = result {
                        error!("Error in sending UDP messages: {error:?}");
                    }
                }
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    fn info(&self) -> DriverInfo {
        DriverInfo {
            name: "UdpServer".to_string(),
        }
    }
}
