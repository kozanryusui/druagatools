use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::config::AdminToken;
use crate::logging::AdminHub;
use crate::online::OnlineState;
use crate::runtime_settings::{RuntimeSettings, SettingsError};
use aon_net_admin::contract::{
    AdminError, AdminLogin, BonusSettings, QuestSettings, RewardSettings, SettingsSnapshot,
    ShopUpdate,
};
use aon_net_admin::routes;

const INDEX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/index.html"));
const CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/admin.css"));
const JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin/aon-net-admin.js"));
const WASM_GZIP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/admin/aon-net-admin_bg.wasm.gz"));
const SESSION_COOKIE: &str = "__Host-aon-net-admin";
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const MAX_LOGIN_FAILURES: u8 = 5;
const MAX_LOGIN_SOURCES: usize = 4_096;

#[derive(Clone)]
struct AdminState {
    settings: Arc<RuntimeSettings>,
    online: Arc<OnlineState>,
    hub: Arc<AdminHub>,
    auth: Option<AdminAuth>,
}

#[derive(Clone)]
struct AdminAuth {
    inner: Arc<AdminAuthInner>,
}

struct AdminAuthInner {
    token: AdminToken,
    sessions: Mutex<HashSet<String>>,
    login_failures: Mutex<HashMap<IpAddr, FailedAttempts>>,
}

struct FailedAttempts {
    window_started: Instant,
    count: u8,
}

pub(crate) fn unsecured_router(
    settings: Arc<RuntimeSettings>,
    online: Arc<OnlineState>,
    hub: Arc<AdminHub>,
) -> Router {
    asset_routes().merge(api_routes()).with_state(AdminState {
        settings,
        online,
        hub,
        auth: None,
    })
}

pub(crate) fn secured_router(
    settings: Arc<RuntimeSettings>,
    online: Arc<OnlineState>,
    hub: Arc<AdminHub>,
    token: AdminToken,
) -> Router {
    let auth = AdminAuth::new(token);
    let protected = api_routes()
        .route(routes::LOGOUT, post(logout))
        .route("/admin/api/{*path}", any(not_found))
        .route_layer(middleware::from_fn_with_state(
            auth.clone(),
            require_authentication,
        ));
    asset_routes()
        .route(routes::LOGIN, post(login))
        .merge(protected)
        .with_state(AdminState {
            settings,
            online,
            hub,
            auth: Some(auth),
        })
        .layer(middleware::from_fn(security_headers))
}

fn asset_routes() -> Router<AdminState> {
    Router::new()
        .route(routes::INDEX, get(index))
        .route(routes::INDEX_SLASH, get(index))
        .route(routes::CSS, get(css))
        .route(routes::WASM_LOADER, get(js))
        .route(routes::WASM, get(wasm))
        .route(routes::FALLBACK, get(index))
}

fn api_routes() -> Router<AdminState> {
    Router::new()
        .route(routes::SNAPSHOT, get(snapshot))
        .route(routes::EVENTS, get(events))
        .route(routes::SHOP_SETTINGS, put(update_shop))
        .route(routes::QUEST_SETTINGS, put(update_quests))
        .route(routes::REWARD_SETTINGS, put(update_rewards))
        .route(routes::BONUS_SETTINGS, put(update_bonuses))
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
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn login(
    State(state): State<AdminState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(login): Json<AdminLogin>,
) -> Response {
    if !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
    let Some(auth) = state.auth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match auth.login(peer.ip(), &login.admin_token) {
        Ok(session) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            let cookie =
                format!("{SESSION_COOKIE}={session}; Path=/; Secure; HttpOnly; SameSite=Strict");
            match HeaderValue::from_str(&cookie) {
                Ok(cookie) => {
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                    response
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(LoginError::Rejected) => authentication_error(StatusCode::UNAUTHORIZED),
        Err(LoginError::Throttled) => authentication_error(StatusCode::TOO_MANY_REQUESTS),
        Err(LoginError::Internal) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    if !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
    if let (Some(auth), Some(session)) = (state.auth, session_cookie(&headers)) {
        auth.remove_session(session);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-aon-net-admin=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    response
}

async fn snapshot(State(state): State<AdminState>) -> Response {
    match state.settings.snapshot() {
        Ok(mut snapshot) => {
            let Ok(online_status) = state.online.status() else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            snapshot.online_status = online_status;
            let mut response = Json(snapshot).into_response();
            if state.auth.is_some() {
                response.headers_mut().insert(
                    "x-aon-net-admin-security",
                    HeaderValue::from_static("enabled"),
                );
            }
            response
        }
        Err(error) => settings_error(error),
    }
}

async fn update_shop(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(update): Json<ShopUpdate>,
) -> Response {
    if state.auth.is_some() && !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
    update_response(state.settings.update_shop(update.shop_name))
}

async fn update_quests(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(update): Json<QuestSettings>,
) -> Response {
    if state.auth.is_some() && !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
    update_response(state.settings.update_quests(update))
}

async fn update_rewards(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(update): Json<RewardSettings>,
) -> Response {
    if state.auth.is_some() && !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
    update_response(state.settings.update_rewards(update))
}

async fn update_bonuses(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(update): Json<BonusSettings>,
) -> Response {
    if state.auth.is_some() && !has_same_https_origin(&headers) {
        return authentication_error(StatusCode::FORBIDDEN);
    }
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

async fn require_authentication(
    State(auth): State<AdminAuth>,
    request: Request,
    next: Next,
) -> Response {
    if session_cookie(request.headers()).is_some_and(|session| auth.has_session(session)) {
        next.run(request).await
    } else {
        authentication_error(StatusCode::UNAUTHORIZED)
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn has_same_https_origin(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin
        .strip_prefix("https://")
        .is_some_and(|origin_host| origin_host.eq_ignore_ascii_case(host))
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| (name == SESSION_COOKIE).then_some(value))
}

fn authentication_error(status: StatusCode) -> Response {
    (
        status,
        Json(AdminError {
            message: "Authentication failed.".to_owned(),
            fields: Vec::new(),
        }),
    )
        .into_response()
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

enum LoginError {
    Rejected,
    Throttled,
    Internal,
}

impl AdminAuth {
    fn new(token: AdminToken) -> Self {
        Self {
            inner: Arc::new(AdminAuthInner {
                token,
                sessions: Mutex::new(HashSet::new()),
                login_failures: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn login(&self, source: IpAddr, token: &str) -> Result<String, LoginError> {
        let now = Instant::now();
        let mut failures = self
            .inner
            .login_failures
            .lock()
            .map_err(|_| LoginError::Internal)?;
        if failures.len() >= MAX_LOGIN_SOURCES {
            failures.retain(|_, entry| now.duration_since(entry.window_started) < LOGIN_WINDOW);
            if failures.len() >= MAX_LOGIN_SOURCES && !failures.contains_key(&source) {
                return Err(LoginError::Throttled);
            }
        }
        if failures.get(&source).is_some_and(|entry| {
            now.duration_since(entry.window_started) < LOGIN_WINDOW
                && entry.count >= MAX_LOGIN_FAILURES
        }) {
            return Err(LoginError::Throttled);
        }
        if !self.inner.token.matches(token) {
            let entry = failures.entry(source).or_insert(FailedAttempts {
                window_started: now,
                count: 0,
            });
            if now.duration_since(entry.window_started) >= LOGIN_WINDOW {
                entry.window_started = now;
                entry.count = 0;
            }
            entry.count = entry.count.saturating_add(1);
            return Err(LoginError::Rejected);
        }
        failures.remove(&source);
        drop(failures);

        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| LoginError::Internal)?;
        let session = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        self.inner
            .sessions
            .lock()
            .map_err(|_| LoginError::Internal)?
            .insert(session.clone());
        Ok(session)
    }

    fn has_session(&self, session: &str) -> bool {
        self.inner
            .sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains(session))
    }

    fn remove_session(&self, session: &str) {
        if let Ok(mut sessions) = self.inner.sessions.lock() {
            sessions.remove(session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "a-random-admin-token-with-32-bytes";

    #[test]
    fn successful_login_creates_a_revocable_session() {
        let auth = AdminAuth::new(AdminToken::new(TOKEN));
        let source = IpAddr::from([127, 0, 0, 1]);
        let session = auth
            .login(source, TOKEN)
            .unwrap_or_else(|_| panic!("the configured token must authenticate"));

        assert!(auth.has_session(&session));
        auth.remove_session(&session);
        assert!(!auth.has_session(&session));
    }

    #[test]
    fn repeated_login_failures_are_throttled_per_source() {
        let auth = AdminAuth::new(AdminToken::new(TOKEN));
        let source = IpAddr::from([192, 0, 2, 10]);
        for _ in 0..MAX_LOGIN_FAILURES {
            assert!(matches!(
                auth.login(source, "wrong token"),
                Err(LoginError::Rejected)
            ));
        }
        assert!(matches!(
            auth.login(source, TOKEN),
            Err(LoginError::Throttled)
        ));

        let other_source = IpAddr::from([192, 0, 2, 11]);
        assert!(auth.login(other_source, TOKEN).is_ok());
    }

    #[test]
    fn origin_and_session_cookie_checks_are_exact() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("ADMIN.EXAMPLE:443"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://admin.example:443"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; __Host-aon-net-admin=session-id"),
        );

        assert!(has_same_https_origin(&headers));
        assert_eq!(session_cookie(&headers), Some("session-id"));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://other.example"),
        );
        assert!(!has_same_https_origin(&headers));
    }
}
