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

fn default_router() -> Router {
    Router::new()
        .route("/", get(endpoints::root))
        .route("/:path", get(endpoints::root))
        .route("/info", get(endpoints::info))
        .route("/mavlink/ws", get(websocket_handler))
        .fallback(get(|| async { (StatusCode::NOT_FOUND, "Not found :(") }))
        .with_state(AppState {
            clients: Arc::new(RwLock::new(HashMap::new())),
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

lazy_static! {
    static ref SERVER: Arc<SingletonServer> = Arc::new(SingletonServer {
        router: Mutex::new(default_router()),
    });
}

struct SingletonServer {
    router: Mutex<Router>,
}

#[derive(Clone)]
struct AppState {
    clients: Arc<RwLock<HashMap<Uuid, ClientSender>>>,
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
