use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
// use tokio::sync::Mutex;
//use tower_http::cors::{CorsLayer, Origin};
use super::endpoints;

//use crate::endpoints;
//use crate::mavlink_vehicle::MAVLinkVehicleArcMutex;

use lazy_static::lazy_static;

#[derive(Debug, Serialize, Deserialize)]
struct ErrorResponse {
    error: String,
}

fn json_error_handler(err: serde_json::Error) -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: err.to_string(),
        }),
    )
}

/*
async fn root(Path(filename): Path<String>) -> impl IntoResponse {
    endpoints::root(filename).await
}
*/

/*
fn app(mavlink_vehicle: Arc<Mutex<MAVLinkVehicleArcMutex>>) -> Router {
    //let cors = CorsLayer::new().allow_origin(Origin::permissive());

    Router::new()
        .route("/info", get(endpoints::info))
        .route("/", get(root))
        .route("/helper/mavlink", get(endpoints::helper_mavlink))
        .route("/mavlink", get(endpoints::mavlink).post(endpoints::mavlink_post))
        .route("/mavlink/:path", get(endpoints::mavlink))
        .route("/ws/mavlink", get(endpoints::websocket))
        //.layer(cors)
        .layer(Extension(mavlink_vehicle))
        .fallback(get(|| async { (StatusCode::NOT_FOUND, "Not found") }))
}
*/

fn default_router() -> Router {
    //let cors = CorsLayer::new().allow_origin(Origin::permissive());
    Router::new()
        .route("/", get(endpoints::root))
        .route("/info", get(endpoints::info))
        //.route("/helper/mavlink", get(endpoints::helper_mavlink))
        //.route("/mavlink", get(endpoints::mavlink).post(endpoints::mavlink_post))
        //.route("/mavlink/:path", get(endpoints::mavlink))
        //.route("/ws/mavlink", get(endpoints::websocket))
        //.layer(cors)
        .fallback(get(|| async { (StatusCode::NOT_FOUND, "Not found") }))
}

// static SERVER: OnceCell<Arc<SingletonServer>> = OnceCell::const_new();

lazy_static! {
    static ref SERVER: Arc<SingletonServer> = Arc::new(SingletonServer {
        router: Mutex::new(default_router()),
    });
}

struct SingletonServer {
    router: Mutex<Router>,
}

pub fn start_server(addr: SocketAddr) {
    let router = SERVER.router.lock().unwrap().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
            if let Err(e) = axum::serve(listener, router.clone()).await {
                eprintln!("WebServer error: {}", e);
            }

            /*
            while let Err(e) = Server::bind(&addr)
                .serve(router.into_make_service())
                .await
            {
                eprintln!("WebServer error: {}", e);
            }
            */
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
