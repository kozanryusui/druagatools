use serde::{Deserialize, Serialize};

use crate::item_database::Character;

#[derive(Debug, Deserialize, Serialize)]
pub struct ChestGuideDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub quests: Vec<ChestGuideQuest>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChestGuideQuest {
    pub id: u8,
    pub network_id: u16,
    pub name: String,
    pub category: QuestCategory,
    pub difficulty: u8,
    pub chests: Vec<ChestGuideEntry>,
    #[serde(default)]
    pub sol_areas: Vec<SolArea>,
    #[serde(default)]
    pub unmapped_sol_locations: u8,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SolArea {
    pub area_index: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<SolFloor>,
    pub stage: String,
    pub minimap: SolMinimap,
    pub locations: Vec<SolLocation>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SolFloor {
    Single(u8),
    Multiple(Vec<u8>),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SolMinimap {
    pub image: String,
    pub width: u16,
    pub height: u16,
    pub origin_x: i16,
    pub origin_z: i16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SolLocation {
    pub kind: SolKind,
    pub world_x: f32,
    pub world_z: f32,
    #[serde(default = "default_sol_radius")]
    pub radius: f32,
}

fn default_sol_radius() -> f32 {
    20.0
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SolKind {
    Sol,
    SilverSol,
    GoldSol,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestCategory {
    Original,
    Advanced,
    Special,
    Random,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChestGuideEntry {
    pub tier: ChestTier,
    pub player_action: String,
    pub variants: Vec<ChestVariant>,
    pub rewards: Vec<ChestReward>,
    pub illustrations: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChestTier {
    Blue,
    Red,
    Silver,
    Gold,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChestVariant {
    pub name: String,
    pub player_action: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChestReward {
    pub recipient: String,
    #[serde(flatten)]
    pub value: ChestRewardValue,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChestRewardValue {
    Item { item_id: u16, quantity: u8 },
    Gold { amount: u32 },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EnemyDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub enemies: Vec<EnemyRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EnemyRecord {
    pub definition_id: u16,
    pub name: String,
    pub base_drop_rate_percent: u8,
    pub item_selection_count: u8,
    pub total_item_weight: u16,
    pub drops: Vec<EnemyDrop>,
    pub quest_ids: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EnemyDrop {
    pub item_id: u16,
    pub weight: u8,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestSourceDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub treasure_pools: Vec<QuestTreasurePool>,
    #[serde(default)]
    pub direct_reward_pools: Vec<QuestDirectRewardPool>,
    pub quests: Vec<QuestSourceQuest>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestSourceQuest {
    #[serde(flatten)]
    pub identity: QuestSourceIdentity,
    pub name: String,
    pub rewards: Vec<ScriptedItemSource>,
    #[serde(default)]
    pub treasure_sources: Vec<QuestTreasureSource>,
    #[serde(default)]
    pub direct_reward_sources: Vec<QuestDirectRewardSource>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestSourceIdentity {
    Solo {
        chapter: u8,
        section: u8,
        character: Character,
    },
    Party {
        chapter: u8,
        section: u8,
    },
    Scheduled {
        guide_quest_id: u8,
        network_id: u16,
    },
    ScheduledPartyClear,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ScriptedItemSource {
    pub item_id: u16,
    #[serde(default)]
    pub required_item_ids: Vec<u16>,
    #[serde(default)]
    pub consumed_item_ids: Vec<u16>,
    pub acquisition: String,
    pub repeatability: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestTreasurePool {
    pub id: String,
    pub rewards: Vec<QuestPoolReward>,
    pub money: Option<QuestMoneyReward>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestPoolReward {
    pub item_id: u16,
    #[serde(default)]
    pub chance_numerator: Option<u16>,
    #[serde(default)]
    pub chance_denominator: Option<u16>,
    #[serde(default)]
    pub selection_condition: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestMoneyReward {
    pub minimum: u32,
    pub maximum: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestTreasureSource {
    pub pool_id: String,
    pub acquisition: String,
    pub repeatability: String,
    pub candidate_locations: Vec<[f32; 2]>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestDirectRewardPool {
    pub id: String,
    pub rewards: Vec<QuestPoolReward>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QuestDirectRewardSource {
    pub pool_id: String,
    pub acquisition: String,
    pub repeatability: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TowerSourceDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub sources: Vec<TowerItemSource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TowerItemSource {
    pub id: String,
    pub name: String,
    pub acquisition: String,
    pub repeatability: String,
    pub rewards: Vec<TowerItemReward>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TowerItemReward {
    pub item_id: u16,
    pub character: Option<Character>,
    pub chance_numerator: Option<u16>,
    pub chance_denominator: Option<u16>,
}

pub fn character_name(character: Character) -> &'static str {
    match character {
        Character::Gilgamesh => "Gilgamesh",
        Character::Valkyrie => "Valkyrie",
        Character::YoungKi => "Young Ki",
        Character::Xeovalga => "Xeovalga",
    }
}
