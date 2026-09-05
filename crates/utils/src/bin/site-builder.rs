use std::collections::{BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use druaga_utils::atomic_output;
use druaga_utils::item_database::{
    AlchemyDatabase, AlchemyIngredientCategory, Character, Equipment, EquipmentSlot, ItemCategory,
    ItemDatabase, ItemEffect, ItemRecord, RuleBasedAlchemyRecipe, WeaponBonus,
};
use druaga_utils::site_database::{
    ChestGuideDatabase, ChestRewardValue, ChestTier, EnemyDatabase, EnemyDrop, EnemyRecord,
    QuestCategory, QuestSourceDatabase, QuestSourceIdentity, QuestSourceQuest, SolFloor, SolKind,
    TowerSourceDatabase, character_name,
};

const ICON_PITCH: u16 = 34;
const CHARACTERS: [(Character, &str, &str); 4] = [
    (Character::Gilgamesh, "Gilgamesh", "gilgamesh"),
    (Character::Valkyrie, "Valkyrie", "walkure"),
    (Character::YoungKi, "Young Ki", "young-ki"),
    (Character::Xeovalga, "Xeovalga", "xeovalga"),
];
const SLOTS: [EquipmentSlot; 6] = [
    EquipmentSlot::Weapon,
    EquipmentSlot::OffHand,
    EquipmentSlot::Head,
    EquipmentSlot::Body,
    EquipmentSlot::Arms,
    EquipmentSlot::Feet,
];

struct Arguments {
    items: PathBuf,
    alchemy: PathBuf,
    chests: PathBuf,
    enemies: PathBuf,
    quest_sources: PathBuf,
    tower_sources: PathBuf,
    output: PathBuf,
}

struct SiteData {
    items: ItemDatabase,
    alchemy: AlchemyDatabase,
    chests: ChestGuideDatabase,
    enemies: EnemyDatabase,
    quest_sources: QuestSourceDatabase,
    tower_sources: TowerSourceDatabase,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(env::args_os())?;
    let data = SiteData {
        items: read_json(&args.items)?,
        alchemy: read_json(&args.alchemy)?,
        chests: read_json(&args.chests)?,
        enemies: read_json(&args.enemies)?,
        quest_sources: read_json(&args.quest_sources)?,
        tower_sources: read_json(&args.tower_sources)?,
    };
    validate(&data)?;
    render_site(&data, &args.output)
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Arguments, Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    let [
        _,
        items,
        alchemy,
        chests,
        enemies,
        quest_sources,
        tower_sources,
        output,
    ] = args.as_slice()
    else {
        return Err("usage: site-builder ITEMS.JSON ALCHEMY.JSON CHESTS.JSON ENEMIES.JSON QUEST-SOURCES.JSON TOWER-SOURCES.JSON OUTPUT-DIRECTORY".into());
    };
    Ok(Arguments {
        items: items.into(),
        alchemy: alchemy.into(),
        chests: chests.into(),
        enemies: enemies.into(),
        quest_sources: quest_sources.into(),
        tower_sources: tower_sources.into(),
        output: output.into(),
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn validate(data: &SiteData) -> Result<(), Box<dyn Error>> {
    let items: HashMap<_, _> = data
        .items
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect();
    for recipe in &data.alchemy.recipes {
        require_item(&items, recipe.result_item_id, "alchemy result")?;
        for id in &recipe.ingredient_item_ids {
            require_item(&items, *id, "alchemy ingredient")?;
        }
    }
    for recipe in &data.alchemy.rule_based_recipes {
        require_item(&items, recipe.result_item_id, "rule-based alchemy result")?;
        if let Some(id) = recipe.next_result_item_id {
            require_item(&items, id, "rule-based alchemy next result")?;
        }
        if !(2..=32).contains(&recipe.point_level) {
            return Err(format!(
                "rule-based alchemy recipe {} has invalid point level {}",
                recipe.id, recipe.point_level
            )
            .into());
        }
    }
    for quest in &data.chests.quests {
        for chest in &quest.chests {
            for reward in &chest.rewards {
                if let ChestRewardValue::Item { item_id, .. } = reward.value {
                    require_item(&items, item_id, "chest reward")?;
                }
            }
        }
    }
    for enemy in &data.enemies.enemies {
        if enemy.total_item_weight
            != enemy
                .drops
                .iter()
                .map(|drop| u16::from(drop.weight))
                .sum::<u16>()
        {
            return Err(format!(
                "enemy 0x{:04x} has an invalid total weight",
                enemy.definition_id
            )
            .into());
        }
        for drop in &enemy.drops {
            require_item(&items, drop.item_id, "enemy drop")?;
        }
        for quest_id in &enemy.quest_ids {
            if !data.chests.quests.iter().any(|quest| quest.id == *quest_id) {
                return Err(format!(
                    "enemy 0x{:04x} refers to quest {quest_id}",
                    enemy.definition_id
                )
                .into());
            }
        }
    }
    let mut treasure_pools = HashMap::new();
    for pool in &data.quest_sources.treasure_pools {
        if treasure_pools.insert(pool.id.as_str(), pool).is_some() {
            return Err(format!("duplicate quest treasure pool {}", pool.id).into());
        }
        for reward in &pool.rewards {
            require_item(&items, reward.item_id, "quest treasure reward")?;
            match (reward.chance_numerator, reward.chance_denominator) {
                (Some(numerator), Some(denominator))
                    if denominator != 0 && numerator <= denominator => {}
                (None, None) => {}
                _ => {
                    return Err(format!(
                        "quest treasure pool {} has an invalid reward chance",
                        pool.id
                    )
                    .into());
                }
            }
        }
    }
    let mut direct_reward_pools = HashMap::new();
    for pool in &data.quest_sources.direct_reward_pools {
        if direct_reward_pools.insert(pool.id.as_str(), pool).is_some() {
            return Err(format!("duplicate direct reward pool {}", pool.id).into());
        }
        for reward in &pool.rewards {
            require_item(&items, reward.item_id, "direct quest reward")?;
            match (reward.chance_numerator, reward.chance_denominator) {
                (Some(numerator), Some(denominator))
                    if denominator != 0 && numerator <= denominator => {}
                (None, None) => {}
                _ => {
                    return Err(format!(
                        "direct reward pool {} has an invalid reward condition",
                        pool.id
                    )
                    .into());
                }
            }
        }
    }
    for quest in &data.quest_sources.quests {
        for reward in &quest.rewards {
            require_item(&items, reward.item_id, "quest item source")?;
            for item_id in &reward.required_item_ids {
                require_item(&items, *item_id, "quest reward requirement")?;
            }
            for item_id in &reward.consumed_item_ids {
                require_item(&items, *item_id, "quest reward consumption")?;
            }
        }
        for source in &quest.treasure_sources {
            if !treasure_pools.contains_key(source.pool_id.as_str()) {
                return Err(format!(
                    "quest {} refers to missing treasure pool {}",
                    quest.name, source.pool_id
                )
                .into());
            }
        }
        for source in &quest.direct_reward_sources {
            if !direct_reward_pools.contains_key(source.pool_id.as_str()) {
                return Err(format!(
                    "quest {} refers to missing direct reward pool {}",
                    quest.name, source.pool_id
                )
                .into());
            }
        }
    }
    for source in &data.tower_sources.sources {
        for reward in &source.rewards {
            require_item(&items, reward.item_id, "Tower reward")?;
            match (reward.chance_numerator, reward.chance_denominator) {
                (Some(numerator), Some(denominator))
                    if denominator != 0 && numerator <= denominator => {}
                (None, None) => {}
                _ => return Err(format!("Tower source {} has an invalid chance", source.id).into()),
            }
        }
    }
    Ok(())
}

fn require_item(
    items: &HashMap<u16, &ItemRecord>,
    id: u16,
    source: &str,
) -> Result<(), Box<dyn Error>> {
    if items.contains_key(&id) {
        Ok(())
    } else {
        Err(format!("{source} refers to missing item 0x{id:04x}").into())
    }
}

fn render_site(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output)?;
    atomic_output::write_bytes(&output.join("site.css"), SITE_STYLE.as_bytes())?;
    write_guide(data, output)?;
    write_item_pages(data, output)?;
    write_crafting_pages(data, output)?;
    write_enemy_page(data, output)?;
    write_quest_source_page(data, output)?;
    write_tower_source_page(data, output)?;
    Ok(())
}

fn write_guide(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    let items = item_map(data);
    let mut html = page_start("Hidden Treasure Chests", "index.html", data);
    html.push_str("<header class=\"hero\"><span class=\"eyebrow\">Station Version 1.60</span><h1>Hidden Treasure Chests</h1><p>This guide gives the required player action and the chest contents for all 78 quests.</p><p><strong>Rule:</strong> If more than one condition succeeds, the game creates only the highest chest tier.</p><p><strong>Chest maps:</strong> A filled red area is the exact trigger area. A pale ring, line, or number helps you find the area.</p><p><strong>Sol maps:</strong> The quest can select one or more marks for each run. Stand at the center of a ring and use Force. The small inner ring shows the exact check area. The large outer ring helps you find the location.</p><div class=\"sol-legend\"><span class=\"sol\">Sol</span><span class=\"silver-sol\">Silver Sol</span><span class=\"gold-sol\">Gold Sol</span></div></header>");
    for quest in &data.chests.quests {
        write!(
            html,
            "<section class=\"quest\" id=\"quest-{}\" data-name=\"{}\"><header class=\"quest-title\"><div><span class=\"eyebrow\">{} · Difficulty {}</span><h2>{}</h2></div><a href=\"#quest-{}\">Link</a></header><div class=\"chest-grid\">",
            quest.id,
            escape(&quest.name),
            quest_category_name(quest.category),
            quest.difficulty,
            escape(&quest.name),
            quest.id
        )?;
        for chest in &quest.chests {
            write!(
                html,
                "<article class=\"chest {}\"><header><h3>{}</h3></header><h4>Player action</h4><p>{}</p>",
                chest_tier_class(chest.tier),
                chest_tier_name(chest.tier),
                escape(&chest.player_action)
            )?;
            for illustration in &chest.illustrations {
                html.push_str(&crop_map_illustration(illustration)?);
            }
            if !chest.variants.is_empty() {
                html.push_str(
                    "<details class=\"variants\"><summary>Quest variants</summary><table><tbody>",
                );
                for variant in &chest.variants {
                    write!(
                        html,
                        "<tr><th>{}</th><td>{}</td></tr>",
                        escape(&variant.name),
                        escape(&variant.player_action)
                    )?;
                }
                html.push_str("</tbody></table></details>");
            }
            html.push_str("<h4>Contents</h4><dl class=\"rewards\">");
            for reward in &chest.rewards {
                write!(html, "<div><dt>{}</dt><dd>", escape(&reward.recipient))?;
                match reward.value {
                    ChestRewardValue::Item { item_id, quantity } => {
                        let item = items[&item_id];
                        write!(
                            html,
                            "<a href=\"{}#item-{item_id:04x}\">{}</a>{}",
                            item_page(item),
                            escape(&item.name),
                            if quantity == 1 {
                                String::new()
                            } else {
                                format!(" × {quantity}")
                            }
                        )?;
                    }
                    ChestRewardValue::Gold { amount } => write!(html, "{amount} Gold")?,
                }
                html.push_str("</dd></div>");
            }
            html.push_str("</dl></article>");
        }
        html.push_str("</div>");
        if !quest.sol_areas.is_empty() {
            html.push_str("<article class=\"sol-guide\"><header><h3>Sol locations</h3></header><div class=\"sol-map-grid\">");
            for area in &quest.sol_areas {
                write_sol_map(&mut html, area, output)?;
            }
            html.push_str("</div>");
            if quest.unmapped_sol_locations != 0 {
                write!(
                    html,
                    "<p class=\"sol-warning\">The script can select {} additional {} outside the playable minimap. You cannot open a Sol when the script selects {}.</p>",
                    quest.unmapped_sol_locations,
                    if quest.unmapped_sol_locations == 1 {
                        "position"
                    } else {
                        "positions"
                    },
                    if quest.unmapped_sol_locations == 1 {
                        "this position"
                    } else {
                        "one of these positions"
                    },
                )?;
            }
            html.push_str("</article>");
        } else {
            html.push_str("<article class=\"sol-guide empty\"><header><h3>Sol locations</h3></header><p>This quest script has no Sol, Silver Sol, or Gold Sol location.</p></article>");
        }
        html.push_str("</section>");
    }
    page_end(&mut html);
    write_page(output, "index.html", &html)
}

fn write_sol_map(
    html: &mut String,
    area: &druaga_utils::site_database::SolArea,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    let map = &area.minimap;
    let width = f32::from(map.width);
    let height = f32::from(map.height);
    let mut marker_bounds = Bounds::empty();
    for location in &area.locations {
        let x = (location.world_x - f32::from(map.origin_x)) / 5.0 + 1.5;
        let y = (location.world_z - f32::from(map.origin_z)) / 5.0 + 1.5;
        let radius = (location.radius / 5.0).max(9.0);
        marker_bounds.include_rect(x - radius, y - radius, x + radius, y + radius);
    }
    let image = fs::read(output.join(&map.image))?;
    let view = rotated_map_bounds(&image, width / 2.0, height / 2.0, marker_bounds)?;
    let label = match &area.floor {
        Some(SolFloor::Single(floor)) => format!("Floor {floor}"),
        Some(SolFloor::Multiple(floors)) => format!(
            "Floors {}",
            floors
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => format!("Area {}", area.area_index + 1),
    };
    write!(
        html,
        "<figure class=\"sol-map\"><svg viewBox=\"{} {} {} {}\" role=\"img\" aria-label=\"{} Sol locations on the game minimap\"><g transform=\"rotate(-45 {} {})\"><image width=\"{}\" height=\"{}\" href=\"{}\"/>",
        view.min_x,
        view.min_y,
        view.width(),
        view.height(),
        label,
        width / 2.0,
        height / 2.0,
        map.width,
        map.height,
        escape(&map.image),
    )?;
    for location in &area.locations {
        let x = (location.world_x - f32::from(map.origin_x)) / 5.0 + 1.5;
        let y = (location.world_z - f32::from(map.origin_z)) / 5.0 + 1.5;
        let exact_radius = location.radius / 5.0;
        let visible_radius = exact_radius.max(9.0);
        write!(
            html,
            "<g class=\"sol-pin {}\"><circle cx=\"{x}\" cy=\"{y}\" r=\"{visible_radius}\"/><circle class=\"exact\" cx=\"{x}\" cy=\"{y}\" r=\"{exact_radius}\"/><title>{}</title></g>",
            sol_kind_class(location.kind),
            sol_kind_name(location.kind),
        )?;
    }
    write!(
        html,
        "</g></svg><figcaption>{} map · {}</figcaption></figure>",
        label,
        area.locations
            .iter()
            .map(|location| sol_kind_name(location.kind))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Bounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Bounds {
    fn empty() -> Self {
        Self {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        }
    }

    fn include(&mut self, x: f32, y: f32) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }

    fn include_rect(&mut self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) {
        self.include(min_x, min_y);
        self.include(max_x, max_y);
    }

    fn is_empty(self) -> bool {
        !self.min_x.is_finite()
    }

    fn expand(mut self, amount: f32) -> Self {
        self.min_x -= amount;
        self.min_y -= amount;
        self.max_x += amount;
        self.max_y += amount;
        self
    }

    fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    fn height(self) -> f32 {
        self.max_y - self.min_y
    }
}

fn crop_map_illustration(illustration: &str) -> Result<String, Box<dyn Error>> {
    if !illustration.contains("class=\"route-map\"") || illustration.contains("panel-map") {
        return Ok(illustration.to_owned());
    }
    let Some(encoded) = illustration
        .split_once("data:image/png;base64,")
        .and_then(|(_, tail)| tail.split_once('"').map(|(data, _)| data))
    else {
        return Ok(illustration.to_owned());
    };
    let image = BASE64.decode(encoded)?;
    let (width, height) = png_dimensions(&image)?;
    let marker_bounds = target_bounds(illustration);
    let view = rotated_map_bounds(
        &image,
        width as f32 / 2.0,
        height as f32 / 2.0,
        marker_bounds,
    )?;
    let Some(start) = illustration.find("viewBox=\"") else {
        return Ok(illustration.to_owned());
    };
    let value_start = start + "viewBox=\"".len();
    let Some(value_end) = illustration[value_start..].find('"') else {
        return Ok(illustration.to_owned());
    };
    let mut result = illustration.to_owned();
    result.replace_range(
        value_start..value_start + value_end,
        &format!(
            "{} {} {} {}",
            view.min_x,
            view.min_y,
            view.width(),
            view.height()
        ),
    );
    Ok(result)
}

fn rotated_map_bounds(
    image: &[u8],
    center_x: f32,
    center_y: f32,
    marker_bounds: Bounds,
) -> Result<Bounds, Box<dyn Error>> {
    let mut bounds = opaque_pixel_bounds(image, center_x, center_y)?;
    if !marker_bounds.is_empty() {
        for (x, y) in [
            (marker_bounds.min_x, marker_bounds.min_y),
            (marker_bounds.max_x, marker_bounds.min_y),
            (marker_bounds.min_x, marker_bounds.max_y),
            (marker_bounds.max_x, marker_bounds.max_y),
        ] {
            let (x, y) = rotate_map_point(x, y, center_x, center_y);
            bounds.include(x, y);
        }
    }
    if bounds.is_empty() {
        return Err("minimap has no visible pixels or markers".into());
    }
    Ok(bounds.expand(6.0))
}

fn rotate_map_point(x: f32, y: f32, center_x: f32, center_y: f32) -> (f32, f32) {
    let factor = std::f32::consts::FRAC_1_SQRT_2;
    let x = x - center_x;
    let y = y - center_y;
    (center_x + factor * (x + y), center_y + factor * (-x + y))
}

fn png_dimensions(image: &[u8]) -> Result<(u32, u32), Box<dyn Error>> {
    let decoder = png::Decoder::new(Cursor::new(image));
    let reader = decoder.read_info()?;
    Ok(reader.info().size())
}

fn opaque_pixel_bounds(
    image: &[u8],
    center_x: f32,
    center_y: f32,
) -> Result<Bounds, Box<dyn Error>> {
    let mut decoder = png::Decoder::new(Cursor::new(image));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;
    let (stride, alpha) = match info.color_type {
        png::ColorType::Rgba => (4, Some(3)),
        png::ColorType::GrayscaleAlpha => (2, Some(1)),
        png::ColorType::Rgb => (3, None),
        png::ColorType::Grayscale => (1, None),
        png::ColorType::Indexed => return Err("indexed minimap did not expand".into()),
    };
    let mut bounds = Bounds::empty();
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            let offset = (y * info.width as usize + x) * stride;
            if alpha.is_none_or(|channel| buffer[offset + channel] != 0) {
                for (x, y) in [
                    (x as f32, y as f32),
                    (x as f32 + 1.0, y as f32),
                    (x as f32, y as f32 + 1.0),
                    (x as f32 + 1.0, y as f32 + 1.0),
                ] {
                    let (x, y) = rotate_map_point(x, y, center_x, center_y);
                    bounds.include(x, y);
                }
            }
        }
    }
    Ok(bounds)
}

fn target_bounds(illustration: &str) -> Bounds {
    let Some(start) = illustration.find("<g class=\"targets\"") else {
        return Bounds::empty();
    };
    let targets = &illustration[start..];
    let end = targets.find("</g></g></svg>").unwrap_or(targets.len());
    let targets = &targets[..end];
    let mut bounds = Bounds::empty();
    for tag_name in ["ellipse", "circle", "rect", "line", "text", "path"] {
        for tag in svg_tags(targets, tag_name) {
            include_svg_tag(&mut bounds, tag_name, tag);
        }
    }
    bounds
}

fn svg_tags<'a>(value: &'a str, name: &str) -> Vec<&'a str> {
    let marker = format!("<{name}");
    let mut tags = Vec::new();
    let mut remainder = value;
    while let Some(start) = remainder.find(&marker) {
        remainder = &remainder[start..];
        let Some(end) = remainder.find('>') else {
            break;
        };
        tags.push(&remainder[..=end]);
        remainder = &remainder[end + 1..];
    }
    tags
}

fn svg_attribute(tag: &str, name: &str) -> Option<f32> {
    let marker = format!("{name}=\"");
    let value = tag.split_once(&marker)?.1.split_once('"')?.0;
    value.parse().ok()
}

fn include_svg_tag(bounds: &mut Bounds, name: &str, tag: &str) {
    match name {
        "ellipse" => {
            if let (Some(cx), Some(cy), Some(rx), Some(ry)) = (
                svg_attribute(tag, "cx"),
                svg_attribute(tag, "cy"),
                svg_attribute(tag, "rx"),
                svg_attribute(tag, "ry"),
            ) {
                bounds.include_rect(cx - rx, cy - ry, cx + rx, cy + ry);
            }
        }
        "circle" => {
            if let (Some(cx), Some(cy), Some(radius)) = (
                svg_attribute(tag, "cx"),
                svg_attribute(tag, "cy"),
                svg_attribute(tag, "r"),
            ) {
                bounds.include_rect(cx - radius, cy - radius, cx + radius, cy + radius);
            }
        }
        "rect" => {
            if let (Some(x), Some(y), Some(width), Some(height)) = (
                svg_attribute(tag, "x"),
                svg_attribute(tag, "y"),
                svg_attribute(tag, "width"),
                svg_attribute(tag, "height"),
            ) {
                bounds.include_rect(x, y, x + width, y + height);
            }
        }
        "line" => {
            if let (Some(x1), Some(y1), Some(x2), Some(y2)) = (
                svg_attribute(tag, "x1"),
                svg_attribute(tag, "y1"),
                svg_attribute(tag, "x2"),
                svg_attribute(tag, "y2"),
            ) {
                bounds.include_rect(x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2));
            }
        }
        "text" => {
            if let (Some(x), Some(y)) = (svg_attribute(tag, "x"), svg_attribute(tag, "y")) {
                bounds.include_rect(x - 10.0, y - 10.0, x + 10.0, y + 10.0);
            }
        }
        "path" => {
            let Some(data) = tag
                .split_once("d=\"")
                .and_then(|(_, value)| value.split_once('"').map(|(data, _)| data))
            else {
                return;
            };
            let values = data
                .split(|character: char| {
                    !(character.is_ascii_digit()
                        || matches!(character, '.' | '-' | '+' | 'e' | 'E'))
                })
                .filter_map(|value| value.parse::<f32>().ok())
                .collect::<Vec<_>>();
            for pair in values.chunks_exact(2) {
                bounds.include(pair[0], pair[1]);
            }
        }
        _ => {}
    }
}

fn sol_kind_name(kind: SolKind) -> &'static str {
    match kind {
        SolKind::Sol => "Sol",
        SolKind::SilverSol => "Silver Sol",
        SolKind::GoldSol => "Gold Sol",
    }
}

fn sol_kind_class(kind: SolKind) -> &'static str {
    match kind {
        SolKind::Sol => "sol",
        SolKind::SilverSol => "silver-sol",
        SolKind::GoldSol => "gold-sol",
    }
}

fn write_item_pages(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    for (character, label, slug) in CHARACTERS {
        let file = format!("items-{slug}.html");
        let mut html = page_start(&format!("{label} equipment"), &file, data);
        write!(html, "<p>{}</p>", escape(WeaponBonus::STATUS_NOTE))?;
        for slot in SLOTS {
            write_item_table(
                &mut html,
                data,
                slot_name(slot),
                slot_id(slot),
                |item| {
                    item.equipment.as_ref().is_some_and(|equipment| {
                        equipment.slot == slot && equipment.characters.contains(&character)
                    })
                },
                true,
            )?;
        }
        page_end(&mut html);
        write_page(output, &file, &html)?;
    }
    for (title, file, category) in category_pages("items") {
        let mut html = page_start(title, &file, data);
        write_item_table(
            &mut html,
            data,
            title,
            "items",
            |item| category_matches(item.category, category),
            category == ItemCategory::Accessory,
        )?;
        page_end(&mut html);
        write_page(output, &file, &html)?;
    }
    Ok(())
}

fn write_item_table(
    html: &mut String,
    data: &SiteData,
    title: &str,
    id: &str,
    predicate: impl Fn(&ItemRecord) -> bool,
    equipment: bool,
) -> Result<(), std::fmt::Error> {
    write!(
        html,
        "<section class=\"item-section\" id=\"{id}\"><h2>{title}</h2><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Item</th>"
    )?;
    if equipment {
        html.push_str(
            "<th>Rank</th><th>Attack</th><th>Defense</th><th>Weight</th><th>Effects</th>",
        );
    } else {
        html.push_str("<th>Description</th>");
    }
    html.push_str("<th>Crafting points</th><th>Sell</th><th>Disassembly</th><th>Obtain from</th></tr></thead><tbody>");
    for item in data.items.items.iter().filter(|item| predicate(item)) {
        write!(html, "<tr id=\"item-{:04x}\"><td>", item.id)?;
        write_item_identity(html, item, equipment)?;
        if let Some(equipment) = item.equipment.as_ref().filter(|_| equipment) {
            write!(
                html,
                "</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td class=\"effects\">{}",
                escape(&equipment.required_rank.label),
                optional_number(equipment.attack),
                optional_number(equipment.defense),
                optional_number(equipment.weight),
                effects_text(equipment)
            )?;
        } else {
            write!(html, "</td><td>{}", formatted_text(&item.description))?;
        }
        write!(
            html,
            "</td><td class=\"number\">{}</td><td class=\"number\">{}</td><td>{}</td><td>{}</td></tr>",
            item.alchemy_rank_points,
            item.sell_value,
            disassembly_link(item, data),
            obtainability(item, data)
        )?;
    }
    html.push_str("</tbody></table></div></section>");
    Ok(())
}

fn write_crafting_pages(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    for (character, label, slug) in CHARACTERS {
        let file = format!("crafting-{slug}.html");
        let mut html = page_start(&format!("{label} crafting"), &file, data);
        for slot in SLOTS {
            write_recipe_table(&mut html, data, slot_name(slot), slot_id(slot), |item| {
                item.equipment.as_ref().is_some_and(|equipment| {
                    equipment.slot == slot && equipment.characters.contains(&character)
                })
            })?;
        }
        page_end(&mut html);
        write_page(output, &file, &html)?;
    }
    for (title, file, category) in category_pages("crafting") {
        let mut html = page_start(title, &file, data);
        write_recipe_table(&mut html, data, title, "recipes", |item| {
            category_matches(item.category, category)
        })?;
        page_end(&mut html);
        write_page(output, &file, &html)?;
    }
    Ok(())
}

fn write_recipe_table(
    html: &mut String,
    data: &SiteData,
    title: &str,
    id: &str,
    predicate: impl Fn(&ItemRecord) -> bool,
) -> Result<(), std::fmt::Error> {
    let items = item_map(data);
    write!(
        html,
        "<section class=\"item-section\" id=\"{id}\"><h2>{title}</h2><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Result</th><th>Ingredients</th><th>Time</th><th>Success</th><th>Obtain from</th></tr></thead><tbody>"
    )?;
    for recipe in &data.alchemy.recipes {
        let result = items[&recipe.result_item_id];
        if !predicate(result) {
            continue;
        }
        write!(html, "<tr id=\"recipe-{}\"><td>", recipe.id)?;
        write_item_link(html, result)?;
        html.push_str("</td><td><ul class=\"compact-list\">");
        for id in &recipe.ingredient_item_ids {
            let item = items[id];
            write!(html, "<li>")?;
            write_item_link(html, item)?;
            html.push_str("</li>");
        }
        write!(
            html,
            "</ul></td><td class=\"number\">{} min</td><td class=\"number\">{}%</td><td>{}</td></tr>",
            recipe.completion_minutes,
            recipe.success_rate_percent,
            obtainability(result, data)
        )?;
    }
    for recipe in &data.alchemy.rule_based_recipes {
        let result = items[&recipe.result_item_id];
        let next_result = recipe.next_result_item_id.map(|id| items[&id]);
        if !predicate(result) && next_result.is_none_or(|item| !predicate(item)) {
            continue;
        }
        write!(
            html,
            "<tr id=\"rule-recipe-{}\"><td><div>Usual result: ",
            recipe.id
        )?;
        write_item_link(html, result)?;
        if let Some(item) = next_result {
            html.push_str("</div><div>Higher result: ");
            write_item_link(html, item)?;
        }
        write!(
            html,
            "</div></td><td><div>{}</div><div>2 items: {}</div><div>3 items: {}</div></td><td class=\"number\">{} min</td><td>{}</td><td>{}</td></tr>",
            escape(&rule_ingredient_description(recipe)),
            rule_point_range(recipe.point_level, 2),
            rule_point_range(recipe.point_level, 3),
            recipe.completion_minutes,
            if recipe.next_result_item_id.is_some() {
                "Two possible results"
            } else {
                "Fixed result"
            },
            obtainability(result, data)
        )?;
    }
    html.push_str("</tbody></table></div></section>");
    Ok(())
}

fn rule_ingredient_description(recipe: &RuleBasedAlchemyRecipe) -> String {
    let category = match recipe.ingredient_category {
        AlchemyIngredientCategory::Other => "other items",
        AlchemyIngredientCategory::MaterialOrTool => "materials or tools",
        AlchemyIngredientCategory::Accessory => "accessories",
        AlchemyIngredientCategory::Weapon => "weapons",
        AlchemyIngredientCategory::OffHand => "off-hand equipment",
        AlchemyIngredientCategory::Head => "head equipment",
        AlchemyIngredientCategory::Body => "body equipment",
        AlchemyIngredientCategory::Arms => "arm equipment",
        AlchemyIngredientCategory::Feet => "foot equipment",
    };
    match recipe.character {
        Some(character) => format!(
            "Use {} {category}",
            druaga_utils::site_database::character_name(character)
        ),
        None => format!("Use {category}"),
    }
}

fn rule_point_range(point_level: u8, ingredient_count: u8) -> String {
    let level = u16::from(point_level);
    match (point_level, ingredient_count) {
        (2, 2) => "0–2 total points".to_owned(),
        (2, 3) => "0 total points".to_owned(),
        (32, 2) => "61 or more total points".to_owned(),
        (32, 3) => "88 or more total points".to_owned(),
        (_, 2) => format!("{}–{} total points", level * 2 - 3, level * 2 - 2),
        (_, 3) => format!("{}–{} total points", level * 3 - 8, level * 3 - 6),
        _ => unreachable!(),
    }
}

fn write_enemy_page(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    let items = item_map(data);
    let quests: HashMap<_, _> = data
        .chests
        .quests
        .iter()
        .map(|quest| (quest.id, quest))
        .collect();
    let mut html = page_start("Enemy database", "enemies.html", data);
    html.push_str("<section class=\"item-section\"><div class=\"table-scroll\"><table class=\"item-table enemy-table\"><thead><tr><th>Enemy</th><th>Quests</th><th>Drops</th></tr></thead><tbody>");
    for enemy in &data.enemies.enemies {
        write!(
            html,
            "<tr id=\"enemy-{:04x}\"><td><strong>{}</strong><code>0x{:04X}</code></td><td>",
            enemy.definition_id,
            escape(&enemy.name),
            enemy.definition_id
        )?;
        if enemy.quest_ids.is_empty() {
            html.push('—');
        } else {
            for (index, id) in enemy.quest_ids.iter().enumerate() {
                if index != 0 {
                    html.push_str("<br>");
                }
                let quest = quests[id];
                write!(
                    html,
                    "<a href=\"index.html#quest-{id}\">{}</a>",
                    escape(&quest.name)
                )?;
            }
        }
        html.push_str("</td><td>");
        if enemy.drops.is_empty() || enemy.base_drop_rate_percent == 0 {
            html.push('—');
        } else {
            html.push_str("<ul class=\"compact-list\">");
            for drop in &enemy.drops {
                let item = items[&drop.item_id];
                write!(
                    html,
                    "<li><a href=\"{}#item-{:04x}\">{}</a> — {}%</li>",
                    item_page(item),
                    item.id,
                    escape(&item.name),
                    drop_percent(enemy, drop)
                )?;
            }
            html.push_str("</ul>");
        }
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table></div></section>");
    page_end(&mut html);
    write_page(output, "enemies.html", &html)
}

fn write_quest_source_page(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    let items = item_map(data);
    let mut html = page_start("Quest item sources", "quest-sources.html", data);
    html.push_str("<section class=\"item-section\"><h2>Scripted item sources</h2><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Quest</th><th>Recipient</th><th>Item</th><th>How to obtain it</th><th>Repeatability</th></tr></thead><tbody>");
    for quest in &data.quest_sources.quests {
        for (index, reward) in quest.rewards.iter().enumerate() {
            let item = items[&reward.item_id];
            write!(
                html,
                "<tr id=\"{}\"><td>{}<br><small>{}</small></td><td>{}</td><td>",
                quest_reward_anchor(quest, index),
                escape(&quest.name),
                quest_source_number(quest),
                quest_source_recipient(quest)
            )?;
            write_item_link(&mut html, item)?;
            write!(html, "</td><td>{}", escape(&reward.acquisition))?;
            write_item_dependencies(&mut html, reward, &items)?;
            write!(html, "</td><td>{}</td></tr>", escape(&reward.repeatability))?;
        }
    }
    html.push_str("</tbody></table></div></section>");
    html.push_str("<section class=\"item-section\"><h2>Direct reward pools</h2><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Quest</th><th>How to obtain the reward</th><th>Possible items</th><th>Repeatability</th></tr></thead><tbody>");
    let direct_pools = quest_direct_reward_pool_map(data);
    for quest in &data.quest_sources.quests {
        for (source_index, source) in quest.direct_reward_sources.iter().enumerate() {
            let pool = direct_pools[source.pool_id.as_str()];
            write!(
                html,
                "<tr id=\"{}\"><td>{}<br><small>{}</small></td><td>{}</td><td><ul class=\"compact-list\">",
                quest_direct_reward_anchor(quest, source_index),
                escape(&quest.name),
                quest_source_number(quest),
                escape(&source.acquisition)
            )?;
            for reward in &pool.rewards {
                let item = items[&reward.item_id];
                html.push_str("<li>");
                write_item_link(&mut html, item)?;
                write!(html, " — {}", quest_pool_reward_condition(reward))?;
                html.push_str("</li>");
            }
            write!(
                html,
                "</ul></td><td>{}</td></tr>",
                escape(&source.repeatability)
            )?;
        }
    }
    html.push_str("</tbody></table></div></section>");
    html.push_str("<section class=\"item-section\"><h2>Quest treasure boxes</h2><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Quest</th><th>How to find the box</th><th>Contents</th><th>Repeatability</th></tr></thead><tbody>");
    let pools = quest_treasure_pool_map(data);
    for quest in &data.quest_sources.quests {
        for (source_index, source) in quest.treasure_sources.iter().enumerate() {
            let pool = pools[source.pool_id.as_str()];
            write!(
                html,
                "<tr id=\"{}\"><td>{}<br><small>{}</small></td><td>{}",
                quest_treasure_anchor(quest, source_index),
                escape(&quest.name),
                quest_source_number(quest),
                escape(&source.acquisition)
            )?;
            html.push_str("</td><td><ul class=\"compact-list\">");
            for reward in &pool.rewards {
                let item = items[&reward.item_id];
                html.push_str("<li>");
                write_item_link(&mut html, item)?;
                write!(html, " — {}", quest_pool_reward_condition(reward))?;
                html.push_str("</li>");
            }
            if let Some(money) = &pool.money {
                write!(
                    html,
                    "<li>{} through {} Gold</li>",
                    money.minimum, money.maximum
                )?;
            }
            write!(
                html,
                "</ul></td><td>{}</td></tr>",
                escape(&source.repeatability)
            )?;
        }
    }
    html.push_str("</tbody></table></div></section>");
    page_end(&mut html);
    write_page(output, "quest-sources.html", &html)
}

fn write_tower_source_page(data: &SiteData, output: &Path) -> Result<(), Box<dyn Error>> {
    let items = item_map(data);
    let mut html = page_start("Tower item sources", "tower-sources.html", data);
    html.push_str("<section class=\"item-section\"><h2>Tower presents and lottery</h2><p>The Tower checks present conditions when it reads a character card. A claimed present cannot be claimed again with the same character card.</p>");
    for source in &data.tower_sources.sources {
        write!(
            html,
            "<article class=\"tower-source\" id=\"tower-{}\"><header><h3>{}</h3></header><p>{}</p><p><strong>Repeatability:</strong> {}</p><div class=\"table-scroll\"><table class=\"item-table\"><thead><tr><th>Character</th><th>Item</th><th>Chance</th></tr></thead><tbody>",
            escape(&source.id),
            escape(&source.name),
            escape(&source.acquisition),
            escape(&source.repeatability),
        )?;
        for reward in &source.rewards {
            write!(
                html,
                "<tr><td>{}</td><td>",
                reward
                    .character
                    .map(character_name)
                    .unwrap_or("All characters")
            )?;
            write_item_link(&mut html, items[&reward.item_id])?;
            write!(
                html,
                "</td><td>{}</td></tr>",
                source_chance(reward.chance_numerator, reward.chance_denominator)
            )?;
        }
        html.push_str("</tbody></table></div></article>");
    }
    html.push_str("</section>");
    page_end(&mut html);
    write_page(output, "tower-sources.html", &html)
}

fn source_chance(numerator: Option<u16>, denominator: Option<u16>) -> String {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator)) => {
            format!("{}%", f32::from(numerator) * 100.0 / f32::from(denominator))
        }
        _ => "Guaranteed".to_owned(),
    }
}

fn write_item_dependencies(
    html: &mut String,
    source: &druaga_utils::site_database::ScriptedItemSource,
    items: &HashMap<u16, &ItemRecord>,
) -> Result<(), std::fmt::Error> {
    for (label, item_ids) in [
        ("Requires", &source.required_item_ids),
        ("Consumes", &source.consumed_item_ids),
    ] {
        if item_ids.is_empty() {
            continue;
        }
        write!(html, "<br><small>{label}: ")?;
        for (index, item_id) in item_ids.iter().enumerate() {
            if index != 0 {
                html.push_str(", ");
            }
            write_item_link(html, items[item_id])?;
        }
        html.push_str("</small>");
    }
    Ok(())
}

fn quest_source_number(quest: &QuestSourceQuest) -> String {
    match quest.identity {
        QuestSourceIdentity::Solo {
            chapter, section, ..
        } => format!("Chapter {chapter}, section {section}"),
        QuestSourceIdentity::Party { chapter, section } => {
            format!("Party chapter {chapter}, section {section}")
        }
        QuestSourceIdentity::Scheduled { network_id, .. } => {
            format!("Network quest 0x{network_id:04X}")
        }
        QuestSourceIdentity::ScheduledPartyClear => "Shared completion reward".to_owned(),
    }
}

fn quest_source_recipient(quest: &QuestSourceQuest) -> &'static str {
    match quest.identity {
        QuestSourceIdentity::Solo { character, .. } => {
            druaga_utils::site_database::character_name(character)
        }
        QuestSourceIdentity::Party { .. }
        | QuestSourceIdentity::Scheduled { .. }
        | QuestSourceIdentity::ScheduledPartyClear => "All characters",
    }
}

fn quest_reward_anchor(quest: &QuestSourceQuest, reward_index: usize) -> String {
    match quest.identity {
        QuestSourceIdentity::Solo {
            chapter,
            section,
            character,
        } => format!(
            "story-solo-{chapter}-{section}-{}-{reward_index}",
            character_slug(character)
        ),
        QuestSourceIdentity::Party { chapter, section } => {
            format!("story-party-{chapter}-{section}-{reward_index}")
        }
        QuestSourceIdentity::Scheduled { guide_quest_id, .. } => {
            format!("quest-{guide_quest_id}-reward-{reward_index}")
        }
        QuestSourceIdentity::ScheduledPartyClear => {
            format!("scheduled-party-clear-{reward_index}")
        }
    }
}

fn quest_treasure_anchor(quest: &QuestSourceQuest, source_index: usize) -> String {
    format!(
        "{}-treasure-{source_index}",
        quest_reward_anchor(quest, 0)
            .strip_suffix("-0")
            .unwrap_or("story")
    )
}

fn quest_direct_reward_anchor(quest: &QuestSourceQuest, source_index: usize) -> String {
    format!(
        "{}-direct-{source_index}",
        quest_reward_anchor(quest, 0)
            .strip_suffix("-0")
            .unwrap_or("quest")
    )
}

fn quest_treasure_pool_map(
    data: &SiteData,
) -> HashMap<&str, &druaga_utils::site_database::QuestTreasurePool> {
    data.quest_sources
        .treasure_pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect()
}

fn quest_direct_reward_pool_map(
    data: &SiteData,
) -> HashMap<&str, &druaga_utils::site_database::QuestDirectRewardPool> {
    data.quest_sources
        .direct_reward_pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool))
        .collect()
}

fn quest_pool_reward_condition(reward: &druaga_utils::site_database::QuestPoolReward) -> String {
    match (reward.chance_numerator, reward.chance_denominator) {
        (Some(numerator), Some(denominator)) => format!(
            "{numerator}/{denominator} ({:.2}%)",
            f64::from(numerator) * 100.0 / f64::from(denominator)
        ),
        _ => reward
            .selection_condition
            .clone()
            .unwrap_or_else(|| "Possible reward".to_owned()),
    }
}

fn character_slug(character: Character) -> &'static str {
    match character {
        Character::Gilgamesh => "gilgamesh",
        Character::Valkyrie => "walkure",
        Character::YoungKi => "young-ki",
        Character::Xeovalga => "xeovalga",
    }
}

fn obtainability(item: &ItemRecord, data: &SiteData) -> String {
    let mut sources = Vec::new();
    let quest_pools = quest_treasure_pool_map(data);
    let direct_pools = quest_direct_reward_pool_map(data);
    if item.purchase_value != 0 {
        sources.push(format!("Shop — {} Gold", item.purchase_value));
    }
    if let Some(rate) = item.server_present_rate_percent {
        sources.push(format!("AON.Net server present — {rate}%"));
    }
    for quest in &data.chests.quests {
        for chest in &quest.chests {
            if chest.rewards.iter().any(|reward| matches!(reward.value, ChestRewardValue::Item { item_id, .. } if item_id == item.id)) {
                sources.push(format!("<a href=\"index.html#quest-{}\">{} {} chest</a>", quest.id, escape(&quest.name), chest_tier_name(chest.tier)));
            }
        }
    }
    for quest in &data.quest_sources.quests {
        for (index, reward) in quest.rewards.iter().enumerate() {
            if reward.item_id == item.id {
                sources.push(format!(
                    "<a href=\"quest-sources.html#{}\">{}: {}</a>",
                    quest_reward_anchor(quest, index),
                    escape(&quest.name),
                    escape(&reward.acquisition)
                ));
            }
        }
        for (source_index, source) in quest.treasure_sources.iter().enumerate() {
            let pool = quest_pools[source.pool_id.as_str()];
            for reward in pool
                .rewards
                .iter()
                .filter(|reward| reward.item_id == item.id)
            {
                sources.push(format!(
                    "<a href=\"quest-sources.html#{}\">{} treasure box: {}</a>",
                    quest_treasure_anchor(quest, source_index),
                    escape(&quest.name),
                    quest_pool_reward_condition(reward)
                ));
            }
        }
        for (source_index, source) in quest.direct_reward_sources.iter().enumerate() {
            let pool = direct_pools[source.pool_id.as_str()];
            for reward in pool
                .rewards
                .iter()
                .filter(|reward| reward.item_id == item.id)
            {
                sources.push(format!(
                    "<a href=\"quest-sources.html#{}\">{}: {}</a>",
                    quest_direct_reward_anchor(quest, source_index),
                    escape(&quest.name),
                    quest_pool_reward_condition(reward)
                ));
            }
        }
    }
    for source in &data.tower_sources.sources {
        for reward in source
            .rewards
            .iter()
            .filter(|reward| reward.item_id == item.id)
        {
            let character = reward
                .character
                .map(|character| format!(" for {}", character_name(character)))
                .unwrap_or_default();
            sources.push(format!(
                "<a href=\"tower-sources.html#tower-{}\">{}{} — {}</a>",
                escape(&source.id),
                escape(&source.name),
                character,
                source_chance(reward.chance_numerator, reward.chance_denominator)
            ));
        }
    }
    for recipe in &data.alchemy.recipes {
        if recipe.result_item_id == item.id {
            sources.push(format!(
                "<a href=\"{}#recipe-{}\">Alchemy recipe {}</a>",
                crafting_page(item),
                recipe.id,
                recipe.id
            ));
        }
    }
    let mut rule_sources = Vec::new();
    for recipe in &data.alchemy.rule_based_recipes {
        if recipe.result_item_id != item.id && recipe.next_result_item_id != Some(item.id) {
            continue;
        }
        let description = rule_ingredient_description(recipe);
        if rule_sources
            .iter()
            .any(|(existing, _)| existing == &description)
        {
            continue;
        }
        rule_sources.push((description, recipe.id));
    }
    for (description, recipe_id) in rule_sources {
        sources.push(format!(
            "<a href=\"{}#rule-recipe-{}\">Generic crafting — {}</a>",
            crafting_page(item),
            recipe_id,
            escape(&description)
        ));
    }
    for enemy in &data.enemies.enemies {
        for drop in enemy.drops.iter().filter(|drop| drop.item_id == item.id) {
            sources.push(format!(
                "<a href=\"enemies.html#enemy-{:04x}\">{} — {}%</a>",
                enemy.definition_id,
                escape(&enemy.name),
                drop_percent(enemy, drop)
            ));
        }
    }
    for source in &data.items.items {
        if source.disassembles_to_item_id == Some(item.id) {
            sources.push(format!(
                "Disassemble <a href=\"{}#item-{:04x}\">{}</a>",
                item_page(source),
                source.id,
                escape(&source.name)
            ));
        }
    }
    if sources.is_empty() {
        return "No quest, Tower, alchemy, shop, disassembly, or monster source in the Version 1.60 data."
            .to_owned();
    }
    render_sources(&sources)
}

fn render_sources(sources: &[String]) -> String {
    let lines = format!(
        "<div class=\"source-list\"><div>{}</div></div>",
        sources.join("</div><div>")
    );
    if sources.len() <= 3 {
        return lines;
    }
    format!(
        "<details class=\"obtain\"><summary>{} sources</summary>{lines}</details>",
        sources.len()
    )
}

fn drop_percent(enemy: &EnemyRecord, drop: &EnemyDrop) -> String {
    if enemy.base_drop_rate_percent == 0
        || enemy.item_selection_count == 0
        || enemy.total_item_weight == 0
    {
        return "0".to_owned();
    }
    let miss = 1.0 - f64::from(drop.weight) / f64::from(enemy.total_item_weight);
    let selected = 1.0 - miss.powi(i32::from(enemy.item_selection_count));
    trim_decimal(f64::from(enemy.base_drop_rate_percent) * selected)
}

fn trim_decimal(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn page_start(title: &str, current: &str, data: &SiteData) -> String {
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{} · The Tower of Druaga</title><link rel=\"stylesheet\" href=\"site.css\"></head><body><button class=\"menu\" type=\"button\">Site index</button><nav class=\"sidebar\"><div class=\"brand\"><b>THE TOWER OF DRUAGA</b><small>AON.Net · Version 1.60</small></div>",
        escape(title)
    );
    html.push_str("<div class=\"nav-heading\">Hidden chest guide</div>");
    for category in [
        QuestCategory::Original,
        QuestCategory::Advanced,
        QuestCategory::Special,
        QuestCategory::Random,
    ] {
        infallible(write!(
            html,
            "<details><summary>{}</summary>",
            quest_category_name(category)
        ));
        for quest in data
            .chests
            .quests
            .iter()
            .filter(|quest| quest.category == category)
        {
            infallible(write!(
                html,
                "<a href=\"index.html#quest-{}\">{}</a>",
                quest.id,
                escape(&quest.name)
            ));
        }
        html.push_str("</details>");
    }
    write_nav_group(&mut html, "Item database", current, &item_page_links());
    write_nav_group(&mut html, "Crafting", current, &crafting_page_links());
    html.push_str("<div class=\"nav-heading\">Enemy database</div>");
    infallible(write!(
        html,
        "<a href=\"enemies.html\"{}>Enemy and drop index</a>",
        aria_current(current, "enemies.html")
    ));
    html.push_str("<div class=\"nav-heading\">Quest item sources</div>");
    infallible(write!(
        html,
        "<a href=\"quest-sources.html\"{}>Scripted item sources</a>",
        aria_current(current, "quest-sources.html")
    ));
    html.push_str("<div class=\"nav-heading\">Tower item sources</div>");
    infallible(write!(
        html,
        "<a href=\"tower-sources.html\"{}>Presents and lottery</a>",
        aria_current(current, "tower-sources.html")
    ));
    html.push_str("</nav><main>");
    if current != "index.html" {
        infallible(write!(
            html,
            "<header class=\"page-title\"><h1>{}</h1></header>",
            escape(title)
        ));
    }
    html
}

fn write_nav_group(
    html: &mut String,
    heading: &str,
    current: &str,
    links: &[(String, &'static str)],
) {
    infallible(write!(html, "<div class=\"nav-heading\">{heading}</div>"));
    for (file, label) in links {
        let slug = file
            .split_once('-')
            .map(|(_, slug)| slug)
            .and_then(|value| value.strip_suffix(".html"));
        if slug
            .filter(|slug| CHARACTERS.iter().any(|(_, _, value)| value == slug))
            .is_some()
        {
            infallible(write!(
                html,
                "<details{}><summary><a href=\"{file}\"{}>{label}</a></summary>",
                if current == file { " open" } else { "" },
                aria_current(current, file)
            ));
            for slot in SLOTS {
                infallible(write!(
                    html,
                    "<a href=\"{file}#{}\">{}</a>",
                    slot_id(slot),
                    slot_name(slot)
                ));
            }
            html.push_str("</details>");
        } else {
            infallible(write!(
                html,
                "<a href=\"{file}\"{}>{label}</a>",
                aria_current(current, file)
            ));
        }
    }
}

fn infallible(result: std::fmt::Result) {
    assert!(result.is_ok(), "writing to a String failed");
}

fn page_end(html: &mut String) {
    html.push_str("</main><script>const menu=document.querySelector('.menu'),sidebar=document.querySelector('.sidebar');menu.addEventListener('click',()=>sidebar.classList.toggle('open'));");
    html.push_str("</script></body></html>");
}

fn write_page(output: &Path, file: &str, html: &str) -> Result<(), Box<dyn Error>> {
    atomic_output::write_bytes(&output.join(file), html.as_bytes())?;
    Ok(())
}

fn item_map(data: &SiteData) -> HashMap<u16, &ItemRecord> {
    data.items
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect()
}

fn write_item_identity(
    html: &mut String,
    item: &ItemRecord,
    description: bool,
) -> Result<(), std::fmt::Error> {
    write!(
        html,
        "<div class=\"item-name\"><span class=\"item-icon\" role=\"img\" aria-label=\"{} icon\" style=\"background-image:url('item-icons/{}.avif');background-position:-{}px -{}px\"></span><span><strong>{}</strong><code>0x{:04X}</code>",
        escape(&item.name),
        escape(&item.icon.sheet),
        u16::from(item.icon.column) * ICON_PITCH,
        u16::from(item.icon.row) * ICON_PITCH,
        escape(&item.name),
        item.id
    )?;
    if description {
        write!(html, "<small>{}</small>", formatted_text(&item.description))?;
    }
    html.push_str("</span></div>");
    Ok(())
}

fn write_item_link(html: &mut String, item: &ItemRecord) -> Result<(), std::fmt::Error> {
    write!(
        html,
        "<a href=\"{}#item-{:04x}\">{}</a>",
        item_page(item),
        item.id,
        escape(&item.name)
    )
}

fn disassembly_link(item: &ItemRecord, data: &SiteData) -> String {
    let Some(id) = item.disassembles_to_item_id else {
        return "—".to_owned();
    };
    let Some(result) = data.items.items.iter().find(|item| item.id == id) else {
        return format!("0x{id:04X}");
    };
    format!(
        "<a href=\"{}#item-{id:04x}\">{}</a>",
        item_page(result),
        escape(&result.name)
    )
}

fn item_page(item: &ItemRecord) -> &'static str {
    match item.category {
        ItemCategory::Quest => "items-quest.html",
        ItemCategory::Consumable => "items-consumables.html",
        ItemCategory::Accessory => "items-accessories.html",
        ItemCategory::Equipment => match item
            .equipment
            .as_ref()
            .and_then(|value| value.characters.first())
        {
            Some(Character::Gilgamesh) => "items-gilgamesh.html",
            Some(Character::Valkyrie) => "items-walkure.html",
            Some(Character::YoungKi) => "items-young-ki.html",
            Some(Character::Xeovalga) => "items-xeovalga.html",
            None => "items-other.html",
        },
        _ => "items-other.html",
    }
}

fn crafting_page(item: &ItemRecord) -> &'static str {
    match item.category {
        ItemCategory::Quest => "crafting-quest.html",
        ItemCategory::Consumable => "crafting-consumables.html",
        ItemCategory::Accessory => "crafting-accessories.html",
        ItemCategory::Equipment => match item
            .equipment
            .as_ref()
            .and_then(|value| value.characters.first())
        {
            Some(Character::Gilgamesh) => "crafting-gilgamesh.html",
            Some(Character::Valkyrie) => "crafting-walkure.html",
            Some(Character::YoungKi) => "crafting-young-ki.html",
            Some(Character::Xeovalga) => "crafting-xeovalga.html",
            None => "crafting-other.html",
        },
        _ => "crafting-other.html",
    }
}

fn item_page_links() -> Vec<(String, &'static str)> {
    category_links("items")
}
fn crafting_page_links() -> Vec<(String, &'static str)> {
    category_links("crafting")
}
fn category_links(prefix: &str) -> Vec<(String, &'static str)> {
    let mut links: Vec<_> = CHARACTERS
        .iter()
        .map(|(_, label, slug)| (format!("{prefix}-{slug}.html"), *label))
        .collect();
    links.extend(
        [
            ("accessories", "Accessories"),
            ("quest", "Quest items"),
            ("consumables", "Consumables"),
            ("other", "Other items"),
        ]
        .map(|(slug, label)| (format!("{prefix}-{slug}.html"), label)),
    );
    links
}

fn category_pages(prefix: &str) -> [(&'static str, String, ItemCategory); 4] {
    [
        (
            "Accessories",
            format!("{prefix}-accessories.html"),
            ItemCategory::Accessory,
        ),
        (
            "Quest items",
            format!("{prefix}-quest.html"),
            ItemCategory::Quest,
        ),
        (
            "Consumables",
            format!("{prefix}-consumables.html"),
            ItemCategory::Consumable,
        ),
        (
            "Other items",
            format!("{prefix}-other.html"),
            ItemCategory::Other,
        ),
    ]
}

fn category_matches(actual: ItemCategory, wanted: ItemCategory) -> bool {
    if wanted == ItemCategory::Other {
        matches!(actual, ItemCategory::MaterialOrTool | ItemCategory::Other)
    } else {
        actual == wanted
    }
}

fn quest_category_name(value: QuestCategory) -> &'static str {
    match value {
        QuestCategory::Original => "Original",
        QuestCategory::Advanced => "Advanced",
        QuestCategory::Special => "Special",
        QuestCategory::Random => "Random",
    }
}
fn chest_tier_name(value: ChestTier) -> &'static str {
    match value {
        ChestTier::Blue => "Blue",
        ChestTier::Red => "Red",
        ChestTier::Silver => "Silver",
        ChestTier::Gold => "Gold",
    }
}
fn chest_tier_class(value: ChestTier) -> &'static str {
    match value {
        ChestTier::Blue => "blue",
        ChestTier::Red => "red",
        ChestTier::Silver => "silver",
        ChestTier::Gold => "gold",
    }
}
fn slot_name(value: EquipmentSlot) -> &'static str {
    match value {
        EquipmentSlot::Weapon => "Weapon",
        EquipmentSlot::OffHand => "Off hand",
        EquipmentSlot::Head => "Head",
        EquipmentSlot::Body => "Body",
        EquipmentSlot::Arms => "Arms",
        EquipmentSlot::Feet => "Feet",
        EquipmentSlot::Accessory => "Accessory",
    }
}
fn slot_id(value: EquipmentSlot) -> &'static str {
    match value {
        EquipmentSlot::Weapon => "weapon",
        EquipmentSlot::OffHand => "off-hand",
        EquipmentSlot::Head => "head",
        EquipmentSlot::Body => "body",
        EquipmentSlot::Arms => "arms",
        EquipmentSlot::Feet => "feet",
        EquipmentSlot::Accessory => "accessory",
    }
}
fn aria_current(current: &str, file: &str) -> &'static str {
    if current == file {
        " aria-current=\"page\""
    } else {
        ""
    }
}
fn optional_number(value: Option<i16>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| value.to_string())
}

fn effects_text(equipment: &Equipment) -> String {
    if equipment.effects.is_empty() && equipment.weapon_bonuses.is_empty() {
        return "—".to_owned();
    }
    equipment
        .weapon_bonuses
        .iter()
        .map(|bonus| bonus.description(&equipment.weapon_bonuses).to_owned())
        .chain(equipment.effects.iter().map(effect_text))
        .map(|value| escape(&value))
        .collect::<Vec<_>>()
        .join("<br>")
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

fn formatted_text(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            escape(
                line.strip_prefix("$$01")
                    .or_else(|| line.strip_prefix("$$02"))
                    .unwrap_or(line),
            )
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

const SITE_STYLE: &str = r#"
:root{--ink:#2a2118;--paper:#e8d8af;--paper2:#f5ebd0;--gold:#ae7b26;--gold2:#d6b45f;--blue:#344f67;--olive:#6d7134;--shadow:#17130f55;--line:#967846}*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;color:var(--ink);background:#1d2424 radial-gradient(circle at 50% -20%,#4a5960 0,#1d2424 48rem);font:15px/1.5 Georgia,"Times New Roman",serif}a{color:#284e68}code{margin-left:8px;color:#766b57;font:11px/1.3 ui-monospace,SFMono-Regular,Consolas,monospace}.sidebar{position:fixed;inset:0 auto 0 0;width:292px;padding:20px 15px;overflow:auto;color:#f4e9ca;background:linear-gradient(180deg,#2f4a61,#1c2b36);border-right:3px solid var(--gold);box-shadow:6px 0 24px #0008;z-index:5}.brand{margin:0 0 12px;padding:0 8px 17px;border-bottom:1px solid #d6b45f66}.brand b{display:block;color:#f0cd76;font-size:20px;letter-spacing:.04em}.brand small{color:#d8d8c8}.nav-heading{margin-top:15px;padding:7px 8px 4px;color:#f0cd76;font-weight:bold;text-transform:uppercase;letter-spacing:.08em;border-top:1px solid #d6b45f44}.search{width:100%;margin:8px 0;padding:8px;color:white;background:#0d192199;border:1px solid #d6b45f88}.sidebar details{margin:4px 0}.sidebar summary{color:#f0cd76;cursor:pointer;font-weight:bold}.sidebar a{display:block;padding:5px 9px;color:#eee6cf;text-decoration:none;border-left:2px solid transparent}.sidebar details>a{padding-left:24px;font-size:13px}.sidebar summary>a{display:inline;padding:0;color:#f0cd76}.sidebar a:hover,.sidebar a[aria-current=page]{color:white;background:#ffffff12;border-left-color:#f0cd76}.menu{display:none}main{margin-left:292px;padding:36px clamp(18px,4vw,64px) 80px}.page-title,.hero,.intro,.quest,.item-section,.page-grid{max-width:1500px;margin:0 auto 24px;background:linear-gradient(135deg,var(--paper2),var(--paper));border:1px solid #e9ce83;border-radius:5px;box-shadow:0 12px 35px #0007}.page-title,.hero{padding:25px 30px;border-top:8px solid var(--gold)}.page-title h1,.hero h1{margin:0;color:var(--blue);font-size:clamp(28px,4vw,44px)}.hero{max-width:1240px}.intro{padding:20px 28px}.intro p{margin:0}.eyebrow{color:#6c593a;font-size:12px;font-weight:bold;letter-spacing:.09em;text-transform:uppercase}.page-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px;padding:28px}.page-card{display:flex;justify-content:space-between;padding:18px;color:var(--ink);background:#fff8e2aa;border:1px solid #b99a5d;text-decoration:none}.page-card strong{color:var(--blue)}.quest{max-width:1240px;padding:28px 30px;scroll-margin-top:18px}.quest-title{display:flex;justify-content:space-between;gap:18px;align-items:flex-start;border-bottom:2px solid var(--line)}.quest-title h2{margin:2px 0 12px;color:var(--blue);font-size:29px}.chest-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px;margin-top:16px}.chest{padding:17px;background:#f9f0d8;border:1px solid var(--line);border-top:6px solid;border-radius:3px;box-shadow:0 4px 10px var(--shadow)}.chest.blue{border-top-color:#3181b4}.chest.red{border-top-color:#a53228}.chest.silver{border-top-color:#89939c}.chest.gold{border-top-color:#c69225}.chest h3{margin:0;font-size:22px}.chest h4{margin:14px 0 2px;color:#66522e;font-size:12px;text-transform:uppercase;letter-spacing:.1em}.chest p{margin:3px 0}.rewards{margin:4px 0}.rewards div{display:grid;grid-template-columns:100px 1fr;border-top:1px dotted #ad9567}.rewards dt{color:#65583f}.rewards dd{margin:0}.variants table{width:100%;border-collapse:collapse}.variants th,.variants td{padding:4px 6px;text-align:left;vertical-align:top;border-top:1px dotted #ad9567}.route-map{margin:12px 0}.route-map svg{display:block;width:100%;max-height:230px;background:#28383d;border:2px solid #8d6b34}.route-map svg>rect{fill:#26363b}.route-map .grid{fill:none;stroke:#e8d8af18;stroke-width:1}.route-map .landmarks{fill:#d9c89755}.route-map .direction-panel{fill:#18212e;stroke:#a68a55;stroke-width:3}.route-map .player-marker{fill:#d6b45f;stroke:#fff0bd;stroke-width:3}.route-map .direction-label,.route-map .clock-label{fill:#fff0bd;font:bold 16px sans-serif;text-anchor:middle}.route-map .compass-arrow{stroke:#ff665d;stroke-width:12}.route-map .compass-head{fill:#ff665d}.route-map .targets .stand-point{fill:#d52d3a;stroke:#ffd3a5;stroke-width:1.5}.route-map .targets .exact-region{fill:#d52d3a80;stroke:#ff7b70;stroke-width:1.5}.route-map .targets .locator-ring{fill:none;stroke:#ffd3a5;stroke-width:1.5;stroke-dasharray:3 2}.route-map .targets .target-leader{stroke:#ffd3a5;stroke-width:1.2}.route-map .targets .direction-arrow{stroke:#ff665d;stroke-width:5;fill:none}.route-map marker path{fill:#ff665d}.route-map .targets text{fill:white;font:bold 12px sans-serif;text-anchor:middle}.route-map figcaption{color:#65583f;font-size:12px}.item-section{padding:22px;scroll-margin-top:12px}.item-section>h2{margin:0 0 14px;color:var(--blue);border-bottom:2px solid var(--line)}.table-scroll{overflow-x:auto}.item-table{width:100%;border-collapse:collapse;background:#fff8e2aa}.item-table th{position:sticky;top:0;padding:8px 9px;color:#f5e9c9;background:#2f4a61;text-align:left;white-space:nowrap}.item-table td{padding:7px 9px;vertical-align:top;border-bottom:1px solid #c6ae78}.item-table tbody tr:hover{background:#fffdf4}.item-table .number{text-align:right;white-space:nowrap}.item-table .effects{min-width:190px}.item-name{display:grid;grid-template-columns:42px minmax(230px,1fr);gap:8px}.item-icon{display:block;width:34px;height:34px}.item-name strong{display:block;color:var(--blue);font-size:16px}.item-name small{display:block;max-width:520px;color:#544b3d;font-size:12px}.compact-list{margin:0;padding-left:18px}.obtain summary{cursor:pointer;white-space:nowrap}.enemy-table td:nth-child(3){min-width:330px}.hidden{display:none}@media(max-width:920px){.sidebar{transform:translateX(-100%);transition:.2s}.sidebar.open{transform:none}.menu{display:block;position:fixed;top:10px;left:10px;z-index:8;padding:9px 12px;color:white;background:var(--blue);border:1px solid var(--gold2)}main{margin-left:0;padding:58px 10px 50px}.page-title,.hero,.intro,.quest,.item-section,.page-grid{padding:18px 14px}.page-grid,.chest-grid{grid-template-columns:1fr}}@media print{body{background:white}.sidebar,.menu{display:none}main{margin:0;padding:0}.page-title,.hero,.intro,.quest,.item-section,.page-grid{box-shadow:none}.item-table th{position:static}.item-table tr{break-inside:avoid}}
.route-map .targets ellipse:not(.exact-region),.route-map .targets rect:not(.exact-region){fill:#d52d3a;stroke:#ffd3a5;stroke-width:1.5}
.sol-guide{margin-top:24px;padding:22px;background:var(--paper2);border:2px solid var(--line);box-shadow:0 8px 20px var(--shadow)}.sol-guide>header h3{margin:0;font-size:28px}.sol-guide.empty{padding-bottom:10px}.sol-warning{padding:12px;background:#f0d8b2;border-left:5px solid #a52e35}.sol-legend{display:flex;flex-wrap:wrap;gap:10px;margin:14px 0}.sol-legend span{padding:4px 10px;color:#fff;border:2px solid #fff8;border-radius:999px}.sol-legend .sol{background:#a52e35}.sol-legend .silver-sol{color:#252525;background:#d7d9dc}.sol-legend .gold-sol{color:#2a2118;background:#d4a528}.sol-map{margin:20px 0}.sol-map svg{display:block;width:100%;max-height:760px;background:#263a3e;border:3px solid var(--line)}.sol-map figcaption{margin-top:5px;color:#6f6049}.sol-pin circle{fill:#a52e35aa;stroke:#fff3d2;stroke-width:2}.sol-pin.silver-sol circle{fill:#e8ecf0cc;stroke:#4f555d}.sol-pin.gold-sol circle{fill:#e1ad24cc;stroke:#6c4b05}.sol-pin .exact{fill:none;stroke:#fff;stroke-width:1.5}.tower-source{margin:22px 0;padding:18px;background:var(--paper2);border:1px solid var(--line)}.tower-source>header h3{margin-top:0}
.route-map svg,.sol-map svg{height:auto;max-height:none;background:transparent}.sol-map-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px}.sol-map{min-width:0;margin:16px 0}.hero .sol-legend{margin-bottom:0}@media(max-width:920px){.sol-map-grid{grid-template-columns:1fr}}
"#;
