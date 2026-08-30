use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use druaga_utils::atomic_output;
use druaga_utils::item_database::ItemDatabase;
use druaga_utils::site_database::{
    ChestGuideDatabase, ChestGuideEntry, ChestGuideQuest, ChestReward, ChestRewardValue, ChestTier,
    ChestVariant, EnemyDatabase, EnemyDrop, EnemyRecord, QuestCategory,
};
use encoding_rs::SHIFT_JIS;
use serde::Deserialize;
use serde_json::Value;

struct Arguments {
    legacy_guide_database: PathBuf,
    legacy_guide_html: PathBuf,
    item_database: PathBuf,
    enemy_drop_csv: PathBuf,
    trigger_map: PathBuf,
    output_directory: PathBuf,
}

#[derive(Deserialize)]
struct TriggerMap {
    entity_catalog: Vec<EnemyCatalogEntry>,
    parties: Vec<PartyResources>,
}

#[derive(Deserialize)]
struct EnemyCatalogEntry {
    definition_id: u16,
    name: String,
}

#[derive(Deserialize)]
struct PartyResources {
    script: String,
    placement_resources: Vec<PlacementResource>,
}

#[derive(Deserialize)]
struct PlacementResource {
    groups: Vec<PlacementGroup>,
}

#[derive(Deserialize)]
struct PlacementGroup {
    entities: Vec<PlacedEnemy>,
}

#[derive(Deserialize)]
struct PlacedEnemy {
    definition_id: u16,
}

struct DropRecord {
    base_rate: u8,
    selections: u8,
    total_weight: u16,
    drops: Vec<EnemyDrop>,
}

type IllustrationMap = HashMap<(u8, ChestTier), Vec<String>>;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_args(env::args_os())?;
    let items: ItemDatabase = serde_json::from_slice(&fs::read(&arguments.item_database)?)?;
    let item_ids = item_ids_by_name(&items);
    let guide = build_guide_database(
        &arguments.legacy_guide_database,
        &arguments.legacy_guide_html,
        &item_ids,
    )?;
    let enemies =
        build_enemy_database(&arguments.enemy_drop_csv, &arguments.trigger_map, &item_ids)?;

    fs::create_dir_all(&arguments.output_directory)?;
    atomic_output::write_bytes(
        &arguments.output_directory.join("chests.json"),
        &serde_json::to_vec_pretty(&guide)?,
    )?;
    atomic_output::write_bytes(
        &arguments.output_directory.join("enemies.json"),
        &serde_json::to_vec_pretty(&enemies)?,
    )?;
    Ok(())
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Arguments, Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    let [
        _,
        legacy_guide_database,
        legacy_guide_html,
        item_database,
        enemy_drop_csv,
        trigger_map,
        output_directory,
    ] = args.as_slice()
    else {
        return Err("usage: site-database-builder GUIDE.JSON GUIDE.HTML ITEMS.JSON EMDROP.CSV TRIGGER-MAP.JSON OUTPUT-DIRECTORY".into());
    };
    Ok(Arguments {
        legacy_guide_database: legacy_guide_database.into(),
        legacy_guide_html: legacy_guide_html.into(),
        item_database: item_database.into(),
        enemy_drop_csv: enemy_drop_csv.into(),
        trigger_map: trigger_map.into(),
        output_directory: output_directory.into(),
    })
}

fn item_ids_by_name(database: &ItemDatabase) -> HashMap<&str, Vec<u16>> {
    let mut result = HashMap::<_, Vec<_>>::new();
    for item in &database.items {
        result.entry(item.name.as_str()).or_default().push(item.id);
    }
    result
}

fn resolve_item_id(name: &str, ids: &HashMap<&str, Vec<u16>>) -> Result<u16, Box<dyn Error>> {
    let candidates = ids
        .get(name)
        .ok_or_else(|| format!("item database has no item named {name}"))?;
    match candidates.as_slice() {
        [id] => Ok(*id),
        // The quest scripts use item 0x209b. Item 0x3000 has the same visible name.
        [0x209b, 0x3000] if name == "回復の小槌" => Ok(0x209b),
        _ => Err(format!("item name {name} is not unique: {candidates:?}").into()),
    }
}

fn build_guide_database(
    legacy_database: &Path,
    legacy_html: &Path,
    item_ids: &HashMap<&str, Vec<u16>>,
) -> Result<ChestGuideDatabase, Box<dyn Error>> {
    let source: Value = serde_json::from_slice(&fs::read(legacy_database)?)?;
    let illustrations = extract_illustrations(&fs::read_to_string(legacy_html)?)?;
    let mut quests = Vec::new();
    for quest in required_array(&source, "quests")? {
        let id = required_u64(quest, "script_index")?.try_into()?;
        let mut chests = Vec::new();
        for (tier_name, tier) in [
            ("Blue", ChestTier::Blue),
            ("Red", ChestTier::Red),
            ("Silver", ChestTier::Silver),
            ("Gold", ChestTier::Gold),
        ] {
            let Some(chest) = quest
                .get("chests")
                .and_then(|value| value.get(tier_name))
                .filter(|value| !value.is_null())
            else {
                continue;
            };
            let variants = required_array(chest, "variants")?
                .iter()
                .map(|variant| {
                    Ok(ChestVariant {
                        name: required_string(variant, "name")?.to_owned(),
                        player_action: required_string(variant, "rule")?.to_owned(),
                    })
                })
                .collect::<Result<_, Box<dyn Error>>>()?;
            let rewards = required_array(chest, "rewards")?
                .iter()
                .map(|reward| {
                    let item_name = required_string(reward, "item")?;
                    Ok(ChestReward {
                        recipient: required_string(reward, "character")?.to_owned(),
                        value: if item_name == "Gold" {
                            ChestRewardValue::Gold {
                                amount: required_u64(reward, "quantity")?.try_into()?,
                            }
                        } else {
                            ChestRewardValue::Item {
                                item_id: resolve_item_id(item_name, item_ids)?,
                                quantity: required_u64(reward, "quantity")?.try_into()?,
                            }
                        },
                    })
                })
                .collect::<Result<_, Box<dyn Error>>>()?;
            chests.push(ChestGuideEntry {
                tier,
                player_action: required_string(chest, "reconstruction")?.to_owned(),
                variants,
                rewards,
                illustrations: illustrations.get(&(id, tier)).cloned().unwrap_or_default(),
            });
        }
        quests.push(ChestGuideQuest {
            id,
            network_id: required_u64(quest, "network_id")?.try_into()?,
            name: required_string(quest, "name")?.to_owned(),
            category: parse_quest_category(required_string(quest, "category")?)?,
            difficulty: required_u64(quest, "difficulty")?.try_into()?,
            chests,
            sol_areas: Vec::new(),
            unmapped_sol_locations: 0,
        });
    }
    quests.sort_by_key(|quest| quest.id);
    Ok(ChestGuideDatabase {
        schema_version: 1,
        game_version: "1.60".to_owned(),
        quests,
    })
}

fn extract_illustrations(html: &str) -> Result<IllustrationMap, Box<dyn Error>> {
    let mut result = HashMap::<_, Vec<_>>::new();
    let quest_marker = "<section class=\"quest\" id=\"quest-";
    let article_marker = "<article class=\"chest ";
    let figure_marker = "<figure class=\"route-map";
    let mut quest_search = 0;
    while let Some(relative_start) = html[quest_search..].find(quest_marker) {
        let quest_start = quest_search + relative_start;
        let quest_id: u8 = attribute_number(&html[quest_start..], quest_marker)
            .ok_or("quest section has no numeric ID")?
            .try_into()?;
        let quest_end = html[quest_start + quest_marker.len()..]
            .find(quest_marker)
            .map_or(html.len(), |offset| {
                quest_start + quest_marker.len() + offset
            });
        let quest_html = &html[quest_start..quest_end];
        let mut article_search = 0;
        while let Some(relative_article) = quest_html[article_search..].find(article_marker) {
            let article_start = article_search + relative_article;
            let article_end = quest_html[article_start..]
                .find("</article>")
                .map(|offset| article_start + offset + 10)
                .ok_or("chest article has no closing tag")?;
            let article = &quest_html[article_start..article_end];
            let class = article[article_marker.len()..]
                .split('"')
                .next()
                .unwrap_or_default();
            let tier = match class {
                "blue" => ChestTier::Blue,
                "red" => ChestTier::Red,
                "silver" => ChestTier::Silver,
                "gold" => ChestTier::Gold,
                value => return Err(format!("unknown chest class {value}").into()),
            };
            let mut figure_search = 0;
            while let Some(relative_figure) = article[figure_search..].find(figure_marker) {
                let figure_start = figure_search + relative_figure;
                let figure_end = article[figure_start..]
                    .find("</figure>")
                    .map(|offset| figure_start + offset + 9)
                    .ok_or("map illustration has no closing tag")?;
                result
                    .entry((quest_id, tier))
                    .or_default()
                    .push(article[figure_start..figure_end].to_owned());
                figure_search = figure_end;
            }
            article_search = article_end;
        }
        quest_search = quest_end;
    }
    Ok(result)
}

fn attribute_number(line: &str, prefix: &str) -> Option<u64> {
    let value = line.split_once(prefix)?.1;
    value.split('"').next()?.parse().ok()
}

fn parse_quest_category(value: &str) -> Result<QuestCategory, Box<dyn Error>> {
    match value {
        "Original" => Ok(QuestCategory::Original),
        "Advanced" => Ok(QuestCategory::Advanced),
        "Special" => Ok(QuestCategory::Special),
        "Random" => Ok(QuestCategory::Random),
        _ => Err(format!("unknown quest category {value}").into()),
    }
}

fn build_enemy_database(
    drop_csv: &Path,
    trigger_map: &Path,
    item_ids: &HashMap<&str, Vec<u16>>,
) -> Result<EnemyDatabase, Box<dyn Error>> {
    let trigger_map: TriggerMap = serde_json::from_slice(&fs::read(trigger_map)?)?;
    let quest_ids = enemy_quest_ids(&trigger_map)?;
    let drop_records = parse_drop_records(drop_csv, item_ids)?;
    let catalog_ids: BTreeSet<_> = trigger_map
        .entity_catalog
        .iter()
        .map(|entry| entry.definition_id)
        .collect();
    if let Some(id) = drop_records.keys().find(|id| !catalog_ids.contains(id)) {
        return Err(format!("drop table refers to missing enemy definition 0x{id:04x}").into());
    }
    let mut enemies = Vec::with_capacity(trigger_map.entity_catalog.len());
    for catalog in trigger_map.entity_catalog {
        let drop = drop_records.get(&catalog.definition_id);
        enemies.push(EnemyRecord {
            definition_id: catalog.definition_id,
            name: catalog.name,
            base_drop_rate_percent: drop.map_or(0, |drop| drop.base_rate),
            item_selection_count: drop.map_or(0, |drop| drop.selections.min(4)),
            total_item_weight: drop.map_or(0, |drop| drop.total_weight),
            drops: drop.map_or_else(Vec::new, |drop| {
                drop.drops
                    .iter()
                    .map(|entry| EnemyDrop {
                        item_id: entry.item_id,
                        weight: entry.weight,
                    })
                    .collect()
            }),
            quest_ids: quest_ids
                .get(&catalog.definition_id)
                .map_or_else(Vec::new, |ids| ids.iter().copied().collect()),
        });
    }
    enemies.sort_by_key(|enemy| enemy.definition_id);
    Ok(EnemyDatabase {
        schema_version: 1,
        game_version: "1.60".to_owned(),
        enemies,
    })
}

fn enemy_quest_ids(map: &TriggerMap) -> Result<BTreeMap<u16, BTreeSet<u8>>, Box<dyn Error>> {
    let mut result = BTreeMap::<_, BTreeSet<_>>::new();
    for party in &map.parties {
        let id: u8 = party
            .script
            .strip_prefix("party")
            .and_then(|value| value.strip_suffix(".dat"))
            .ok_or_else(|| format!("invalid party script name {}", party.script))?
            .parse()?;
        for entity in party
            .placement_resources
            .iter()
            .flat_map(|resource| &resource.groups)
            .flat_map(|group| &group.entities)
        {
            result.entry(entity.definition_id).or_default().insert(id);
        }
    }
    Ok(result)
}

fn parse_drop_records(
    path: &Path,
    item_ids: &HashMap<&str, Vec<u16>>,
) -> Result<BTreeMap<u16, DropRecord>, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let (text, _, had_errors) = SHIFT_JIS.decode(&bytes);
    if had_errors {
        return Err(format!("{} is not valid CP932 text", path.display()).into());
    }
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut result = BTreeMap::new();
    let mut category = 0u16;
    let mut previous_family = None::<String>;
    for row in reader.records() {
        let row = row?;
        let family = row.get(0).unwrap_or_default();
        if previous_family
            .as_deref()
            .is_some_and(|previous| previous != family)
        {
            category += 1;
        }
        previous_family = Some(family.to_owned());
        let variant: u16 = row.get(1).unwrap_or_default().parse()?;
        let definition_id = category << 8 | variant;
        let base_rate = row.get(2).unwrap_or_default().parse()?;
        let selections = row.get(3).unwrap_or_default().parse()?;
        let mut drops = Vec::new();
        for column in (4..row.len()).step_by(2) {
            let name = row.get(column).unwrap_or_default();
            if name.is_empty() {
                break;
            }
            let parsed_weight = row
                .get(column + 1)
                .filter(|value| !value.is_empty())
                .unwrap_or("1")
                .parse::<u16>()?;
            // The Station parser stores the parsed integer in one byte.
            let weight = (parsed_weight as u8).max(1);
            drops.push(EnemyDrop {
                item_id: resolve_item_id(name, item_ids).map_err(|error| {
                    format!("enemy definition 0x{definition_id:04x}, CSV column {column}: {error}")
                })?,
                weight,
            });
        }
        let total_item_weight = drops.iter().map(|drop| u16::from(drop.weight)).sum();
        result.insert(
            definition_id,
            DropRecord {
                base_rate,
                selections,
                total_weight: total_item_weight,
                drops,
            },
        );
    }
    Ok(result)
}

fn required_array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>, Box<dyn Error>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("field {field} is not an array").into())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, Box<dyn Error>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field {field} is not a string").into())
}

fn required_u64(value: &Value, field: &str) -> Result<u64, Box<dyn Error>> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("field {field} is not an unsigned integer").into())
}
