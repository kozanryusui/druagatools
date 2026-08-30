use std::cell::RefCell;
use std::rc::Rc;

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{EventSource, HashChangeEvent, HtmlSelectElement, MessageEvent};

use super::contract::{
    AdminError, AdminEvent, AdminEventEnvelope, AdminSnapshot, BonusSettings, LogLevel, QuestMode,
    QuestOption, QuestSettings, RewardSettings, SettingsSnapshot, ShopUpdate,
};
use super::routes;

const LANTERN: u16 = 0x401e;
const HOLLOW_PROOF: u16 = 0x401d;
const MAX_BROWSER_LOGS: usize = 2_000;

#[derive(Clone, Eq, PartialEq)]
struct ConfigurationData {
    settings: SettingsSnapshot,
    party_quests: Vec<QuestOption>,
    special_quests: Vec<QuestOption>,
    timetable: Vec<super::contract::QuestTimetableEntry>,
}

#[component]
pub fn App() -> impl IntoView {
    let snapshot = RwSignal::new(None::<AdminSnapshot>);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let page = RwSignal::new(current_page());
    let paused_at = RwSignal::new(None::<u64>);
    let log_level = RwSignal::new("all".to_owned());
    let log_query = RwSignal::new(String::new());
    let configuration = Memo::new(move |_| {
        snapshot.get().map(|data| ConfigurationData {
            settings: data.settings,
            party_quests: data.party_quests,
            special_quests: data.special_quests,
            timetable: data.timetable,
        })
    });

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let listener =
            Closure::<dyn FnMut(HashChangeEvent)>::new(move |_| page.set(current_page()));
        window.set_onhashchange(Some(listener.as_ref().unchecked_ref()));
        listener.forget();
    });

    leptos::task::spawn_local(async move {
        match fetch_snapshot().await {
            Ok(value) => {
                snapshot.set(Some(value));
                connect_events(snapshot, error);
            }
            Err(message) => error.set(Some(message)),
        }
    });

    view! {
        <div class="shell">
            <aside class="sidebar">
                <div class="brand">"AON.Net"</div>
                <nav class="nav">
                    <a href="#configuration" class:active=move || page.get() == "configuration">"Configuration"</a>
                    <a href="#logs" class:active=move || page.get() == "logs">"Logs"</a>
                </nav>
            </aside>
            <main class="main">
                <div hidden=move || page.get() != "configuration">
                    <ConfigurationPage configuration snapshot error saving/>
                </div>
                <div hidden=move || page.get() != "logs">
                    <LogsPage snapshot paused_at log_level log_query/>
                </div>
            </main>
        </div>
    }
}

#[component]
fn ConfigurationPage(
    configuration: Memo<Option<ConfigurationData>>,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <h1>"Configuration"</h1>
        {move || configuration.get().map(|data| view! {
            <div class="grid two">
                <div class="stack">
                    <ShopPanel current=data.settings.shop_name snapshot error saving/>
                    <RewardsPanel current=data.settings.rewards.clone() bonuses=data.settings.bonuses.clone() snapshot error saving/>
                    <BonusesPanel current=data.settings.bonuses rewards=data.settings.rewards snapshot error saving/>
                </div>
                <div class="stack">
                    <QuestPanel current=data.settings.quests party=data.party_quests special=data.special_quests snapshot error saving/>
                    <TimetablePanel entries=data.timetable/>
                </div>
            </div>
        })}
    }
}

#[component]
fn ShopPanel(
    current: String,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) -> impl IntoView {
    let name = RwSignal::new(current);
    view! {
        <section class="panel">
            <h2>"Shop name"</h2>
            <div class="field"><label for="shop-name">"Name"</label><input id="shop-name" type="text" prop:value=move || name.get() on:input=move |event| name.set(event_target_value(&event))/></div>
            <SaveAction label="Save shop name" saving error on_save=move || save(routes::SHOP_SETTINGS, ShopUpdate { shop_name: name.get() }, snapshot, error, saving)/>
        </section>
    }
}

#[component]
fn RewardsPanel(
    current: RewardSettings,
    bonuses: BonusSettings,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) -> impl IntoView {
    let values = RwSignal::new(current);
    let rows = [
        ("100%", 0_u8),
        ("50%", 1),
        ("25%", 2),
        ("10%", 3),
        ("2%", 4),
    ];
    view! {
        <section class="panel">
            <h2>"Quest rewards"</h2>
            <table><thead><tr><th>"Chance"</th><th>"Reward item"</th></tr></thead><tbody>
                {rows.into_iter().map(|(label, index)| view! {
                    <tr><td>{label}</td><td><RewardSelect index values/></td></tr>
                }).collect_view()}
            </tbody></table>
            <SaveAction label="Save quest rewards" saving error on_save=move || {
                let rewards = values.get();
                if rewards.enabled_count() + bonuses.non_default_count() > 4 {
                    error.set(Some("Enable no more than four quest rewards and bonuses in total.".to_owned()));
                } else { save(routes::REWARD_SETTINGS, rewards, snapshot, error, saving); }
            }/>
        </section>
    }
}

#[component]
fn RewardSelect(index: u8, values: RwSignal<RewardSettings>) -> impl IntoView {
    let value =
        move || reward_at(&values.get(), index).map_or_else(String::new, |value| value.to_string());
    view! {
        <select prop:value=value on:change=move |event| {
            let selected = event_target_select_value(&event).parse::<u16>().ok();
            values.update(|rewards| set_reward_at(rewards, index, selected));
        }>
            <option value="">"None"</option>
            <option value=LANTERN.to_string()>"照魔のランタン"</option>
            <option value=HOLLOW_PROOF.to_string()>"ウツロの証"</option>
        </select>
    }
}

#[component]
fn BonusesPanel(
    current: BonusSettings,
    rewards: RewardSettings,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) -> impl IntoView {
    let values = RwSignal::new(current);
    view! {
        <section class="panel">
            <h2>"Quest bonuses"</h2>
            <div class="bonus-grid">
                <PercentField label="Experience" value=move || values.get().experience_percent on_value=move |value| values.update(|v| v.experience_percent = value)/>
                <PercentField label="Money" value=move || values.get().money_percent on_value=move |value| values.update(|v| v.money_percent = value)/>
                <PercentField label="Item drop rate" value=move || values.get().item_drop_percent on_value=move |value| values.update(|v| v.item_drop_percent = value)/>
            </div>
            <SaveAction label="Save quest bonuses" saving error on_save=move || {
                let bonuses = values.get();
                if rewards.enabled_count() + bonuses.non_default_count() > 4 {
                    error.set(Some("Enable no more than four quest rewards and bonuses in total.".to_owned()));
                } else { save(routes::BONUS_SETTINGS, bonuses, snapshot, error, saving); }
            }/>
        </section>
    }
}

#[component]
fn PercentField<F, G>(label: &'static str, value: F, on_value: G) -> impl IntoView
where
    F: Fn() -> u32 + Send + Sync + 'static,
    G: Fn(u32) + 'static,
{
    view! { <div class="field"><label>{label}</label><input type="number" min="0" prop:value=move || value().to_string() on:input=move |event| if let Ok(value) = event_target_value(&event).parse() { on_value(value) }/></div> }
}

#[component]
fn QuestPanel(
    current: QuestSettings,
    party: Vec<QuestOption>,
    special: Vec<QuestOption>,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) -> impl IntoView {
    let values = RwSignal::new(current);
    view! {
        <section class="panel">
            <h2>"Quest rotation"</h2>
            <div class="mode-grid">
                <div class="mode">
                    <label class="mode-choice"><input type="radio" name="quest-mode" prop:checked=move || values.get().mode == QuestMode::Random on:change=move |_| values.update(|v| v.mode = QuestMode::Random)/><strong>"Random rotation"</strong></label>
                    <NumberText label="Rotation interval in minutes" value=move || values.get().random_interval_minutes.to_string() on_value=move |text| if let Ok(value) = text.parse() { values.update(|v| v.random_interval_minutes = value) }/>
                </div>
                <div class="mode">
                    <label class="mode-choice"><input type="radio" name="quest-mode" prop:checked=move || values.get().mode == QuestMode::Fixed on:change=move |_| values.update(|v| v.mode = QuestMode::Fixed)/><strong>"Temporary fixed rotation"</strong></label>
                    <NumberText label="Duration in minutes" value=move || values.get().fixed_duration_minutes.to_string() on_value=move |text| if let Ok(value) = text.parse() { values.update(|v| v.fixed_duration_minutes = value) }/>
                </div>
            </div>
            <div class="rotation-fields">
                <QuestSelect label="Party quest A" options=party.clone() value=move || values.get().party_quests[0] on_value=move |id| values.update(|v| v.party_quests[0] = id)/>
                <QuestSelect label="Party quest B" options=party.clone() value=move || values.get().party_quests[1] on_value=move |id| values.update(|v| v.party_quests[1] = id)/>
                <QuestSelect label="Special quest" options=special value=move || values.get().special_quest on_value=move |id| values.update(|v| v.special_quest = id)/>
            </div>
            <div class="actions">
                <button disabled=move || saving.get() on:click=move |_| save(routes::QUEST_SETTINGS, values.get(), snapshot, error, saving)>"Save quest rotation"</button>
                <button class="secondary" disabled=move || saving.get() on:click=move |_| { values.update(|v| v.mode = QuestMode::Random); save(routes::QUEST_SETTINGS, values.get(), snapshot, error, saving); }>"Return to random"</button>
                <Status error/>
            </div>
        </section>
    }
}

#[component]
fn NumberText<F, G>(label: &'static str, value: F, on_value: G) -> impl IntoView
where
    F: Fn() -> String + Send + Sync + 'static,
    G: Fn(String) + 'static,
{
    view! { <div class="field"><label>{label}</label><input type="text" inputmode="numeric" prop:value=value on:input=move |event| on_value(event_target_value(&event))/></div> }
}

#[component]
fn QuestSelect<F, G>(
    label: &'static str,
    options: Vec<QuestOption>,
    value: F,
    on_value: G,
) -> impl IntoView
where
    F: Fn() -> u16 + Send + Sync + 'static,
    G: Fn(u16) + 'static,
{
    view! { <div class="field"><label>{label}</label><select prop:value=move || value().to_string() on:change=move |event| if let Ok(id) = event_target_select_value(&event).parse() { on_value(id) }>{options.into_iter().map(|option| view! { <option value=option.quest_id.to_string()>{option.name}</option> }).collect_view()}</select></div> }
}

#[component]
fn TimetablePanel(entries: Vec<super::contract::QuestTimetableEntry>) -> impl IntoView {
    view! { <section class="panel"><h2>"Next random quests"</h2><table><thead><tr><th>"Time"</th><th>"Party A"</th><th>"Party B"</th><th>"Special"</th></tr></thead><tbody>{entries.into_iter().map(|entry| view! { <tr><td>{entry.starts_at}</td><td>{entry.party_quests[0].clone()}</td><td>{entry.party_quests[1].clone()}</td><td>{entry.special_quest}</td></tr> }).collect_view()}</tbody></table></section> }
}

#[component]
fn SaveAction<F>(
    label: &'static str,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_save: F,
) -> impl IntoView
where
    F: Fn() + 'static,
{
    view! { <div class="actions"><button disabled=move || saving.get() on:click=move |_| on_save()>{label}</button><Status error/></div> }
}

#[component]
fn Status(error: RwSignal<Option<String>>) -> impl IntoView {
    view! { <span class="status" class:error=move || error.get().is_some()>{move || error.get().unwrap_or_default()}</span> }
}

#[component]
fn LogsPage(
    snapshot: RwSignal<Option<AdminSnapshot>>,
    paused_at: RwSignal<Option<u64>>,
    log_level: RwSignal<String>,
    log_query: RwSignal<String>,
) -> impl IntoView {
    view! {
        <h1>"Logs"</h1>
        <section class="panel">
            <div class="logs-toolbar">
                <select on:change=move |event| log_level.set(event_target_select_value(&event))><option value="all">"All"</option><option value="info">"Info"</option><option value="warning">"Warning"</option><option value="error">"Error"</option></select>
                <input type="text" placeholder="Filter logs" on:input=move |event| log_query.set(event_target_value(&event))/>
                <button class="secondary" on:click=move |_| {
                    if paused_at.get_untracked().is_some() {
                        paused_at.set(None);
                    } else {
                        let sequence = snapshot.get_untracked().as_ref().and_then(|state| state.logs.last()).map_or(0, |record| record.sequence);
                        paused_at.set(Some(sequence));
                    }
                }>{move || if paused_at.get().is_some() { "Resume" } else { "Pause" }}</button>
                <button class="secondary" on:click=move |_| snapshot.update(|snapshot| if let Some(snapshot) = snapshot { snapshot.logs.clear() })>"Clear"</button>
            </div>
            <div class="log-view">{move || {
                let level = log_level.get(); let query = log_query.get().to_lowercase(); let pause = paused_at.get();
                snapshot.get().map(|data| data.logs.into_iter().filter(|record| pause.is_none_or(|sequence| record.sequence <= sequence) && level_matches(record.level, &level) && (query.is_empty() || format!("{} {}", record.target, record.message).to_lowercase().contains(&query))).map(|record| {
                    let class = format!("level-{}", level_name(record.level).to_lowercase());
                    view! { <div class="log-line"><span>{record.timestamp}</span><span class=class>{level_name(record.level)}</span><span>{format!("{}  {}", record.target, record.message)}</span></div> }
                }).collect_view())
            }}</div>
        </section>
    }
}

fn save<T: Serialize + 'static>(
    route: &'static str,
    value: T,
    snapshot: RwSignal<Option<AdminSnapshot>>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
) {
    saving.set(true);
    error.set(None);
    leptos::task::spawn_local(async move {
        let result = async {
            let request = Request::put(route)
                .json(&value)
                .map_err(|e| e.to_string())?;
            let response = request.send().await.map_err(|e| e.to_string())?;
            if response.ok() {
                response
                    .json::<SettingsSnapshot>()
                    .await
                    .map_err(|e| e.to_string())
            } else {
                Err(response
                    .json::<AdminError>()
                    .await
                    .map(|e| e.message)
                    .unwrap_or_else(|_| {
                        format!("Request failed with status {}.", response.status())
                    }))
            }
        }
        .await;
        match result {
            Ok(settings) => snapshot.update(|state| {
                if let Some(state) = state {
                    state.settings = settings
                }
            }),
            Err(message) => error.set(Some(message)),
        }
        saving.set(false);
    });
}

async fn fetch_snapshot() -> Result<AdminSnapshot, String> {
    Request::get(routes::SNAPSHOT)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

fn connect_events(snapshot: RwSignal<Option<AdminSnapshot>>, error: RwSignal<Option<String>>) {
    let Ok(source) = EventSource::new(routes::EVENTS) else {
        error.set(Some("Cannot connect to the log stream.".to_owned()));
        return;
    };
    let source = Rc::new(RefCell::new(Some(source)));
    let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(text) = event.data().as_string() else {
            return;
        };
        let Ok(envelope) = serde_json::from_str::<AdminEventEnvelope>(&text) else {
            return;
        };
        let mut event_gap = false;
        snapshot.update(|state| {
            if let Some(state) = state {
                event_gap = envelope.sequence > state.sequence.saturating_add(1);
                state.sequence = envelope.sequence;
                match envelope.event {
                    AdminEvent::SettingsChanged(value) => state.settings = value,
                    AdminEvent::TimetableChanged(value) => state.timetable = value,
                    AdminEvent::Log(value) => {
                        if state.logs.len() == MAX_BROWSER_LOGS {
                            state.logs.remove(0);
                        }
                        state.logs.push(value);
                    }
                }
            }
        });
        if event_gap {
            leptos::task::spawn_local(async move {
                match fetch_snapshot().await {
                    Ok(value) => snapshot.set(Some(value)),
                    Err(message) => error.set(Some(message)),
                }
            });
        }
    });
    if let Some(event_source) = source.borrow().as_ref() {
        event_source.set_onmessage(Some(listener.as_ref().unchecked_ref()));
    }
    listener.forget();
    std::mem::forget(source);
}

fn current_page() -> String {
    web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .filter(|h| h == "#logs")
        .map_or_else(|| "configuration".to_owned(), |_| "logs".to_owned())
}
fn event_target_select_value(event: &leptos::ev::Event) -> String {
    event
        .target()
        .and_then(|t| t.dyn_into::<HtmlSelectElement>().ok())
        .map(|e| e.value())
        .unwrap_or_default()
}
fn reward_at(value: &RewardSettings, index: u8) -> Option<u16> {
    match index {
        0 => value.always,
        1 => value.half,
        2 => value.quarter,
        3 => value.ten_percent,
        _ => value.two_percent,
    }
}
fn set_reward_at(value: &mut RewardSettings, index: u8, item: Option<u16>) {
    match index {
        0 => value.always = item,
        1 => value.half = item,
        2 => value.quarter = item,
        3 => value.ten_percent = item,
        _ => value.two_percent = item,
    }
}
fn level_rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Debug => 0,
        LogLevel::Info => 1,
        LogLevel::Warning => 2,
        LogLevel::Error => 3,
    }
}
fn level_matches(record: LogLevel, filter: &str) -> bool {
    match filter {
        "info" => level_rank(record) >= 1,
        "warning" => level_rank(record) >= 2,
        "error" => level_rank(record) >= 3,
        _ => true,
    }
}
fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warning => "WARNING",
        LogLevel::Error => "ERROR",
    }
}
