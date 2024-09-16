mod endpoints;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension,
    },
    response::IntoResponse,
    http::StatusCode, routing::get, Router
};
use tokio::sync::RwLock;
use std::sync::{Arc, Mutex};
use tracing::*;
use uuid::Uuid;
use futures::{sink::SinkExt, stream::StreamExt};

use lazy_static::lazy_static;

use std::collections::HashMap;

fn default_router() -> Router {
    Router::new()
        .route("/", get(endpoints::root))
        .route("/:path", get(endpoints::root))
        .route("/info", get(endpoints::info))
        .route("/mavlink/ws", get(websocket_clients))
        .fallback(get(|| async { (StatusCode::NOT_FOUND, "Not found :(") }))
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket_connection(socket, state))
}

async fn websocket_connection(socket: WebSocket, state: AppState) {
    let identifier = Uuid::new_v4();
    println!("Client connected with ID: {}", identifier);

    let (sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Save the sender in the clients map
    state.clients.write().await.insert(identifier, tx);

    // Spawn a task to forward messages from the channel to the websocket
    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                println!("Received from client {}: {}", identifier, text);
                broadcast_message(&state, identifier, Message::Text(text)).await;
            }
            Message::Close(_) => {
                println!("Client {} disconnected", identifier);
                break;
            }
            _ => {}
        }
    }

    // Remove client from the hashmap when disconnected
    state.clients.write().await.remove(&identifier);
    println!("Client {} removed", identifier);
}

async fn broadcast_message(state: &AppState, sender_identifier: Uuid, message: Message) {
    let mut clients = state.clients.write().await;

    eprintln!("Send to hub here!");

    for (&client_identifier, tx) in clients.iter_mut() {
        if client_identifier != sender_identifier {
            tx.send(message.clone());
        }
    }
}

lazy_static! {
    static ref SERVER: Arc<SingletonServer> = Arc::new(SingletonServer {
        router: Mutex::new(default_router()),
    });
}

struct SingletonServer {
    router: Mutex<Router>,
}

struct AppState {
    clients: Arc<RwLock<HashMap<Uuid, WebSocketSender>>>,
}

type WebSocketSender = futures::stream::SplitSink<WebSocket, Message>;

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
            if let Err(e) = axum::serve(listener, router.clone()).await {
                error!("WebServer error: {}", e);
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
