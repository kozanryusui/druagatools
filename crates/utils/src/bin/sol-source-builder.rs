use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use druaga_utils::atomic_output;
use druaga_utils::gsm2::Image;
use druaga_utils::item_database::ItemDatabase;
use druaga_utils::site_database::{
    ChestGuideDatabase, QuestPoolReward, QuestSourceDatabase, QuestSourceIdentity,
    QuestSourceQuest, QuestTreasurePool, QuestTreasureSource, SolArea, SolFloor, SolKind,
    SolLocation, SolMinimap,
};
use serde::Deserialize;

struct Arguments {
    scripts: PathBuf,
    area_database: PathBuf,
    map_names: PathBuf,
    source_minimaps: PathBuf,
    items: PathBuf,
    chests: PathBuf,
    quest_sources: PathBuf,
    output_minimaps: PathBuf,
}

#[derive(Deserialize)]
struct AreaRecord {
    scripts: Vec<u8>,
    area: u8,
    stage: String,
}

#[derive(Clone)]
struct MapRecord {
    stage: String,
    image: PathBuf,
    width: u16,
    height: u16,
    origin_x: i16,
    origin_z: i16,
    occupied: Vec<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Event {
    slot: u8,
    kind: SolKind,
    x: f32,
    z: f32,
    radius: f32,
    area: Option<u8>,
    layout: Option<u8>,
    direct_item_id: Option<u16>,
}

#[derive(Clone, Copy)]
struct Spawn {
    slot: u8,
    kind: SolKind,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(env::args_os())?;
    let areas: Vec<AreaRecord> = read_json(&args.area_database)?;
    let items: ItemDatabase = read_json(&args.items)?;
    let mut chests: ChestGuideDatabase = read_json(&args.chests)?;
    let mut sources: QuestSourceDatabase = read_json(&args.quest_sources)?;
    let item_ids = items
        .items
        .iter()
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    let maps = read_maps(&args.map_names, &args.source_minimaps)?;
    fs::create_dir_all(&args.output_minimaps)?;

    remove_old_sol_sources(&mut sources);
    for quest in &mut chests.quests {
        let script = fs::read_to_string(args.scripts.join(format!("party{:02}.txt", quest.id)))?;
        let events = parse_events(quest.id, &script)?;
        let (sol_areas, unmapped_sol_locations) = map_events(quest.id, &events, &areas, &maps)?;
        quest.sol_areas = sol_areas;
        quest.unmapped_sol_locations = unmapped_sol_locations;
        copy_minimaps(
            &quest.sol_areas,
            &args.source_minimaps,
            &args.output_minimaps,
        )?;
        add_quest_sources(
            &mut sources,
            quest.id,
            quest.network_id,
            &quest.name,
            &script,
            &events,
            &item_ids,
        )?;
    }

    atomic_output::write_bytes(&args.chests, &serde_json::to_vec_pretty(&chests)?)?;
    atomic_output::write_bytes(&args.quest_sources, &serde_json::to_vec_pretty(&sources)?)?;
    Ok(())
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Arguments, Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    let [
        _,
        scripts,
        areas,
        map_names,
        source_minimaps,
        items,
        chests,
        quest_sources,
        output_minimaps,
    ] = args.as_slice()
    else {
        return Err("usage: sol-source-builder SCRIPTS-DIRECTORY MAINCTRL-AREAS.JSON MAPNAME.DAT SOURCE-MINIMAPS ITEMS.JSON CHESTS.JSON QUEST-SOURCES.JSON OUTPUT-MINIMAPS".into());
    };
    Ok(Arguments {
        scripts: scripts.into(),
        area_database: areas.into(),
        map_names: map_names.into(),
        source_minimaps: source_minimaps.into(),
        items: items.into(),
        chests: chests.into(),
        quest_sources: quest_sources.into(),
        output_minimaps: output_minimaps.into(),
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn read_maps(path: &Path, minimaps: &Path) -> Result<HashMap<String, MapRecord>, Box<dyn Error>> {
    let data = fs::read(path)?;
    if data.len() % 32 != 0 {
        return Err("mapname.dat has a partial record".into());
    }
    let mut result = HashMap::new();
    for record in data.chunks_exact(32) {
        let path_end = record[..28]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(28);
        let resource = std::str::from_utf8(&record[..path_end])?;
        let Some(name) = resource.strip_prefix("map/") else {
            continue;
        };
        let Some(base) = name.strip_suffix(".gsm") else {
            continue;
        };
        let image = minimaps.join(format!("{base}.png"));
        if !image.exists() {
            continue;
        }
        let (width, height) = png_size(&image)?;
        let gsm = fs::read(
            path.parent()
                .ok_or("mapname.dat has no parent directory")?
                .join(format!("{base}.gsm")),
        )?;
        let gsm = Image::parse(&gsm)?;
        if gsm.width != width || gsm.height != height {
            return Err(format!("{base} GSM and PNG dimensions differ").into());
        }
        let occupied = gsm
            .rgba_pixels()
            .chunks_exact(4)
            .map(|pixel| pixel[3] != 0)
            .collect();
        result.insert(
            format!("{base}.gmk"),
            MapRecord {
                stage: format!("{base}.gmk"),
                image,
                width,
                height,
                origin_x: i16::from_le_bytes(record[28..30].try_into()?),
                origin_z: i16::from_le_bytes(record[30..32].try_into()?),
                occupied,
            },
        );
    }
    Ok(result)
}

fn png_size(path: &Path) -> Result<(u16, u16), Box<dyn Error>> {
    let data = fs::read(path)?;
    if data.get(..16) != Some(&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..]) {
        return Err(format!("{} is not a PNG file", path.display()).into());
    }
    let width = u32::from_be_bytes(data[16..20].try_into()?).try_into()?;
    let height = u32::from_be_bytes(data[20..24].try_into()?).try_into()?;
    Ok((width, height))
}

fn parse_events(script_id: u8, script: &str) -> Result<Vec<Event>, Box<dyn Error>> {
    let functions = function_ranges(script);
    let mut helpers = BTreeMap::<String, Spawn>::new();
    for (name, body) in &functions {
        let spawns = body.lines().filter_map(parse_spawn).collect::<Vec<_>>();
        if let Some(first) = spawns.first().copied()
            && spawns
                .iter()
                .all(|spawn| spawn.slot == first.slot && spawn.kind == first.kind)
        {
            helpers.insert(name.clone(), first);
        }
    }

    let lines = script.lines().collect::<Vec<_>>();
    let mut events = Vec::new();
    let mut conditions = Vec::<String>::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('}') {
            conditions.pop();
        }
        if trimmed.contains("if (") && trimmed.ends_with('{') {
            conditions.push(trimmed.to_owned());
        }
        let Some((x, z, radius)) = parse_force_position(line) else {
            continue;
        };
        let context = conditions.join(" ");
        let area = parse_comparison(&context, "GetQuestProgressState() == ")
            .or_else(|| dynamic_area(script_id, line));
        let layout = if matches!(script_id, 31 | 62) {
            parse_comparison(&context, "GetQuestIntegerParameter(value[91]) == ")
        } else if matches!(script_id, 22 | 53) {
            parse_comparison(&context, "value[207] == ")
        } else if script_id == 28 {
            parse_comparison(&context, "value[71] == ")
        } else if script_id == 59 {
            parse_comparison(&context, "value[69] == ")
        } else if matches!(script_id, 64 | 65) {
            parse_comparison(&context, "value[51] == ")
        } else {
            None
        };
        let following = lines[index..lines.len().min(index + 8)].join(" ");
        let spawn = parse_spawn(&following).or_else(|| {
            helpers
                .iter()
                .filter_map(|(name, spawn)| {
                    following
                        .find(&format!("{name}();"))
                        .map(|offset| (offset, *spawn))
                })
                .min_by_key(|(offset, _)| *offset)
                .map(|(_, spawn)| spawn)
        });
        if let Some(spawn) = spawn {
            let direct_item_id = parse_direct_reward(&following, spawn.slot);
            events.push(Event {
                slot: spawn.slot,
                kind: spawn.kind,
                x,
                z,
                radius,
                area,
                layout,
                direct_item_id,
            });
        }
    }
    events.dedup();
    Ok(events)
}

fn parse_direct_reward(context: &str, slot: u8) -> Option<u16> {
    let arguments = call_arguments(context, &format!("SetTreasureBoxRewards({slot}, "))?;
    arguments.split(',').next()?.trim().parse().ok()
}

fn function_ranges(script: &str) -> Vec<(String, &str)> {
    let mut starts = Vec::new();
    for marker in ["fn main() {", "fn unit_"] {
        let mut search = 0;
        while let Some(offset) = script[search..].find(marker) {
            starts.push(search + offset);
            search += offset + marker.len();
        }
    }
    starts.sort_unstable();
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(script.len());
            let header = script[*start..].lines().next().unwrap_or_default();
            let name = header
                .strip_prefix("fn ")
                .and_then(|value| value.split_once('('))
                .map(|(name, _)| name)
                .unwrap_or_default();
            (name.to_owned(), &script[*start..end])
        })
        .collect()
}

fn parse_force_position(line: &str) -> Option<(f32, f32, f32)> {
    if !line.contains("TestLocalPlayerState(3)") {
        return None;
    }
    let arguments = call_arguments(line, "IsLocalPlayerNearPosition(")?;
    let values = arguments.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 3 {
        return None;
    }
    Some((
        parse_script_float(values[0])?,
        parse_script_float(values[1])?,
        parse_script_float(values[2])?,
    ))
}

fn parse_script_float(value: &str) -> Option<f32> {
    value
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .parse()
        .ok()
}

fn parse_spawn(line: &str) -> Option<Spawn> {
    let arguments = call_arguments(line, "SpawnTreasureBox(")?;
    let values = arguments.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() < 5 || values[2] != "1" {
        return None;
    }
    let kind = match values[1] {
        "3" => SolKind::Sol,
        "8" => SolKind::SilverSol,
        "9" => SolKind::GoldSol,
        _ => return None,
    };
    Some(Spawn {
        slot: values[0].parse().ok()?,
        kind,
    })
}

fn call_arguments<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let start = line.find(marker)? + marker.len();
    let mut depth = 0;
    for (offset, character) in line[start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(&line[start..start + offset]),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn parse_comparison(context: &str, marker: &str) -> Option<u8> {
    let value = context.split(marker).nth(1)?;
    value
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn dynamic_area(script_id: u8, line: &str) -> Option<u8> {
    matches!(script_id, 31 | 62)
        .then(|| parse_comparison(line, "value[91] == "))
        .flatten()
}

fn map_events(
    script_id: u8,
    events: &[Event],
    areas: &[AreaRecord],
    maps: &HashMap<String, MapRecord>,
) -> Result<(Vec<SolArea>, u8), Box<dyn Error>> {
    let mut grouped = BTreeMap::<(u8, String), Vec<SolLocation>>::new();
    let mut unmapped = 0u8;
    for event in events {
        let stages = if let Some(stage) = dynamic_stage(script_id, event) {
            vec![(event.area.unwrap_or(0), stage)]
        } else {
            areas
                .iter()
                .filter(|record| record.scripts.contains(&script_id))
                .filter(|record| sol_can_appear_in_area(script_id, record.area))
                .filter(|record| event.area.is_none_or(|area| record.area == area))
                .filter_map(|record| maps.get(&record.stage).map(|map| (record, map)))
                .filter(|(_, map)| point_is_on_map(event.x, event.z, event.radius, map))
                .map(|(record, _)| (record.area, record.stage.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        if stages.is_empty() {
            let active_maps = areas
                .iter()
                .filter(|record| record.scripts.contains(&script_id))
                .map(|record| format!("area {} {}", record.area, record.stage))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "party{script_id:02} Sol position ({}, {}) is not on an active map; active maps: {active_maps}",
                event.x, event.z,
            );
            unmapped = unmapped
                .checked_add(1)
                .ok_or("too many unmapped Sol locations")?;
            continue;
        }
        for (area, stage) in stages {
            let map = maps
                .get(&stage)
                .ok_or_else(|| format!("no minimap record for {stage}"))?;
            if !point_is_on_map(event.x, event.z, event.radius, map) {
                eprintln!(
                    "party{script_id:02} Sol position ({}, {}) is outside {stage}",
                    event.x, event.z
                );
                unmapped = unmapped
                    .checked_add(1)
                    .ok_or("too many unmapped Sol locations")?;
                continue;
            }
            grouped.entry((area, stage)).or_default().push(SolLocation {
                kind: event.kind,
                world_x: event.x,
                world_z: event.z,
                radius: event.radius,
            });
        }
    }
    let mut areas = grouped
        .into_iter()
        .map(|((area_index, stage), mut locations)| {
            locations.dedup_by(|left, right| {
                left.kind == right.kind
                    && left.world_x == right.world_x
                    && left.world_z == right.world_z
                    && left.radius == right.radius
            });
            let map = &maps[&stage];
            let image = map
                .image
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid minimap file name")?;
            Ok(SolArea {
                area_index,
                floor: sol_floor(script_id, area_index, &stage),
                stage: map.stage.clone(),
                minimap: SolMinimap {
                    image: format!("minimaps/{image}"),
                    width: map.width,
                    height: map.height,
                    origin_x: map.origin_x,
                    origin_z: map.origin_z,
                },
                locations,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    areas.sort_by(
        |left, right| match (first_floor(left), first_floor(right)) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => left
                .area_index
                .cmp(&right.area_index)
                .then_with(|| left.stage.cmp(&right.stage)),
        },
    );
    Ok((areas, unmapped))
}

fn first_floor(area: &SolArea) -> Option<u8> {
    match area.floor.as_ref()? {
        SolFloor::Single(floor) => Some(*floor),
        SolFloor::Multiple(floors) => floors.first().copied(),
    }
}

fn sol_can_appear_in_area(script_id: u8, area: u8) -> bool {
    !matches!(script_id, 12 | 45) || (1..=4).contains(&area)
}

fn tower_floor_sequence(script_id: u8, area: u8) -> Vec<u8> {
    if !matches!(script_id, 12 | 45) || !(1..=4).contains(&area) {
        return Vec::new();
    }
    (1..=55).step_by(6).map(|start| start + area).collect()
}

fn sol_floor(script_id: u8, area: u8, stage: &str) -> Option<SolFloor> {
    if let Some(floor) = tower_floor(script_id, stage) {
        return Some(SolFloor::Single(floor));
    }
    let floors = tower_floor_sequence(script_id, area);
    (!floors.is_empty()).then_some(SolFloor::Multiple(floors))
}

fn tower_floor(script_id: u8, stage: &str) -> Option<u8> {
    matches!(script_id, 28 | 59 | 64 | 65)
        .then(|| {
            stage
                .strip_prefix("pq29_0_")?
                .strip_suffix(".gmk")?
                .parse()
                .ok()
        })
        .flatten()
}

fn dynamic_stage(script_id: u8, event: &Event) -> Option<String> {
    match (script_id, event.layout) {
        (22, Some(layout @ 1..=3)) => Some(format!("pq23_b_{layout}.gmk")),
        (53, Some(layout @ 1..=3)) => Some(format!("pq54_b_{layout}.gmk")),
        (31, Some(layout @ 0..=9)) => Some(format!("pq32_1_{layout}.gmk")),
        (62, Some(layout @ 0..=9)) => Some(format!("pq63_1_{layout}.gmk")),
        (28, Some(layout @ 0..=127)) => Some(format!("pq29_0_{layout}.gmk")),
        (59, Some(layout @ 0..=127)) => Some(format!("pq29_0_{layout}.gmk")),
        (64 | 65, Some(layout @ 0..=127)) => Some(format!("pq29_0_{layout}.gmk")),
        _ => None,
    }
}

fn point_is_on_map(x: f32, z: f32, radius: f32, map: &MapRecord) -> bool {
    let pixel_x = (x - f32::from(map.origin_x)) / 5.0 + 1.5;
    let pixel_y = (z - f32::from(map.origin_z)) / 5.0 + 1.5;
    if !(0.0..f32::from(map.width)).contains(&pixel_x)
        || !(0.0..f32::from(map.height)).contains(&pixel_y)
    {
        return false;
    }
    let center_x = pixel_x.round() as i32;
    let center_y = pixel_y.round() as i32;
    let pixel_radius = (radius / 5.0).ceil() as i32;
    (-pixel_radius..=pixel_radius).any(|offset_y| {
        (-pixel_radius..=pixel_radius).any(|offset_x| {
            let x = center_x + offset_x;
            let y = center_y + offset_y;
            x >= 0
                && y >= 0
                && x < i32::from(map.width)
                && y < i32::from(map.height)
                && map.occupied[y as usize * usize::from(map.width) + x as usize]
        })
    })
}

fn copy_minimaps(areas: &[SolArea], source: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    for area in areas {
        let file = Path::new(&area.minimap.image)
            .file_name()
            .ok_or("invalid minimap output name")?;
        fs::copy(source.join(file), output.join(file))?;
    }
    Ok(())
}

fn remove_old_sol_sources(database: &mut QuestSourceDatabase) {
    database
        .treasure_pools
        .retain(|pool| !pool.id.starts_with("sol-"));
    for quest in &mut database.quests {
        quest
            .treasure_sources
            .retain(|source| !source.pool_id.starts_with("sol-"));
    }
}

fn add_quest_sources(
    database: &mut QuestSourceDatabase,
    script_id: u8,
    network_id: u16,
    name: &str,
    script: &str,
    events: &[Event],
    item_ids: &BTreeSet<u16>,
) -> Result<(), Box<dyn Error>> {
    let mut groups = BTreeMap::<(u8, &'static str, Option<u16>), Vec<&Event>>::new();
    for event in events {
        groups
            .entry((event.slot, sol_kind_slug(event.kind), event.direct_item_id))
            .or_default()
            .push(event);
    }
    if groups.is_empty() {
        return Ok(());
    }
    let quest_index = database
        .quests
        .iter()
        .position(|quest| matches!(quest.identity, QuestSourceIdentity::Scheduled { guide_quest_id, .. } if guide_quest_id == script_id))
        .unwrap_or_else(|| {
            database.quests.push(QuestSourceQuest {
                identity: QuestSourceIdentity::Scheduled {
                    guide_quest_id: script_id,
                    network_id,
                },
                name: name.to_owned(),
                rewards: Vec::new(),
                treasure_sources: Vec::new(),
                direct_reward_sources: Vec::new(),
            });
            database.quests.len() - 1
        });
    for ((slot, _kind_slug, direct_item_id), events) in groups {
        let rewards = if let Some(item_id) = direct_item_id {
            if !item_ids.contains(&item_id) {
                return Err(format!("party{script_id:02} refers to missing item {item_id}").into());
            }
            BTreeSet::from([item_id])
        } else {
            reward_items(script, slot, item_ids)?
        };
        if rewards.is_empty() {
            return Err(
                format!("party{script_id:02} slot {slot} has no resolved Sol rewards").into(),
            );
        }
        let pool_id = direct_item_id.map_or_else(
            || format!("sol-pool-{:016x}", reward_pool_hash(&rewards)),
            |item_id| format!("sol-direct-item-{item_id:04x}"),
        );
        if !database
            .treasure_pools
            .iter()
            .any(|pool| pool.id == pool_id)
        {
            database.treasure_pools.push(QuestTreasurePool {
                id: pool_id.clone(),
                rewards: rewards
                    .into_iter()
                    .map(|item_id| QuestPoolReward {
                        item_id,
                        chance_numerator: None,
                        chance_denominator: None,
                        selection_condition: Some("Possible reward from this Sol pool.".to_owned()),
                    })
                    .collect(),
                money: None,
            });
        }
        let kind = events[0].kind;
        database.quests[quest_index]
            .treasure_sources
            .push(QuestTreasureSource {
                pool_id,
                acquisition: format!(
                    "Use Force at the selected {} location in the hidden chest guide.",
                    sol_kind_name(kind)
                ),
                repeatability: format!("One {} in each quest run.", sol_kind_name(kind)),
                candidate_locations: events.iter().map(|event| [event.x, event.z]).collect(),
            });
    }
    Ok(())
}

fn reward_pool_hash(rewards: &BTreeSet<u16>) -> u64 {
    rewards.iter().fold(0xcbf29ce484222325, |hash, item_id| {
        item_id.to_le_bytes().into_iter().fold(hash, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
    })
}

fn reward_items(
    script: &str,
    slot: u8,
    item_ids: &BTreeSet<u16>,
) -> Result<BTreeSet<u16>, Box<dyn Error>> {
    let marker = format!("SetTreasureBoxRewards({slot}, ");
    let lines = script.lines().collect::<Vec<_>>();
    for (line_index, line) in lines.iter().enumerate() {
        let Some(arguments) = call_arguments(line, &marker) else {
            continue;
        };
        let literal_items = arguments
            .split(',')
            .filter_map(|value| value.trim().parse::<u16>().ok())
            .filter(|item_id| item_ids.contains(item_id))
            .collect::<BTreeSet<_>>();
        if !literal_items.is_empty() {
            return Ok(literal_items);
        }
        let Some(base) = arguments
            .split(',')
            .next()
            .and_then(|value| value.trim().strip_prefix("value["))
            .and_then(|value| value.split([' ', ']']).next())
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        let producers = lines[..line_index]
            .iter()
            .rev()
            .take(12)
            .filter_map(|line| parse_unit_call(line))
            .collect::<Vec<_>>();
        let functions = function_ranges(script);
        for unit in producers {
            let body = functions
                .iter()
                .find(|(name, _)| name == &unit)
                .map(|(_, body)| *body)
                .ok_or_else(|| format!("missing reward producer {unit}"))?;
            let mut result = BTreeSet::new();
            for assignment in body
                .lines()
                .filter(|line| line.contains(&format!("value[{base}")))
            {
                let Some((_, right)) = assignment.split_once(" = ") else {
                    continue;
                };
                let value = right.trim().trim_end_matches(';');
                if let Ok(item_id) = value.parse::<u16>()
                    && item_ids.contains(&item_id)
                {
                    result.insert(item_id);
                }
            }
            if !result.is_empty() {
                return Ok(result);
            }
        }
    }
    Ok(BTreeSet::new())
}

fn parse_unit_call(line: &str) -> Option<String> {
    let value = line.trim();
    value
        .strip_prefix("unit_")
        .and_then(|value| value.strip_suffix("();"))
        .map(|number| format!("unit_{number}"))
}

fn sol_kind_slug(kind: SolKind) -> &'static str {
    match kind {
        SolKind::Sol => "sol",
        SolKind::SilverSol => "silver-sol",
        SolKind::GoldSol => "gold-sol",
    }
}

fn sol_kind_name(kind: SolKind) -> &'static str {
    match kind {
        SolKind::Sol => "Sol",
        SolKind::SilverSol => "Silver Sol",
        SolKind::GoldSol => "Gold Sol",
    }
}
