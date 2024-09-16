mod endpoints;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::Response,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, RwLock};
use tracing::*;
use uuid::Uuid;

use lazy_static::lazy_static;

use std::collections::HashMap;

fn default_router(clients: Arc<RwLock<HashMap<Uuid, ClientSender>>>) -> Router {
    Router::new()
        .route("/", get(endpoints::root))
        .route("/:path", get(endpoints::root))
        .route("/info", get(endpoints::info))
        .route("/mavlink/ws", get(websocket_handler))
        .fallback(get(|| async { (StatusCode::NOT_FOUND, "Not found :(") }))
        .with_state(AppState {
            clients,
        })
}

async fn websocket_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| async { websocket_connection(socket, state).await })
}
async fn websocket_connection(socket: WebSocket, state: AppState) {
    let identifier = Uuid::new_v4();
    debug!("WS client connected with ID: {identifier}");

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    state.clients.write().await.insert(identifier, tx);

    // Spawn a task to forward messages from the channel to the websocket
    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            println!("Sending..");
            send_message_to_all_clients(Message::Text("Oi".into())).await;
        }
    });

    // Handle incoming messages
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                trace!("WS client received from {identifier}: {text}");
                broadcast_message(&state, identifier, Message::Text(text)).await;
            }
            Message::Close(frame) => {
                debug!("WS client {identifier} disconnected: {frame:#?}");
                break;
            }
            _ => {}
        }
    }

    // We should be disconnected now, let's remove it
    state.clients.write().await.remove(&identifier);
    debug!("WS client {identifier} removed");
    send_task.await.unwrap();
}

async fn broadcast_message(state: &AppState, sender_identifier: Uuid, message: Message) {
    let mut clients = state.clients.write().await;

    eprintln!("Send to hub here!");

    for (&client_identifier, tx) in clients.iter_mut() {
        if client_identifier != sender_identifier {
            if let Err(error) = tx.send(message.clone()) {
                error!(
                    "Failed to send message to client {}: {:?}",
                    client_identifier, error
                );
            }
        }
    }
}

pub async fn send_message_to_all_clients(message: Message) {
    let state = SERVER.state.clone();
    let clients = state.clients.read().await;
    println!("Size: {}", clients.len());
    for (&client_identifier, tx) in clients.iter() {
        if let Err(error) = tx.send(message.clone()) {
            error!(
                "Failed to send message to client {}: {:?}",
                client_identifier, error
            );
        } else {
            debug!("Sent message to client {}", client_identifier);
        }
    }
}

lazy_static! {
    static ref SERVER: Arc<SingletonServer> = {
            let clients = Arc::new(RwLock::new(HashMap::new()));
            let router = Mutex::new(default_router(clients.clone()));
            Arc::new(SingletonServer {
                router,
                state: AppState {
                    clients,
                },
            })
    };
}

struct SingletonServer {
    router: Mutex<Router>,
    state: AppState,
}

#[derive(Clone)]
struct AppState {
    clients: Arc<RwLock<HashMap<Uuid, ClientSender>>>,
    message_tx: broadcast::Sender<String>,
}

type ClientSender = mpsc::UnboundedSender<Message>;

pub fn start_server(address: String) {
    let router = SERVER.router.lock().unwrap().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let listener = match tokio::net::TcpListener::bind(&address).await {
                Ok(listener) => listener,
                Err(e) => {
                    error!("WebServer TCP bind error: {}", e);
                    continue;
                }
            };
            if let Err(error) = axum::serve(listener, router.clone()).into_future().await {
                error!("WebServer error: {}", error);
            }
        }
    });
}

pub fn configure_router<F>(modifier: F)
where
    F: FnOnce(&mut Router),
{
    let mut router = SERVER.router.lock().unwrap();
    modifier(&mut router);
}
