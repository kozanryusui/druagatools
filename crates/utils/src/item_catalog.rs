use std::collections::HashMap;

use encoding_rs::SHIFT_JIS;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DECOMPOSITION_HAMMER_ID: u16 = 0x206e;

const HEADER_SIZE: usize = 0xa8;
const RECORD_SIZE: usize = 0x48;
const ALCHEMY_HEADER_SIZE: usize = 12;
const ALCHEMY_RECORD_SIZE: usize = 10;
const RULE_ALCHEMY_HEADER_SIZE: usize = 0x98;
const RULE_ALCHEMY_TABLE_COUNT: usize = 36;
const RULE_ALCHEMY_RECORD_COUNT: usize = 31;
const RULE_ALCHEMY_RECORD_SIZE: usize = 6;

#[derive(Debug, Error)]
pub enum Error {
    #[error("the item catalog is too short")]
    CatalogTooShort,
    #[error("item catalog version {0} is not supported")]
    UnsupportedCatalogVersion(u32),
    #[error("the item catalog record table is outside the file")]
    BadRecordTable,
    #[error("item 0x{item_id:04x} has an invalid {field} string offset 0x{offset:x}")]
    BadStringOffset {
        item_id: u16,
        field: &'static str,
        offset: usize,
    },
    #[error("item 0x{item_id:04x} has invalid Shift JIS in its {field}")]
    BadStringEncoding { item_id: u16, field: &'static str },
    #[error("item 0x{item_id:04x} has an invalid effect range")]
    BadEffectRange { item_id: u16 },
    #[error("the alchemy database is too short")]
    AlchemyTooShort,
    #[error("alchemy database version {0} is not supported")]
    UnsupportedAlchemyVersion(u16),
    #[error("the alchemy recipe table length is invalid")]
    BadAlchemyLength,
    #[error("the rule-based alchemy database is too short")]
    RuleAlchemyTooShort,
    #[error("the rule-based alchemy database has an invalid magic value")]
    BadRuleAlchemyMagic,
    #[error("rule-based alchemy database version {0} is not supported")]
    UnsupportedRuleAlchemyVersion(u32),
    #[error("a rule-based alchemy table is outside the file")]
    BadRuleAlchemyTable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Effect {
    pub id: u8,
    pub value: i16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AlchemyRecipe {
    pub result_item_id: u16,
    pub completion_minutes: u16,
    pub ingredient_item_ids: Vec<u16>,
}

#[derive(Clone, Copy, Debug)]
pub struct RuleBasedAlchemyRecipe {
    pub result_item_id: u16,
    pub next_result_weight: u16,
    pub completion_minutes: u16,
}

#[derive(Debug)]
pub struct RuleBasedAlchemyTable {
    pub category: u8,
    pub affinity: u8,
    pub recipes: Vec<RuleBasedAlchemyRecipe>,
}

#[derive(Debug)]
pub struct Item {
    pub id: u16,
    pub equip_slot: u8,
    pub character_mask: u8,
    pub flags: u32,
    pub rank_points: u16,
    pub required_title: u16,
    pub required_rank: u8,
    pub text_format: u8,
    pub list_order: u16,
    pub purchase_value: u32,
    pub sell_value: u32,
    pub icon_sheet: u16,
    pub icon_x: u8,
    pub icon_y: u8,
    pub name: String,
    pub description: String,
    pub auxiliary_text: String,
    pub effects: Vec<Effect>,
    pub weapon_motion: f32,
    pub attack_min: i16,
    pub attack_max_or_defense: i16,
    pub portable_duration: f32,
}

impl Item {
    pub fn resolved_icon_sheet(&self) -> usize {
        let sheet = usize::from(self.icon_sheet);
        if self.id & 0xff00 == 0x9f00 {
            // Accessory records include the first character-sheet base, but use shared artwork.
            return sheet.checked_sub(5).unwrap_or(sheet);
        }
        let character = usize::from((self.id >> 8) & 0x0f);
        let base = if self.id > 0x8fff && character < 4 {
            [5, 12, 19, 26][character]
        } else {
            0
        };
        base + sheet
    }

    pub fn is_equipment(&self) -> bool {
        (1..=6).contains(&self.equip_slot) && self.character_mask != 0
    }
}

pub fn parse_items(data: &[u8]) -> Result<Vec<Item>, Error> {
    if data.len() < HEADER_SIZE {
        return Err(Error::CatalogTooShort);
    }
    let version = u32_at(data, 0);
    if version != 100 {
        return Err(Error::UnsupportedCatalogVersion(version));
    }
    let count = usize::try_from(u32_at(data, 4)).map_err(|_| Error::BadRecordTable)?;
    let records_end = HEADER_SIZE
        .checked_add(
            count
                .checked_mul(RECORD_SIZE)
                .ok_or(Error::BadRecordTable)?,
        )
        .ok_or(Error::BadRecordTable)?;
    if records_end > data.len() {
        return Err(Error::BadRecordTable);
    }

    let mut items = Vec::with_capacity(count);
    for record in data[HEADER_SIZE..records_end].chunks_exact(RECORD_SIZE) {
        let id = u16_at(record, 0);
        if id == 0 {
            continue;
        }
        let effect_count = usize::from(record[0x28]);
        let effect_start = usize::from(record[0x29]);
        if effect_start
            .checked_add(effect_count)
            .is_none_or(|end| end > 6)
        {
            return Err(Error::BadEffectRange { item_id: id });
        }
        let effects = (effect_start..effect_start + effect_count)
            .map(|index| Effect {
                id: record[0x2a + index],
                value: i16_at(record, 0x30 + index * 2),
            })
            .collect();
        items.push(Item {
            id,
            equip_slot: record[2],
            character_mask: record[3],
            flags: u32_at(record, 4),
            rank_points: u16_at(record, 8),
            required_title: u16_at(record, 0x0a),
            required_rank: record[0x0c],
            text_format: record[0x0d],
            list_order: u16_at(record, 0x0e),
            purchase_value: u32_at(record, 0x10),
            sell_value: u32_at(record, 0x14),
            icon_sheet: u16_at(record, 0x18),
            icon_x: record[0x1a],
            icon_y: record[0x1b],
            name: string_at(data, id, "name", u32_at(record, 0x1c))?,
            description: string_at(data, id, "description", u32_at(record, 0x20))?,
            auxiliary_text: string_at(data, id, "auxiliary text", u32_at(record, 0x24))?,
            effects,
            weapon_motion: f32::from_bits(u32_at(record, 0x3c)),
            attack_min: i16_at(record, 0x40),
            attack_max_or_defense: i16_at(record, 0x42),
            portable_duration: f32::from_bits(u32_at(record, 0x44)),
        });
    }
    Ok(items)
}

pub fn parse_alchemy_recipes(data: &[u8]) -> Result<Vec<AlchemyRecipe>, Error> {
    if data.len() < ALCHEMY_HEADER_SIZE {
        return Err(Error::AlchemyTooShort);
    }
    let version = u16_at(data, 4);
    if version != 100 {
        return Err(Error::UnsupportedAlchemyVersion(version));
    }
    let count = usize::try_from(u32_at(data, 8)).map_err(|_| Error::BadAlchemyLength)?;
    let expected = ALCHEMY_HEADER_SIZE
        .checked_add(
            count
                .checked_mul(ALCHEMY_RECORD_SIZE)
                .ok_or(Error::BadAlchemyLength)?,
        )
        .ok_or(Error::BadAlchemyLength)?;
    if data.len() != expected {
        return Err(Error::BadAlchemyLength);
    }

    let mut recipes = Vec::with_capacity(count);
    for recipe in data[ALCHEMY_HEADER_SIZE..].chunks_exact(ALCHEMY_RECORD_SIZE) {
        recipes.push(AlchemyRecipe {
            result_item_id: u16_at(recipe, 0),
            completion_minutes: u16_at(recipe, 2),
            ingredient_item_ids: [u16_at(recipe, 4), u16_at(recipe, 6), u16_at(recipe, 8)]
                .into_iter()
                .filter(|ingredient| *ingredient != 0)
                .collect(),
        });
    }
    Ok(recipes)
}

pub fn parse_rule_based_alchemy(data: &[u8]) -> Result<Vec<RuleBasedAlchemyTable>, Error> {
    if data.len() < RULE_ALCHEMY_HEADER_SIZE {
        return Err(Error::RuleAlchemyTooShort);
    }
    if data.get(..4) != Some(b"ALC2") {
        return Err(Error::BadRuleAlchemyMagic);
    }
    let version = u32_at(data, 4);
    if version != 100 {
        return Err(Error::UnsupportedRuleAlchemyVersion(version));
    }

    let mut tables = Vec::with_capacity(RULE_ALCHEMY_TABLE_COUNT);
    for index in 0..RULE_ALCHEMY_TABLE_COUNT {
        let offset =
            usize::try_from(u32_at(data, 8 + index * 4)).map_err(|_| Error::BadRuleAlchemyTable)?;
        let size = RULE_ALCHEMY_RECORD_COUNT * RULE_ALCHEMY_RECORD_SIZE;
        let end = offset.checked_add(size).ok_or(Error::BadRuleAlchemyTable)?;
        let table = data.get(offset..end).ok_or(Error::BadRuleAlchemyTable)?;
        let recipes = table
            .chunks_exact(RULE_ALCHEMY_RECORD_SIZE)
            .map(|record| RuleBasedAlchemyRecipe {
                result_item_id: u16_at(record, 0),
                next_result_weight: u16_at(record, 2),
                completion_minutes: u16_at(record, 4),
            })
            .collect();
        tables.push(RuleBasedAlchemyTable {
            category: u8::try_from(index / 4).unwrap_or_default(),
            affinity: u8::try_from(index % 4).unwrap_or_default(),
            recipes,
        });
    }
    Ok(tables)
}

pub fn decomposition_results(recipes: &[AlchemyRecipe]) -> HashMap<u16, u16> {
    let mut results = HashMap::new();
    for recipe in recipes {
        if recipe
            .ingredient_item_ids
            .contains(&DECOMPOSITION_HAMMER_ID)
        {
            for ingredient in &recipe.ingredient_item_ids {
                if *ingredient != DECOMPOSITION_HAMMER_ID {
                    results.insert(*ingredient, recipe.result_item_id);
                }
            }
        }
    }
    results
}

fn string_at(data: &[u8], item_id: u16, field: &'static str, offset: u32) -> Result<String, Error> {
    let offset = usize::try_from(offset).map_err(|_| Error::BadStringOffset {
        item_id,
        field,
        offset: usize::MAX,
    })?;
    let bytes = data.get(offset..).ok_or(Error::BadStringOffset {
        item_id,
        field,
        offset,
    })?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(Error::BadStringOffset {
            item_id,
            field,
            offset,
        })?;
    let (value, _, malformed) = SHIFT_JIS.decode(&bytes[..end]);
    if malformed {
        return Err(Error::BadStringEncoding { item_id, field });
    }
    Ok(value.into_owned())
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn i16_at(data: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}
