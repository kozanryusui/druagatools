use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use druaga_utils::atomic_output;
use druaga_utils::gsm2::Image;
use druaga_utils::item_catalog::{
    Effect, Item, decomposition_results, parse_alchemy_recipes, parse_items,
    parse_rule_based_alchemy,
};
use druaga_utils::item_database::{
    AlchemyDatabase, AlchemyIngredientCategory, AlchemyRecipe, Character, Equipment, EquipmentSlot,
    IconReference, ItemCategory, ItemDatabase, ItemEffect, ItemRecord, Rank,
    RuleBasedAlchemyRecipe,
};
use encoding_rs::SHIFT_JIS;
use zenravif::{Encoder, Img, RGBA8};

const ICON_PITCH: u8 = 34;
const ICON_QUALITY: f32 = 80.0;
const IMAGE_BASE: u32 = 0x0040_0000;
const RANK_NAME_TABLE: u32 = 0x004f_a938;
const MAGIC_EFFECT_TABLES: u32 = 0x004f_b93c;
const FORCE_EFFECT_TABLES: u32 = 0x004f_b94c;
const SPECIAL_EFFECT_TABLES: u32 = 0x004f_b95c;
const ENEMY_FAMILY_NAMES: u32 = 0x004f_c208;
const EFFECT_RECORD_SIZE: u32 = 0x1c;

const ICON_SHEETS: [&str; 28] = [
    "c04b0000", "c04b0001", "c04b0002", "c04b0003", "c04b0004", "c04b0100", "c04b0101", "c04b0102",
    "c04b0103", "c04b0104", "c04b0105", "c04b0200", "c04b0201", "c04b0202", "c04b0203", "c04b0204",
    "c04b0205", "c04b0300", "c04b0301", "c04b0302", "c04b0303", "c04b0304", "c04b0305", "c04b0400",
    "c04b0401", "c04b0402", "c04b0403", "c04b0404",
];

const PAGES: [(&str, &str); 9] = [
    ("items.html", "Item database"),
    ("items-gilgamesh.html", "Gilgamesh"),
    ("items-walkure.html", "Valkyrie"),
    ("items-young-ki.html", "Young Ki"),
    ("items-xeovalga.html", "Xeovalga"),
    ("items-accessories.html", "Accessories"),
    ("items-quest.html", "Quest items"),
    ("items-consumables.html", "Consumables"),
    ("items-other.html", "Other items"),
];

enum Command {
    Sources {
        item_catalog: PathBuf,
        alchemy_database: PathBuf,
        rule_alchemy_database: PathBuf,
        tower_executable: PathBuf,
        icon_directory: PathBuf,
        output_directory: PathBuf,
    },
    Database {
        item_database: PathBuf,
        alchemy_database: PathBuf,
        output_directory: PathBuf,
    },
}

struct TowerMetadata<'a> {
    executable: &'a [u8],
}

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(env::args_os())? {
        Command::Sources {
            item_catalog,
            alchemy_database,
            rule_alchemy_database,
            tower_executable,
            icon_directory,
            output_directory,
        } => build_from_sources(
            &item_catalog,
            &alchemy_database,
            &rule_alchemy_database,
            &tower_executable,
            &icon_directory,
            &output_directory,
        ),
        Command::Database {
            item_database,
            alchemy_database,
            output_directory,
        } => render_from_databases(&item_database, &alchemy_database, &output_directory),
    }
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Command, Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    match args.as_slice() {
        [_, mode, item_catalog, alchemy_database, rule_alchemy_database, tower_executable, icon_directory, output_directory]
            if mode == "sources" =>
        {
            Ok(Command::Sources {
                item_catalog: item_catalog.into(),
                alchemy_database: alchemy_database.into(),
                rule_alchemy_database: rule_alchemy_database.into(),
                tower_executable: tower_executable.into(),
                icon_directory: icon_directory.into(),
                output_directory: output_directory.into(),
            })
        }
        [_, mode, item_database, alchemy_database, output_directory] if mode == "database" => {
            Ok(Command::Database {
                item_database: item_database.into(),
                alchemy_database: alchemy_database.into(),
                output_directory: output_directory.into(),
            })
        }
        _ => Err("usage:\n  item-database-builder sources ITEM.DAT ALCHEMY.DAT ALCHEMY2.DAT TOWER.EXE ICON_DIRECTORY OUTPUT_DIRECTORY\n  item-database-builder database ITEMS.JSON ALCHEMY.JSON OUTPUT_DIRECTORY".into()),
    }
}

fn build_from_sources(
    item_catalog: &Path,
    alchemy_database: &Path,
    rule_alchemy_database: &Path,
    tower_executable: &Path,
    icon_directory: &Path,
    output_directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let item_data = fs::read(item_catalog)?;
    let alchemy_data = fs::read(alchemy_database)?;
    let rule_alchemy_data = fs::read(rule_alchemy_database)?;
    let tower_data = fs::read(tower_executable)?;
    let mut source_items = parse_items(&item_data)?;
    source_items.sort_by_key(|item| item.id);
    let source_recipes = parse_alchemy_recipes(&alchemy_data)?;
    let source_rule_tables = parse_rule_based_alchemy(&rule_alchemy_data)?;
    let metadata = TowerMetadata::new(&tower_data)?;
    let decompositions = decomposition_results(&source_recipes);

    let item_database = build_item_database(&source_items, &decompositions, &metadata)?;
    let alchemy_database = AlchemyDatabase {
        schema_version: 2,
        game_version: "1.60".to_owned(),
        recipes: source_recipes
            .into_iter()
            .enumerate()
            .map(|(index, recipe)| AlchemyRecipe {
                id: u32::try_from(index).unwrap_or(u32::MAX),
                result_item_id: recipe.result_item_id,
                ingredient_item_ids: recipe.ingredient_item_ids,
                completion_minutes: recipe.completion_minutes,
                success_rate_percent: 100,
            })
            .collect(),
        rule_based_recipes: build_rule_based_recipes(source_rule_tables)?,
    };

    fs::create_dir_all(output_directory)?;
    let data_output = output_directory.join("data");
    fs::create_dir_all(&data_output)?;
    let mut item_json = serde_json::to_vec_pretty(&item_database)?;
    item_json.push(b'\n');
    let mut alchemy_json = serde_json::to_vec_pretty(&alchemy_database)?;
    alchemy_json.push(b'\n');
    atomic_output::write_bytes(&data_output.join("items.json"), &item_json)?;
    atomic_output::write_bytes(&data_output.join("alchemy.json"), &alchemy_json)?;

    let icon_output = output_directory.join("item-icons");
    fs::create_dir_all(&icon_output)?;
    write_icon_sheets(icon_directory, &icon_output)?;

    let item_database = serde_json::from_slice(&item_json)?;
    let alchemy_database = serde_json::from_slice(&alchemy_json)?;
    render_website(&item_database, &alchemy_database, output_directory)
}

fn build_rule_based_recipes(
    tables: Vec<druaga_utils::item_catalog::RuleBasedAlchemyTable>,
) -> Result<Vec<RuleBasedAlchemyRecipe>, Box<dyn Error>> {
    let mut result = Vec::new();
    for table in tables {
        if table.category < 3 && table.affinity != 0 {
            continue;
        }
        let category = match table.category {
            0 => AlchemyIngredientCategory::Other,
            1 => AlchemyIngredientCategory::MaterialOrTool,
            2 => AlchemyIngredientCategory::Accessory,
            3 => AlchemyIngredientCategory::Weapon,
            4 => AlchemyIngredientCategory::OffHand,
            5 => AlchemyIngredientCategory::Head,
            6 => AlchemyIngredientCategory::Body,
            7 => AlchemyIngredientCategory::Arms,
            8 => AlchemyIngredientCategory::Feet,
            _ => {
                return Err(
                    format!("invalid rule-based alchemy category {}", table.category).into(),
                );
            }
        };
        let character = if table.category < 3 {
            None
        } else {
            Some(match table.affinity {
                0 => Character::Gilgamesh,
                1 => Character::Valkyrie,
                2 => Character::YoungKi,
                3 => Character::Xeovalga,
                _ => {
                    return Err(
                        format!("invalid rule-based alchemy affinity {}", table.affinity).into(),
                    );
                }
            })
        };
        for (index, recipe) in table.recipes.iter().enumerate() {
            if recipe.result_item_id == 0 {
                continue;
            }
            let next_result_item_id = (recipe.next_result_weight != 0)
                .then(|| table.recipes.get(index + 1).map(|next| next.result_item_id))
                .flatten()
                .filter(|id| *id != recipe.result_item_id && *id != 0);
            result.push(RuleBasedAlchemyRecipe {
                id: u32::try_from(result.len()).unwrap_or(u32::MAX),
                ingredient_category: category,
                character,
                point_level: u8::try_from(index + 2).unwrap_or(u8::MAX),
                result_item_id: recipe.result_item_id,
                next_result_item_id,
                next_result_weight: recipe.next_result_weight,
                completion_minutes: recipe.completion_minutes,
            });
        }
    }
    Ok(result)
}

fn render_from_databases(
    item_database: &Path,
    alchemy_database: &Path,
    output_directory: &Path,
) -> Result<(), Box<dyn Error>> {
    let item_database = serde_json::from_slice(&fs::read(item_database)?)?;
    let alchemy_database = serde_json::from_slice(&fs::read(alchemy_database)?)?;
    render_website(&item_database, &alchemy_database, output_directory)
}

fn build_item_database(
    items: &[Item],
    decompositions: &HashMap<u16, u16>,
    metadata: &TowerMetadata<'_>,
) -> Result<ItemDatabase, Box<dyn Error>> {
    let mut records = Vec::with_capacity(items.len());
    for item in items {
        let sheet = icon_sheet_name(item.resolved_icon_sheet()).ok_or_else(|| {
            format!(
                "item 0x{:04x} refers to unused icon sheet {}",
                item.id,
                item.resolved_icon_sheet()
            )
        })?;
        records.push(ItemRecord {
            id: item.id,
            name: item.name.clone(),
            description: item.description.clone(),
            category: item_category(item),
            catalog_order: item.list_order,
            purchase_value: item.purchase_value,
            sell_value: item.sell_value,
            alchemy_rank_points: item.rank_points,
            required_title_id: (item.required_title != 0).then_some(item.required_title),
            disassembles_to_item_id: decompositions.get(&item.id).copied(),
            server_present_rate_percent: (item.id == 0x401e).then_some(100),
            icon: IconReference {
                sheet: sheet.to_owned(),
                column: item.icon_x,
                row: item.icon_y,
            },
            equipment: if item.is_equipment() || is_accessory(item) {
                Some(build_equipment(item, metadata)?)
            } else {
                None
            },
        });
    }
    Ok(ItemDatabase {
        schema_version: 1,
        game_version: "1.60".to_owned(),
        items: records,
    })
}

fn build_equipment(item: &Item, metadata: &TowerMetadata<'_>) -> Result<Equipment, Box<dyn Error>> {
    let characters = [
        (1, Character::Gilgamesh),
        (2, Character::Valkyrie),
        (4, Character::YoungKi),
        (8, Character::Xeovalga),
    ]
    .into_iter()
    .filter_map(|(mask, character)| (item.character_mask & mask != 0).then_some(character))
    .collect::<Vec<_>>();
    let effect_character = characters
        .first()
        .copied()
        .ok_or_else(|| format!("equipment 0x{:04x} has no character", item.id))?;
    let slot = match item.equip_slot {
        0 if is_accessory(item) => EquipmentSlot::Accessory,
        1 => EquipmentSlot::Weapon,
        2 => EquipmentSlot::OffHand,
        3 => EquipmentSlot::Head,
        4 => EquipmentSlot::Body,
        5 => EquipmentSlot::Arms,
        6 => EquipmentSlot::Feet,
        _ => return Err(format!("equipment 0x{:04x} has invalid slot", item.id).into()),
    };
    let required_rank = Rank {
        value: item.required_rank,
        label: metadata.rank_name(item.required_rank)?,
    };
    let effects = item
        .effects
        .iter()
        .map(|effect| typed_effect(*effect, effect_character, metadata))
        .collect::<Result<_, _>>()?;
    Ok(Equipment {
        characters,
        slot,
        required_rank,
        attack: (item.flags & 0x100 != 0).then_some(item.attack_min),
        defense: (item.flags & 0x200 != 0).then_some(item.attack_max_or_defense),
        weight: (item.flags & 0x100 != 0).then_some(item.attack_max_or_defense),
        effects,
    })
}

fn typed_effect(
    effect: Effect,
    character: Character,
    metadata: &TowerMetadata<'_>,
) -> Result<ItemEffect, Box<dyn Error>> {
    let amount = effect.value;
    let result = match effect.id {
        4 => ItemEffect::MaximumHp { amount },
        5 => ItemEffect::MaximumAp { amount },
        6 => ItemEffect::HpConvertedToAp { amount },
        7 => ItemEffect::ApConvertedToHp { amount },
        8 => ItemEffect::Strength { amount },
        9 => ItemEffect::Vitality { amount },
        10 => ItemEffect::Intelligence { amount },
        11 => ItemEffect::Spirit { amount },
        12 => ItemEffect::Dexterity { amount },
        13 => ItemEffect::Agility { amount },
        14 => ItemEffect::AttackPower { amount },
        15 => ItemEffect::PhysicalDefense { amount },
        16 => ItemEffect::MagicDefense { amount },
        17 => ItemEffect::Damage { amount },
        18 => ItemEffect::FinalDamagePercent { percent: amount },
        19 => ItemEffect::RetaliationDamage { amount },
        20 => ItemEffect::PhysicalDamageReceivedPercent { percent: amount },
        21 => ItemEffect::MagicDamageReceivedPercent { percent: amount },
        22 => ItemEffect::MovementSpeedPercent { percent: amount },
        23 => ItemEffect::AttackSpeedPercent { percent: amount },
        24 => ItemEffect::CastingSpeedPercent { percent: amount },
        25 => ItemEffect::AccuracyPercent { percent: amount },
        26 => ItemEffect::EvasionPercent { percent: amount },
        27 => ItemEffect::CriticalRatePercent { percent: amount },
        28 => ItemEffect::Resistance { amount },
        29 => ItemEffect::EnemyFamilyAdvantage {
            family: metadata.enemy_family(amount)?,
        },
        30 => ItemEffect::EnemyFamilyConcealment {
            family: metadata.enemy_family(amount)?,
        },
        0xb0..=0xdf => ItemEffect::AbilityLevel {
            ability: metadata.ability_name(character, effect.id)?,
            levels: amount,
        },
        0xe0..=0xff => ItemEffect::AbilityStrength {
            ability: metadata.ability_name(character, effect.id)?,
            strength: amount,
        },
        _ => return Err(format!("unsupported item effect 0x{:02x}", effect.id).into()),
    };
    Ok(result)
}

impl<'a> TowerMetadata<'a> {
    fn new(executable: &'a [u8]) -> Result<Self, Box<dyn Error>> {
        if executable.get(..2) != Some(b"MZ") {
            return Err("the Tower executable does not have an MZ header".into());
        }
        Ok(Self { executable })
    }

    fn rank_name(&self, rank: u8) -> Result<String, Box<dyn Error>> {
        if rank > 31 {
            return Err(format!("invalid equipment rank {rank}").into());
        }
        let name = self.pointer_at(RANK_NAME_TABLE + u32::from(rank) * 4)?;
        self.string_at(name)
    }

    fn enemy_family(&self, value: i16) -> Result<String, Box<dyn Error>> {
        let index = u32::try_from(value).map_err(|_| format!("invalid enemy family {value}"))?;
        if index > 3 {
            return Err(format!("invalid enemy family {value}").into());
        }
        let name = self.pointer_at(ENEMY_FAMILY_NAMES + index * 4)?;
        self.string_at(name)
    }

    fn ability_name(&self, character: Character, effect_id: u8) -> Result<String, Box<dyn Error>> {
        let table = match effect_id & 0xf0 {
            0xb0 | 0xe0 => FORCE_EFFECT_TABLES,
            0xc0 | 0xf0 => MAGIC_EFFECT_TABLES,
            0xd0 => SPECIAL_EFFECT_TABLES,
            _ => return Err(format!("effect 0x{effect_id:02x} has no ability table").into()),
        };
        let character_index = match character {
            Character::Gilgamesh => 0,
            Character::Valkyrie => 1,
            Character::YoungKi => 2,
            Character::Xeovalga => 3,
        };
        let records = self.pointer_at(table + character_index * 4)?;
        let record = records + u32::from(effect_id & 0x0f) * EFFECT_RECORD_SIZE;
        let name = self.pointer_at(record + 4)?;
        self.string_at(name)
    }

    fn pointer_at(&self, virtual_address: u32) -> Result<u32, Box<dyn Error>> {
        let offset = self.file_offset(virtual_address)?;
        let bytes = self.executable.get(offset..offset + 4).ok_or_else(|| {
            format!("pointer at 0x{virtual_address:08x} is outside the executable")
        })?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap_or([0; 4])))
    }

    fn string_at(&self, virtual_address: u32) -> Result<String, Box<dyn Error>> {
        let offset = self.file_offset(virtual_address)?;
        let bytes = self.executable.get(offset..).ok_or_else(|| {
            format!("string at 0x{virtual_address:08x} is outside the executable")
        })?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| format!("string at 0x{virtual_address:08x} is not terminated"))?;
        let (value, _, malformed) = SHIFT_JIS.decode(&bytes[..end]);
        if malformed {
            return Err(format!("string at 0x{virtual_address:08x} is not Shift JIS").into());
        }
        Ok(value.into_owned())
    }

    fn file_offset(&self, virtual_address: u32) -> Result<usize, Box<dyn Error>> {
        let offset = virtual_address
            .checked_sub(IMAGE_BASE)
            .ok_or_else(|| format!("address 0x{virtual_address:08x} is below the image base"))?;
        Ok(usize::try_from(offset)?)
    }
}

fn item_category(item: &Item) -> ItemCategory {
    if is_accessory(item) {
        ItemCategory::Accessory
    } else if item.is_equipment() {
        ItemCategory::Equipment
    } else if item.id & 0xf000 == 0x4000 {
        ItemCategory::Quest
    } else if (0x2000..0x4000).contains(&item.id) && item.flags & 8 == 0 {
        ItemCategory::Consumable
    } else if (0x2000..0x4000).contains(&item.id) && item.flags & 8 != 0 {
        ItemCategory::MaterialOrTool
    } else {
        ItemCategory::Other
    }
}

fn is_accessory(item: &Item) -> bool {
    item.id & 0xff00 == 0x9f00
}

fn write_icon_sheets(input: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    for sheet in ICON_SHEETS {
        let source = fs::read(input.join(format!("{sheet}.gsm")))?;
        let image = Image::parse(&source)?;
        let bytes = image.rgba_pixels();
        let pixels: Vec<_> = bytes
            .chunks_exact(4)
            .map(|pixel| RGBA8::new(pixel[0], pixel[1], pixel[2], pixel[3]))
            .collect();
        let encoded = Encoder::new()
            .with_quality(ICON_QUALITY)
            .with_speed(4)
            .encode_rgba(Img::new(
                &pixels,
                usize::from(image.width),
                usize::from(image.height),
            ))?;
        atomic_output::write_bytes(&output.join(format!("{sheet}.avif")), &encoded.avif_file)?;
    }
    Ok(())
}

fn render_website(
    database: &ItemDatabase,
    alchemy: &AlchemyDatabase,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output)?;
    atomic_output::write_bytes(&output.join("items.css"), ITEM_STYLE.as_bytes())?;
    let items: HashMap<_, _> = database.items.iter().map(|item| (item.id, item)).collect();
    write_landing_page(database, alchemy, output)?;
    for (character, title, file) in [
        (Character::Gilgamesh, "Gilgamesh", "items-gilgamesh.html"),
        (Character::Valkyrie, "Valkyrie", "items-walkure.html"),
        (Character::YoungKi, "Young Ki", "items-young-ki.html"),
        (Character::Xeovalga, "Xeovalga", "items-xeovalga.html"),
    ] {
        write_equipment_page(database, &items, character, title, file, output)?;
    }
    write_accessory_page(database, &items, output)?;
    write_category_page(
        database,
        &items,
        "Quest items",
        "items-quest.html",
        |item| matches!(item.category, ItemCategory::Quest),
        output,
    )?;
    write_category_page(
        database,
        &items,
        "Consumables",
        "items-consumables.html",
        |item| matches!(item.category, ItemCategory::Consumable),
        output,
    )?;
    write_category_page(
        database,
        &items,
        "Other items",
        "items-other.html",
        |item| {
            matches!(
                item.category,
                ItemCategory::MaterialOrTool | ItemCategory::Other
            )
        },
        output,
    )
}

fn write_accessory_page(
    database: &ItemDatabase,
    items: &HashMap<u16, &ItemRecord>,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let file = "items-accessories.html";
    let mut html = page_start("Accessories", file);
    html.push_str("<section class=\"item-section\" id=\"accessory\"><h2>Accessories</h2><div class=\"table-scroll\"><table class=\"item-table equipment-table\"><thead><tr><th>Item</th><th>Required rank</th><th>Attack</th><th>Defense</th><th>Weight</th><th>Effects</th><th>Sell</th><th>Disassembly</th></tr></thead><tbody>");
    for item in database.items.iter().filter(|item| {
        item.equipment
            .as_ref()
            .is_some_and(|equipment| equipment.slot == EquipmentSlot::Accessory)
    }) {
        write_equipment_row(&mut html, item, items)?;
    }
    html.push_str("</tbody></table></div></section>");
    page_end(&mut html);
    atomic_output::write_bytes(&output.join(file), html.as_bytes())?;
    Ok(())
}

fn write_landing_page(
    database: &ItemDatabase,
    alchemy: &AlchemyDatabase,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut html = page_start("Item database", "items.html");
    write!(
        html,
        "<section class=\"intro\"><p><strong>{}</strong> items and <strong>{}</strong> exact alchemy recipes.</p></section><section class=\"page-grid\">",
        database.items.len(),
        alchemy.recipes.len()
    )?;
    for (file, title) in &PAGES[1..] {
        write!(
            html,
            "<a class=\"page-card\" href=\"{}\"><strong>{}</strong><span>Open</span></a>",
            escape(file),
            escape(title)
        )?;
    }
    html.push_str("</section>");
    page_end(&mut html);
    atomic_output::write_bytes(&output.join("items.html"), html.as_bytes())?;
    Ok(())
}

fn write_equipment_page(
    database: &ItemDatabase,
    items: &HashMap<u16, &ItemRecord>,
    character: Character,
    title: &str,
    file: &str,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut html = page_start(&format!("{title} equipment"), file);
    for slot in all_slots() {
        write!(
            html,
            "<section class=\"item-section\" id=\"{}\"><h2>{}</h2><div class=\"table-scroll\"><table class=\"item-table equipment-table\"><thead><tr><th>Item</th><th>Required rank</th><th>Attack</th><th>Defense</th><th>Weight</th><th>Effects</th><th>Sell</th><th>Disassembly</th></tr></thead><tbody>",
            slot_id(slot),
            slot_name(slot)
        )?;
        for item in database.items.iter().filter(|item| {
            item.equipment.as_ref().is_some_and(|equipment| {
                equipment.slot == slot && equipment.characters.contains(&character)
            })
        }) {
            write_equipment_row(&mut html, item, items)?;
        }
        html.push_str("</tbody></table></div></section>");
    }
    page_end(&mut html);
    atomic_output::write_bytes(&output.join(file), html.as_bytes())?;
    Ok(())
}

fn write_category_page(
    database: &ItemDatabase,
    items: &HashMap<u16, &ItemRecord>,
    title: &str,
    file: &str,
    predicate: impl Fn(&ItemRecord) -> bool,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut html = page_start(title, file);
    html.push_str("<section class=\"item-section\"><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Item</th><th>Description</th><th>Sell</th><th>Disassembly</th></tr></thead><tbody>");
    for item in database.items.iter().filter(|item| predicate(item)) {
        write_item_row(&mut html, item, items)?;
    }
    html.push_str("</tbody></table></div></section>");
    page_end(&mut html);
    atomic_output::write_bytes(&output.join(file), html.as_bytes())?;
    Ok(())
}

fn write_equipment_row(
    html: &mut String,
    item: &ItemRecord,
    items: &HashMap<u16, &ItemRecord>,
) -> Result<(), std::fmt::Error> {
    let Some(equipment) = item.equipment.as_ref() else {
        return Err(std::fmt::Error);
    };
    write!(html, "<tr id=\"item-{:04x}\"><td>", item.id)?;
    write_item_identity(html, item, true)?;
    write!(
        html,
        "</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"effects\">",
        escape(&equipment.required_rank.label),
        optional_number(equipment.attack),
        optional_number(equipment.defense),
        optional_number(equipment.weight)
    )?;
    if equipment.effects.is_empty() {
        html.push('—');
    } else {
        for (index, effect) in equipment.effects.iter().enumerate() {
            if index != 0 {
                html.push_str("<br>");
            }
            html.push_str(&escape(&effect_text(effect)));
        }
    }
    write!(
        html,
        "</td><td class=\"number\">{}</td><td>{}</td></tr>",
        item.sell_value,
        disassembly_link(item, items)
    )
}

fn write_item_row(
    html: &mut String,
    item: &ItemRecord,
    items: &HashMap<u16, &ItemRecord>,
) -> Result<(), std::fmt::Error> {
    write!(html, "<tr id=\"item-{:04x}\"><td>", item.id)?;
    write_item_identity(html, item, false)?;
    write!(
        html,
        "</td><td>{}</td><td class=\"number\">{}</td><td>{}</td></tr>",
        formatted_text(&item.description),
        item.sell_value,
        disassembly_link(item, items)
    )
}

fn write_item_identity(
    html: &mut String,
    item: &ItemRecord,
    include_description: bool,
) -> Result<(), std::fmt::Error> {
    write!(
        html,
        "<div class=\"item-name\"><span class=\"item-icon\" role=\"img\" aria-label=\"{} icon\" style=\"background-image:url('item-icons/{}.avif');background-position:-{}px -{}px\"></span><span><strong>{}</strong><code>0x{:04X}</code>",
        escape(&item.name),
        escape(&item.icon.sheet),
        u16::from(item.icon.column) * u16::from(ICON_PITCH),
        u16::from(item.icon.row) * u16::from(ICON_PITCH),
        escape(&item.name),
        item.id
    )?;
    if include_description {
        write!(html, "<small>{}</small>", formatted_text(&item.description))?;
    }
    html.push_str("</span></div>");
    Ok(())
}

fn disassembly_link(item: &ItemRecord, items: &HashMap<u16, &ItemRecord>) -> String {
    let Some(result_id) = item.disassembles_to_item_id else {
        return "—".to_owned();
    };
    let Some(result) = items.get(&result_id) else {
        return format!("0x{result_id:04X}");
    };
    format!(
        "<a href=\"{}#item-{result_id:04x}\">{}</a>",
        item_page(result),
        escape(&result.name)
    )
}

fn effect_text(effect: &ItemEffect) -> String {
    let signed = |value: i16| {
        if value > 0 {
            format!("+{value}")
        } else {
            value.to_string()
        }
    };
    match effect {
        ItemEffect::MaximumHp { amount } => format!("Maximum HP {}", signed(*amount)),
        ItemEffect::MaximumAp { amount } => format!("Maximum AP {}", signed(*amount)),
        ItemEffect::HpConvertedToAp { amount } => {
            format!("HP to AP conversion {}", signed(*amount))
        }
        ItemEffect::ApConvertedToHp { amount } => {
            format!("AP to HP conversion {}", signed(*amount))
        }
        ItemEffect::Strength { amount } => format!("Strength {}", signed(*amount)),
        ItemEffect::Vitality { amount } => format!("Vitality {}", signed(*amount)),
        ItemEffect::Intelligence { amount } => format!("Intelligence {}", signed(*amount)),
        ItemEffect::Spirit { amount } => format!("Spirit {}", signed(*amount)),
        ItemEffect::Dexterity { amount } => format!("Dexterity {}", signed(*amount)),
        ItemEffect::Agility { amount } => format!("Agility {}", signed(*amount)),
        ItemEffect::AttackPower { amount } => format!("Attack power {}", signed(*amount)),
        ItemEffect::PhysicalDefense { amount } => format!("Physical defense {}", signed(*amount)),
        ItemEffect::MagicDefense { amount } => format!("Magic defense {}", signed(*amount)),
        ItemEffect::Damage { amount } => format!("Damage {}", signed(*amount)),
        ItemEffect::FinalDamagePercent { percent } => format!("Final damage {}%", signed(*percent)),
        ItemEffect::RetaliationDamage { amount } => {
            format!("Retaliation damage {}", signed(*amount))
        }
        ItemEffect::PhysicalDamageReceivedPercent { percent } => {
            format!("Physical damage received {}%", signed(*percent))
        }
        ItemEffect::MagicDamageReceivedPercent { percent } => {
            format!("Magic damage received {}%", signed(*percent))
        }
        ItemEffect::MovementSpeedPercent { percent } => {
            format!("Movement speed {}%", signed(*percent))
        }
        ItemEffect::AttackSpeedPercent { percent } => format!("Attack speed {}%", signed(*percent)),
        ItemEffect::CastingSpeedPercent { percent } => {
            format!("Casting speed {}%", signed(*percent))
        }
        ItemEffect::AccuracyPercent { percent } => format!("Accuracy {}%", signed(*percent)),
        ItemEffect::EvasionPercent { percent } => format!("Evasion {}%", signed(*percent)),
        ItemEffect::CriticalRatePercent { percent } => {
            format!("Critical rate {}%", signed(*percent))
        }
        ItemEffect::Resistance { amount } => format!("Resistance {}", signed(*amount)),
        ItemEffect::EnemyFamilyAdvantage { family } => format!("Advantage against {family}"),
        ItemEffect::EnemyFamilyConcealment { family } => format!("Hidden from {family}"),
        ItemEffect::AbilityLevel { ability, levels } => {
            format!("{ability} level {}", signed(*levels))
        }
        ItemEffect::AbilityStrength { ability, strength } => {
            format!("Strengthens {ability} {}", signed(*strength))
        }
    }
}

fn optional_number(value: Option<i16>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

fn page_start(title: &str, current_file: &str) -> String {
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{} · The Tower of Druaga</title><link rel=\"stylesheet\" href=\"items.css\"></head><body><button class=\"menu\" type=\"button\">Item index</button><nav class=\"sidebar\"><div class=\"brand\"><b>THE TOWER OF DRUAGA</b><small>AON.Net item database</small></div><a href=\"index.html\">Hidden chest guide</a><a href=\"items.html\"{}>Item database</a>",
        escape(title),
        current(current_file, "items.html")
    );
    for (file, character) in &PAGES[1..5] {
        let open = if *file == current_file { " open" } else { "" };
        let _ = write!(
            html,
            "<details{open}><summary><a href=\"{file}\"{}>{character}</a></summary>",
            current(current_file, file)
        );
        for slot in all_slots() {
            let _ = write!(
                html,
                "<a href=\"{file}#{}\">{}</a>",
                slot_id(slot),
                slot_name(slot)
            );
        }
        html.push_str("</details>");
    }
    for (file, label) in &PAGES[5..] {
        let _ = write!(
            html,
            "<a href=\"{file}\"{}>{label}</a>",
            current(current_file, file)
        );
    }
    let _ = write!(
        html,
        "</nav><main><header class=\"page-title\"><h1>{}</h1></header>",
        escape(title)
    );
    html
}

fn page_end(html: &mut String) {
    html.push_str("</main><script>const menu=document.querySelector('.menu'),sidebar=document.querySelector('.sidebar');menu.addEventListener('click',()=>sidebar.classList.toggle('open'));</script></body></html>");
}

fn current(current_file: &str, file: &str) -> &'static str {
    if current_file == file {
        " aria-current=\"page\""
    } else {
        ""
    }
}

fn item_page(item: &ItemRecord) -> &'static str {
    match item.category {
        ItemCategory::Quest => "items-quest.html",
        ItemCategory::Consumable => "items-consumables.html",
        ItemCategory::Accessory => "items-accessories.html",
        ItemCategory::Equipment => match item
            .equipment
            .as_ref()
            .and_then(|equipment| equipment.characters.first())
        {
            Some(Character::Gilgamesh) => "items-gilgamesh.html",
            Some(Character::Valkyrie) => "items-walkure.html",
            Some(Character::YoungKi) => "items-young-ki.html",
            Some(Character::Xeovalga) => "items-xeovalga.html",
            None => "items-other.html",
        },
        ItemCategory::MaterialOrTool | ItemCategory::Other => "items-other.html",
    }
}

fn all_slots() -> [EquipmentSlot; 6] {
    [
        EquipmentSlot::Weapon,
        EquipmentSlot::OffHand,
        EquipmentSlot::Head,
        EquipmentSlot::Body,
        EquipmentSlot::Arms,
        EquipmentSlot::Feet,
    ]
}

fn slot_name(slot: EquipmentSlot) -> &'static str {
    match slot {
        EquipmentSlot::Weapon => "Weapon",
        EquipmentSlot::OffHand => "Off hand",
        EquipmentSlot::Head => "Head",
        EquipmentSlot::Body => "Body",
        EquipmentSlot::Arms => "Arms",
        EquipmentSlot::Feet => "Feet",
        EquipmentSlot::Accessory => "Accessory",
    }
}

fn slot_id(slot: EquipmentSlot) -> &'static str {
    match slot {
        EquipmentSlot::Weapon => "weapon",
        EquipmentSlot::OffHand => "off-hand",
        EquipmentSlot::Head => "head",
        EquipmentSlot::Body => "body",
        EquipmentSlot::Arms => "arms",
        EquipmentSlot::Feet => "feet",
        EquipmentSlot::Accessory => "accessory",
    }
}

fn icon_sheet_name(index: usize) -> Option<&'static str> {
    match index {
        0..=10 => ICON_SHEETS.get(index).copied(),
        12..=17 => ICON_SHEETS.get(index - 1).copied(),
        19..=24 => ICON_SHEETS.get(index - 2).copied(),
        26..=30 => ICON_SHEETS.get(index - 3).copied(),
        _ => None,
    }
}

fn formatted_text(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let line = line
                .strip_prefix("$$01")
                .or_else(|| line.strip_prefix("$$02"))
                .unwrap_or(line);
            escape(line)
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const ITEM_STYLE: &str = r#"
:root{--ink:#2a2118;--paper:#e8d8af;--paper2:#f5ebd0;--gold:#ae7b26;--gold2:#d6b45f;--blue:#344f67;--line:#967846}*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;color:var(--ink);background:#1d2424 radial-gradient(circle at 50% -20%,#4a5960 0,#1d2424 48rem);font:15px/1.45 Georgia,"Times New Roman",serif}a{color:#284e68}.sidebar{position:fixed;inset:0 auto 0 0;width:278px;padding:22px 16px;overflow:auto;color:#f4e9ca;background:linear-gradient(180deg,#2f4a61,#1c2b36);border-right:3px solid var(--gold);box-shadow:6px 0 24px #0008;z-index:5}.brand{margin:0 0 12px;padding:0 8px 17px;border-bottom:1px solid #d6b45f66}.brand b{display:block;color:#f0cd76;font-size:21px;letter-spacing:.04em}.brand small{color:#d8d8c8}.sidebar>a,.sidebar details>a,.sidebar summary>a{display:block;padding:7px 9px;color:#eee6cf;text-decoration:none;border-left:2px solid transparent}.sidebar>a:hover,.sidebar details>a:hover,.sidebar a[aria-current=page]{color:white;background:#ffffff12;border-left-color:#f0cd76}.sidebar details{margin:5px 0}.sidebar summary{color:#f0cd76;cursor:pointer;font-weight:bold;list-style-position:inside}.sidebar summary>a{display:inline;padding-left:3px;color:#f0cd76}.sidebar details>a{padding-left:25px;font-size:13px}.menu{display:none}main{margin-left:278px;padding:34px clamp(18px,4vw,60px) 80px}.page-title,.intro,.item-section,.page-grid{max-width:1500px;margin:0 auto 24px;background:linear-gradient(135deg,var(--paper2),var(--paper));border:1px solid #e9ce83;border-radius:5px;box-shadow:0 12px 35px #0007}.page-title{padding:24px 30px;border-top:8px solid var(--gold)}.page-title h1{margin:0;color:var(--blue);font-size:clamp(28px,4vw,43px)}.intro{padding:20px 28px}.intro p{margin:0}.item-section{padding:22px;scroll-margin-top:12px}.item-section>h2{margin:0 0 14px;color:var(--blue);border-bottom:2px solid var(--line)}.table-scroll{overflow-x:auto}.item-table{width:100%;border-collapse:collapse;background:#fff8e2aa}.item-table th{position:sticky;top:0;padding:8px 9px;color:#f5e9c9;background:#2f4a61;text-align:left;white-space:nowrap}.item-table td{padding:7px 9px;vertical-align:top;border-bottom:1px solid #c6ae78}.item-table tbody tr:hover{background:#fffdf4}.item-table .number{text-align:right;white-space:nowrap}.equipment-table th:first-child{min-width:300px}.equipment-table .effects{min-width:205px}.item-name{display:grid;grid-template-columns:42px minmax(230px,1fr);gap:8px}.item-icon{display:block;width:34px;height:34px;margin:1px 0;background-repeat:no-repeat}.item-name strong{display:block;color:var(--blue);font-size:16px}.item-name code{margin-left:8px;color:#766b57;font-size:11px}.item-name small{display:block;max-width:520px;color:#544b3d;font-size:12px}.page-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px;padding:28px}.page-card{display:flex;justify-content:space-between;padding:18px;color:var(--ink);background:#fff8e2aa;border:1px solid #b99a5d;text-decoration:none}.page-card strong{color:var(--blue)}@media(max-width:900px){.sidebar{transform:translateX(-100%);transition:.2s}.sidebar.open{transform:none}.menu{display:block;position:fixed;top:10px;left:10px;z-index:8;padding:9px 12px;color:white;background:var(--blue);border:1px solid var(--gold2)}main{margin-left:0;padding:58px 10px 50px}.page-title,.intro,.item-section,.page-grid{padding:18px 14px}.page-grid{grid-template-columns:1fr}}@media print{body{background:white}.sidebar,.menu{display:none}main{margin:0;padding:0}.page-title,.intro,.item-section,.page-grid{box-shadow:none}.item-table th{position:static}.item-table tr{break-inside:avoid}}
"#;
