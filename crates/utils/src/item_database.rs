use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ItemDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub items: Vec<ItemRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ItemRecord {
    pub id: u16,
    pub name: String,
    pub description: String,
    pub category: ItemCategory,
    pub catalog_order: u16,
    pub purchase_value: u32,
    pub sell_value: u32,
    pub alchemy_rank_points: u16,
    pub required_title_id: Option<u16>,
    pub disassembles_to_item_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_present_rate_percent: Option<u8>,
    pub icon: IconReference,
    pub equipment: Option<Equipment>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemCategory {
    Equipment,
    Consumable,
    Quest,
    MaterialOrTool,
    Accessory,
    Other,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IconReference {
    pub sheet: String,
    pub column: u8,
    pub row: u8,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Equipment {
    pub characters: Vec<Character>,
    pub slot: EquipmentSlot,
    pub required_rank: Rank,
    pub attack: Option<i16>,
    pub defense: Option<i16>,
    pub weight: Option<i16>,
    pub effects: Vec<ItemEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weapon_bonuses: Vec<WeaponBonus>,
}

/// Bonuses from the Tower item flag-label table, not the counted effect array.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponBonus {
    FireDamage,
    Freeze,
    LightningDamage,
    WindDamage,
    Sleep,
    Stun,
    Charm,
    ThreeWayShot,
    TwoShotBurst,
    RearShot,
}

impl WeaponBonus {
    pub const STATUS_NOTE: &str = "When equipped weapons have more than one status effect, only the highest-priority effect can activate. Priority: stun, sleep, freeze, then charm. If the enemy is immune, no other weapon status effect activates.";

    pub fn description(self, weapon_bonuses: &[Self]) -> &'static str {
        match self {
            Self::FireDamage => "Fire damage",
            Self::Freeze => "Chance to freeze enemies",
            Self::LightningDamage => "Lightning damage",
            Self::WindDamage => "Wind damage",
            Self::Sleep if weapon_bonuses.contains(&Self::Stun) => {
                "Sleep does not activate; this weapon's stun effect takes priority."
            }
            Self::Sleep => "Chance to put enemies to sleep",
            Self::Stun => "Chance to stun enemies",
            Self::Charm => "Chance to charm enemies. Multiplayer conditions are not yet confirmed.",
            Self::ThreeWayShot => "Normal attack: 3-way shot. Fires three arrows in a spread.",
            Self::TwoShotBurst => "Normal attack fires two arrows in sequence.",
            Self::RearShot => "Normal attack fires forward and backward.",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Character {
    Gilgamesh,
    Valkyrie,
    YoungKi,
    Xeovalga,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentSlot {
    Weapon,
    OffHand,
    Head,
    Body,
    Arms,
    Feet,
    Accessory,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Rank {
    pub value: u8,
    pub label: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemEffect {
    MaximumHp { amount: i16 },
    MaximumAp { amount: i16 },
    HpConvertedToAp { amount: i16 },
    ApConvertedToHp { amount: i16 },
    Strength { amount: i16 },
    Vitality { amount: i16 },
    Intelligence { amount: i16 },
    Spirit { amount: i16 },
    Dexterity { amount: i16 },
    Agility { amount: i16 },
    AttackPower { amount: i16 },
    PhysicalDefense { amount: i16 },
    MagicDefense { amount: i16 },
    Damage { amount: i16 },
    FinalDamagePercent { percent: i16 },
    RetaliationDamage { amount: i16 },
    PhysicalDamageReceivedPercent { percent: i16 },
    MagicDamageReceivedPercent { percent: i16 },
    MovementSpeedPercent { percent: i16 },
    AttackSpeedPercent { percent: i16 },
    CastingSpeedPercent { percent: i16 },
    AccuracyPercent { percent: i16 },
    EvasionPercent { percent: i16 },
    CriticalRatePercent { percent: i16 },
    Resistance { amount: i16 },
    EnemyFamilyAdvantage { family: String },
    EnemyFamilyConcealment { family: String },
    AbilityLevel { ability: String, levels: i16 },
    AbilityStrength { ability: String, strength: i16 },
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlchemyDatabase {
    pub schema_version: u32,
    pub game_version: String,
    pub recipes: Vec<AlchemyRecipe>,
    pub rule_based_recipes: Vec<RuleBasedAlchemyRecipe>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlchemyRecipe {
    pub id: u32,
    pub result_item_id: u16,
    pub ingredient_item_ids: Vec<u16>,
    pub completion_minutes: u16,
    pub success_rate_percent: u8,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RuleBasedAlchemyRecipe {
    pub id: u32,
    pub ingredient_category: AlchemyIngredientCategory,
    pub character: Option<Character>,
    pub point_level: u8,
    pub result_item_id: u16,
    pub next_result_item_id: Option<u16>,
    pub next_result_weight: u16,
    pub completion_minutes: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlchemyIngredientCategory {
    Other,
    MaterialOrTool,
    Accessory,
    Weapon,
    OffHand,
    Head,
    Body,
    Arms,
    Feet,
}
