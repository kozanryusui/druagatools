use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use tracing::{debug, info, trace, warn};

use crate::config::PowerOnConfig;
use crate::protocol::power_on::{
    PowerOnRequest, PowerOnResponse, PowerOnTime, deserialize_power_on_request,
    serialize_power_on_response,
};
use crate::runtime_settings::RuntimeSettings;

#[derive(Clone)]
struct AppState {
    power_on: Arc<PowerOnConfig>,
    settings: Arc<RuntimeSettings>,
}

pub(super) fn router(config: PowerOnConfig, settings: Arc<RuntimeSettings>) -> Router {
    Router::new()
        .route("/sys/servlet/PowerOn", post(handle_power_on))
        .with_state(AppState {
            power_on: Arc::new(config),
            settings,
        })
}

async fn handle_power_on(State(state): State<AppState>, body: Bytes) -> Response {
    trace!(body = %String::from_utf8_lossy(&body), "received encoded PowerOn body");
    let request = match deserialize_power_on_request(&body) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "rejected PowerOn request");
            return (StatusCode::BAD_REQUEST, error.to_string()).into_response();
        }
    };

    let PowerOnRequest {
        game_id,
        game_version,
        serial,
        address,
        firmware_version,
        boot_version,
        encoding,
    } = request;
    info!(
        %game_version,
        %serial,
        %address,
        "accepted PowerOn request"
    );
    debug!(
        %game_id,
        firmware_major = firmware_version.major,
        firmware_minor = firmware_version.minor,
        boot_major = boot_version.major,
        boot_minor = boot_version.minor,
        ?encoding,
        "parsed PowerOn request details"
    );
    let shop_name = match state.settings.shop_name() {
        Ok(shop_name) => shop_name,
        Err(error) => {
            warn!(%error, "cannot load the shop name");
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let response = response_from_config(&state.power_on, shop_name);

    match serialize_power_on_response(&response) {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=us-ascii")],
            body,
        )
            .into_response(),
        Err(error) => {
            warn!(%error, "cannot encode PowerOn response");
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    }
}

fn response_from_config(config: &PowerOnConfig, shop_name: String) -> PowerOnResponse {
    let now = jiff::Zoned::now();
    PowerOnResponse {
        status: 0,
        uri: config.uri.clone(),
        host: config.host.clone(),
        shop_name,
        shop_nickname: config.shop_nickname.clone(),
        region_code: config.region_code.clone(),
        region_name_0: config.region_name_0.clone(),
        region_name_1: config.region_name_1.clone(),
        region_name_2: config.region_name_2.clone(),
        region_name_3: config.region_name_3.clone(),
        place_id: config.place_id.clone(),
        setting: String::new(),
        time: PowerOnTime {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
            second: now.second(),
        },
    }
}
