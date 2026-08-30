use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use druaga_utils::atomic_output;
use druaga_utils::item_database::Character;
use druaga_utils::site_database::{TowerItemReward, TowerItemSource, TowerSourceDatabase};

const CHARACTERS: [Character; 4] = [
    Character::Gilgamesh,
    Character::Valkyrie,
    Character::YoungKi,
    Character::Xeovalga,
];

struct Arguments {
    lottery: PathBuf,
    presents: PathBuf,
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(env::args_os())?;
    let mut sources = parse_presents(&fs::read(args.presents)?)?;
    sources.extend(parse_lottery(&fs::read(args.lottery)?)?);
    sources.extend(recovery_sources());
    let database = TowerSourceDatabase {
        schema_version: 1,
        game_version: "1.60".to_owned(),
        sources,
    };
    atomic_output::write_bytes(&args.output, &serde_json::to_vec_pretty(&database)?)?;
    Ok(())
}

fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Arguments, Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    let [_, lottery, presents, output] = args.as_slice() else {
        return Err("usage: tower-source-builder LOTTERY.DAT PRESENT.DAT OUTPUT.JSON".into());
    };
    Ok(Arguments {
        lottery: lottery.into(),
        presents: presents.into(),
        output: output.into(),
    })
}

fn parse_presents(data: &[u8]) -> Result<Vec<TowerItemSource>, Box<dyn Error>> {
    require_magic(data, b"PRES")?;
    let count = usize::from(read_u16(data, 0x0c)?);
    let offset = usize::try_from(read_u32(data, 0x18)?)?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let record = offset + index * 24;
        let condition_count = *data.get(record + 0x13).ok_or("truncated present record")?;
        if condition_count != 1 {
            return Err(format!("present record {index} has {condition_count} conditions").into());
        }
        let condition = usize::try_from(read_u32(data, record + 0x14)?)?;
        let opcode = read_u16(data, condition)?;
        let first = read_u16(data, condition + 2)?;
        let second = read_u16(data, condition + 4)?;
        let (name, acquisition) = present_condition(opcode, first, second)?;
        let item_ids = CHARACTERS
            .iter()
            .enumerate()
            .map(|(character_index, _)| read_u16(data, record + character_index * 2))
            .collect::<Result<Vec<_>, _>>()?;
        let all_characters_receive_same_item = item_ids.windows(2).all(|pair| pair[0] == pair[1]);
        let mut rewards = Vec::new();
        for (character_index, character) in CHARACTERS.into_iter().enumerate() {
            if all_characters_receive_same_item && character_index != 0 {
                continue;
            }
            rewards.push(TowerItemReward {
                item_id: item_ids[character_index],
                character: (!all_characters_receive_same_item).then_some(character),
                chance_numerator: None,
                chance_denominator: None,
            });
        }
        result.push(TowerItemSource {
            id: format!("present-{index}"),
            name,
            acquisition,
            repeatability: "One time for each character card.".to_owned(),
            rewards,
        });
    }
    Ok(result)
}

fn present_condition(
    opcode: u16,
    first: u16,
    second: u16,
) -> Result<(String, String), Box<dyn Error>> {
    match opcode {
        1 => Ok((
            format!("Story clear {first}-{second}"),
            format!("Clear story chapter {first}, section {second}. Return to the Tower."),
        )),
        9 if second == 0 => Ok((
            format!("{first} quest clears"),
            format!("Clear {first} quests. Return to the Tower."),
        )),
        17 if second == 0 => Ok((
            format!("{first} titles"),
            format!("Obtain {first} titles. Return to the Tower."),
        )),
        _ => Err(format!("unsupported present condition {opcode}:{first}:{second}").into()),
    }
}

fn parse_lottery(data: &[u8]) -> Result<Vec<TowerItemSource>, Box<dyn Error>> {
    require_magic(data, b"LOTT")?;
    let mut rewards = Vec::new();
    for (character_index, character) in CHARACTERS.into_iter().enumerate() {
        let count = usize::from(read_u16(data, 8 + character_index * 2)?);
        let offset = usize::try_from(read_u32(data, 0x10 + character_index * 4)?)?;
        let mut previous = 0;
        for index in 0..count {
            let record = offset + index * 4;
            let threshold = read_u16(data, record)?;
            if threshold <= previous || threshold > 100 {
                return Err(format!("invalid lottery threshold {threshold}").into());
            }
            rewards.push(TowerItemReward {
                item_id: read_u16(data, record + 2)?,
                character: Some(character),
                chance_numerator: Some(threshold - previous),
                chance_denominator: Some(100),
            });
            previous = threshold;
        }
        if previous != 100 {
            return Err(format!("character lottery ends at {previous}, not 100").into());
        }
    }
    Ok(vec![TowerItemSource {
        id: "transferred-card-lottery".to_owned(),
        name: "Transferred-card lottery".to_owned(),
        acquisition: "Transfer a character card through the Tower. The Tower selects one reward from that character's table.".to_owned(),
        repeatability: "One reward for each transferred card.".to_owned(),
        rewards,
    }])
}

fn recovery_sources() -> Vec<TowerItemSource> {
    vec![
        TowerItemSource {
            id: "story-2-2-recovery".to_owned(),
            name: "Story item recovery after 2-2".to_owned(),
            acquisition: "Clear story chapter 2, section 2. If item 0x400D is missing, return to the Tower.".to_owned(),
            repeatability: "The Tower restores the item while it is missing.".to_owned(),
            rewards: vec![TowerItemReward {
                item_id: 0x400d,
                character: None,
                chance_numerator: None,
                chance_denominator: None,
            }],
        },
        TowerItemSource {
            id: "story-6-1-lantern-recovery".to_owned(),
            name: "Lantern recovery after 6-1".to_owned(),
            acquisition: "Clear story chapter 6, section 1. If no lantern from 0x400F through 0x4018 remains, return to the Tower.".to_owned(),
            repeatability: "The Tower restores item 0x400F while all lantern stages are missing.".to_owned(),
            rewards: vec![TowerItemReward {
                item_id: 0x400f,
                character: None,
                chance_numerator: None,
                chance_denominator: None,
            }],
        },
    ]
}

fn require_magic(data: &[u8], expected: &[u8; 4]) -> Result<(), Box<dyn Error>> {
    if data.get(..4) == Some(expected) && read_u32(data, 4)? == 100 {
        Ok(())
    } else {
        Err("unsupported Tower item source file".into())
    }
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, Box<dyn Error>> {
    Ok(u16::from_le_bytes(
        data.get(offset..offset + 2)
            .ok_or("truncated file")?
            .try_into()?,
    ))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, Box<dyn Error>> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("truncated file")?
            .try_into()?,
    ))
}
