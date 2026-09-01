use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, put};
use axum::{Json, Router};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::logging::AdminHub;
use crate::runtime_settings::{RuntimeSettings, SettingsError};
use aon_net_admin::contract::{
    AdminError, BonusSettings, QuestSettings, RewardSettings, SettingsSnapshot, ShopUpdate,
};
use aon_net_admin::routes;

const INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/index.html"));
const CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/admin.css"));
const JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/aon-net-admin.js"));
const WASM_GZIP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/admin/aon-net-admin_bg.wasm.gz"));

#[derive(Clone)]
struct AdminState {
    settings: Arc<RuntimeSettings>,
    hub: Arc<AdminHub>,
}

pub(crate) fn router(settings: Arc<RuntimeSettings>, hub: Arc<AdminHub>) -> Router {
    Router::new()
        .route(routes::INDEX, get(index))
        .route(routes::INDEX_SLASH, get(index))
        .route(routes::SNAPSHOT, get(snapshot))
        .route(routes::EVENTS, get(events))
        .route(routes::SHOP_SETTINGS, put(update_shop))
        .route(routes::QUEST_SETTINGS, put(update_quests))
        .route(routes::REWARD_SETTINGS, put(update_rewards))
        .route(routes::BONUS_SETTINGS, put(update_bonuses))
        .route(routes::CSS, get(css))
        .route(routes::WASM_LOADER, get(js))
        .route(routes::WASM, get(wasm))
        .route(routes::FALLBACK, get(index))
        .with_state(AdminState { settings, hub })
}

async fn index() -> Response {
    asset(INDEX, "text/html; charset=utf-8", false)
}

async fn css() -> Response {
    asset(CSS, "text/css; charset=utf-8", false)
}

async fn js() -> Response {
    asset(JS, "text/javascript; charset=utf-8", false)
}

async fn wasm() -> Response {
    asset(WASM_GZIP, "application/wasm", true)
}

fn asset(bytes: &'static [u8], content_type: &'static str, gzip: bool) -> Response {
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    if gzip {
        response.headers_mut().insert(
            header::CONTENT_ENCODING,
            header::HeaderValue::from_static("gzip"),
        );
    }
    response
}

async fn snapshot(State(state): State<AdminState>) -> Response {
    match state.settings.snapshot() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => settings_error(error),
    }
}

async fn update_shop(State(state): State<AdminState>, Json(update): Json<ShopUpdate>) -> Response {
    update_response(state.settings.update_shop(update.shop_name))
}

async fn update_quests(
    State(state): State<AdminState>,
    Json(update): Json<QuestSettings>,
) -> Response {
    update_response(state.settings.update_quests(update))
}

async fn update_rewards(
    State(state): State<AdminState>,
    Json(update): Json<RewardSettings>,
) -> Response {
    update_response(state.settings.update_rewards(update))
}

async fn update_bonuses(
    State(state): State<AdminState>,
    Json(update): Json<BonusSettings>,
) -> Response {
    update_response(state.settings.update_bonuses(update))
}

fn update_response(result: Result<SettingsSnapshot, SettingsError>) -> Response {
    match result {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => settings_error(error),
    }
}

fn settings_error(error: SettingsError) -> Response {
    let status = if error.field_error().is_some() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let fields = error.field_error().into_iter().collect();
    (
        status,
        Json(AdminError {
            message: error.to_string(),
            fields,
        }),
    )
        .into_response()
}

async fn events(
    State(state): State<AdminState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.hub.subscribe()).filter_map(|message| {
        let envelope = message.ok()?;
        let data = serde_json::to_string(&envelope).ok()?;
        Some(Ok(Event::default().data(data)))
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
