use axum::{extract::Path, response::IntoResponse, Json};
// use hyper::StatusCode;
use axum::response::Html;
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
// use tokio_stream::StreamExt as _;

static HTML_DIST: Dir = include_dir!("src/web/html");

use super::data;
// use super::mavlink_vehicle::MAVLinkVehicleArcMutex;
// use super::websocket_manager::WebsocketActor;

// use log::*;

#[derive(Serialize, Debug, Default)]
pub struct InfoContent {
    /// Name of the program
    name: String,
    /// Version/tag
    version: String,
    /// Git SHA
    sha: String,
    build_date: String,
    /// Authors name
    authors: String,
}

#[derive(Serialize, Debug, Default)]
pub struct Info {
    /// Version of the REST API
    version: u32,
    /// Service information
    service: InfoContent,
}

#[derive(Deserialize)]
pub struct WebsocketQuery {
    /// Regex filter to selected the desired MAVLink messages by name
    filter: Option<String>,
}

#[derive(Deserialize)]
pub struct MAVLinkHelperQuery {
    /// MAVLink message name, possible options are here: https://docs.rs/mavlink/0.10.0/mavlink/#modules
    name: String,
}

pub async fn root(filename: Option<Path<String>>) -> impl IntoResponse {
    let filename = if filename.is_none() {
        "index.html".to_string()
    } else {
        filename.unwrap().0
    };

    HTML_DIST.get_file(filename).map_or(
        Html("File not found".to_string()).into_response(),
        |file| {
            let content = file.contents_utf8().unwrap_or("");
            Html(content.to_string()).into_response()
        },
    )
}

// #[api_v2_operation]
/// Provides information about the API and this program
pub async fn info() -> Json<Info> {
    let info = Info {
        version: 0,
        service: InfoContent {
            name: env!("CARGO_PKG_NAME").into(),
            version: "0.0.0".into(), //env!("VERGEN_GIT_SEMVER").into(),
            sha: env!("VERGEN_GIT_SHA").into(),
            build_date: env!("VERGEN_BUILD_TIMESTAMP").into(),
            authors: env!("CARGO_PKG_AUTHORS").into(),
        },
    };

    Json(info)
}

// #[api_v2_operation]
/// Provides an object containing all MAVLink messages received by the service
pub async fn mavlink(Path(path): Path<String>) -> Option<serde_json::Value> {
    data::messages().pointer_json(&path)
}

pub fn parse_query<T: serde::ser::Serialize>(message: &T) -> String {
    let error_message =
        "Not possible to parse mavlink message, please report this issue!".to_string();
    serde_json::to_string_pretty(&message).unwrap_or(error_message)
}

/*
#[api_v2_operation]
/// Returns a MAVLink message matching the given message name
pub async fn helper_mavlink(
    _req: HttpRequest,
    query: web::Query<MAVLinkHelperQuery>,
) -> actix_web::Result<HttpResponse> {
    let message_name = query.into_inner().name;

    let result = match mavlink::ardupilotmega::MavMessage::message_id_from_name(&message_name) {
        Ok(id) => mavlink::Message::default_message_from_id(id),
        Err(error) => Err(error),
    };

    match result {
        Ok(result) => {
            let msg = match result {
                mavlink::ardupilotmega::MavMessage::common(msg) => {
                    parse_query(&data::MAVLinkMessage {
                        header: mavlink::MavHeader::default(),
                        message: msg,
                    })
                }
                msg => parse_query(&data::MAVLinkMessage {
                    header: mavlink::MavHeader::default(),
                    message: msg,
                }),
            };

            ok_response(msg).await
        }
        Err(content) => not_found_response(parse_query(&content)).await,
    }
}

#[api_v2_operation]
#[allow(clippy::await_holding_lock)]
/// Send a MAVLink message for the desired vehicle
pub async fn mavlink_post(
    data: web::Data<MAVLinkVehicleArcMutex>,
    _req: HttpRequest,
    bytes: web::Bytes,
) -> actix_web::Result<HttpResponse> {
    let json_string = match String::from_utf8(bytes.to_vec()) {
        Ok(content) => content,
        Err(err) => {
            return not_found_response(format!("Failed to parse input as UTF-8 string: {err:?}"))
                .await;
        }
    };

    debug!("MAVLink post received: {json_string}");

    if let Ok(content) =
        json5::from_str::<data::MAVLinkMessage<mavlink::ardupilotmega::MavMessage>>(&json_string)
    {
        match data.lock().unwrap().send(&content.header, &content.message) {
            Ok(_result) => {
                data::update((content.header, content.message));
                return HttpResponse::Ok().await;
            }
            Err(err) => {
                return not_found_response(format!("Failed to send message: {err:?}")).await
            }
        }
    }

    if let Ok(content) =
        json5::from_str::<data::MAVLinkMessage<mavlink::common::MavMessage>>(&json_string)
    {
        let content_ardupilotmega = mavlink::ardupilotmega::MavMessage::common(content.message);
        match data
            .lock()
            .unwrap()
            .send(&content.header, &content_ardupilotmega)
        {
            Ok(_result) => {
                data::update((content.header, content_ardupilotmega));
                return HttpResponse::Ok().await;
            }
            Err(err) => {
                return not_found_response(format!("Failed to send message: {err:?}")).await;
            }
        }
    }

    not_found_response(String::from(
        "Failed to parse message, not a valid MAVLinkMessage.",
    ))
    .await
}

#[api_v2_operation]
/// Websocket used to receive and send MAVLink messages asynchronously
pub async fn websocket(
    req: HttpRequest,
    query: web::Query<WebsocketQuery>,
    stream: web::Payload,
) -> Result<HttpResponse, actix_web::Error> {
    let filter = match query.into_inner().filter {
        Some(filter) => filter,
        _ => ".*".to_owned(),
    };

    debug!("New websocket with filter {:#?}", &filter);

    ws::start(WebsocketActor::new(filter), &req, stream)
}

async fn not_found_response(message: String) -> actix_web::Result<HttpResponse> {
    HttpResponse::NotFound()
        .content_type("application/json")
        .body(message)
        .await
}

async fn ok_response(message: String) -> actix_web::Result<HttpResponse> {
    HttpResponse::Ok()
        .content_type("application/json")
        .body(message)
        .await
}
*/
