use std::collections::HashSet;
use std::io::{Read, Seek};

use super::fat::read_fat_chain;
use super::superblock::{read_u16, read_u32};
use super::{DATA_BYTES_PER_PAGE, MemoryCardReader, Ps2McError};

const DIRECTORY_ENTRY_SIZE: usize = 512;
const MODE_DIRECTORY: u32 = 0x0020;
const MODE_USED: u32 = 0x8000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionLimits {
    pub max_depth: usize,
    pub max_entries: usize,
    pub max_chain_length: usize,
}

impl InspectionLimits {
    #[must_use]
    pub const fn new(max_depth: usize, max_entries: usize, max_chain_length: usize) -> Self {
        Self {
            max_depth,
            max_entries,
            max_chain_length,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListingEntry {
    pub path: String,
    pub raw_mode: u32,
    pub length: u32,
    pub cluster_chain: Vec<u32>,
    pub created: String,
    pub modified: String,
}

#[derive(Debug)]
struct DirectoryEntry {
    raw_mode: u32,
    length: u32,
    cluster: u32,
    created: String,
    modified: String,
    name: String,
}

pub fn inspect_structure<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    limits: InspectionLimits,
) -> Result<Vec<ListingEntry>, Ps2McError> {
    let root = reader.superblock().rootdir_cluster;
    let mut pending = vec![(root, String::new(), 0_usize)];
    let mut visited_directories = HashSet::new();
    let mut listing = Vec::new();

    while let Some((directory_cluster, parent_path, depth)) = pending.pop() {
        if !visited_directories.insert(directory_cluster) {
            return Err(Ps2McError::DirectoryCycle {
                cluster: directory_cluster,
            });
        }

        let entries = read_directory(reader, directory_cluster, limits.max_chain_length)?;
        for entry in entries.into_iter().skip(1) {
            if entry.raw_mode == u32::MAX
                || entry.raw_mode & MODE_USED == 0
                || is_dot_name(&entry.name)
            {
                continue;
            }
            if listing.len() >= limits.max_entries {
                return Err(Ps2McError::LimitExceeded {
                    limit: "entry count",
                    maximum: limits.max_entries,
                });
            }
            validate_name(&entry.name)?;
            let path = format!("{parent_path}/{}", entry.name);
            let is_directory = entry.raw_mode & MODE_DIRECTORY != 0;
            let cluster_chain = read_entry_chain(reader, &entry, is_directory, &limits)?;

            if is_directory {
                let child_depth = depth.checked_add(1).ok_or(Ps2McError::OffsetOverflow)?;
                if child_depth > limits.max_depth {
                    return Err(Ps2McError::LimitExceeded {
                        limit: "directory depth",
                        maximum: limits.max_depth,
                    });
                }
                pending.push((entry.cluster, path.clone(), child_depth));
            }

            listing.push(ListingEntry {
                path,
                raw_mode: entry.raw_mode,
                length: entry.length,
                cluster_chain,
                created: entry.created,
                modified: entry.modified,
            });
        }
    }

    listing.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(listing)
}

pub fn render_listing(entries: &[ListingEntry]) -> String {
    let mut output = String::from("path\tmode\tlength\tcluster_chain\tcreated\tmodified\n");
    for entry in entries {
        let chain = if entry.cluster_chain.is_empty() {
            "-".to_owned()
        } else {
            entry
                .cluster_chain
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        output.push_str(&format!(
            "{}\t0x{:08x}\t{}\t{}\t{}\t{}\n",
            entry.path, entry.raw_mode, entry.length, chain, entry.created, entry.modified
        ));
    }
    output
}

fn read_directory<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    directory_cluster: u32,
    max_chain_length: usize,
) -> Result<Vec<DirectoryEntry>, Ps2McError> {
    let chain = read_fat_chain(reader, directory_cluster, max_chain_length)?;
    let first = chain.first().copied().ok_or_else(|| {
        Ps2McError::InvalidDirectoryEntry("directory has no cluster chain".to_owned())
    })?;
    let first_data = read_allocation_cluster(reader, first)?;
    let header_data = first_data
        .get(..DIRECTORY_ENTRY_SIZE)
        .ok_or(Ps2McError::ReadOutOfRange {
            offset: 0,
            length: DIRECTORY_ENTRY_SIZE as u64,
            input_len: first_data.len() as u64,
        })?;
    let header = parse_entry(header_data)?;
    let entry_count = usize::try_from(header.length).map_err(|_| Ps2McError::OffsetOverflow)?;
    let available_entries = chain
        .len()
        .checked_mul(2)
        .ok_or(Ps2McError::OffsetOverflow)?;
    validate_directory_header(&header, directory_cluster, entry_count, available_entries)?;

    let mut entries = Vec::with_capacity(entry_count);
    for cluster in chain {
        let data = read_allocation_cluster(reader, cluster)?;
        for slot in 0..2 {
            if entries.len() == entry_count {
                return Ok(entries);
            }
            let start = slot * DIRECTORY_ENTRY_SIZE;
            let end = start
                .checked_add(DIRECTORY_ENTRY_SIZE)
                .ok_or(Ps2McError::OffsetOverflow)?;
            let raw = data.get(start..end).ok_or(Ps2McError::ReadOutOfRange {
                offset: start as u64,
                length: DIRECTORY_ENTRY_SIZE as u64,
                input_len: data.len() as u64,
            })?;
            entries.push(parse_entry(raw)?);
        }
    }
    Ok(entries)
}

fn validate_directory_header(
    header: &DirectoryEntry,
    directory_cluster: u32,
    entry_count: usize,
    available_entries: usize,
) -> Result<(), Ps2McError> {
    if header.raw_mode & (MODE_USED | MODE_DIRECTORY) != (MODE_USED | MODE_DIRECTORY) {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "directory header mode is not a used directory".to_owned(),
        ));
    }
    if header.name != "." {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "directory header name is not dot".to_owned(),
        ));
    }
    if header.cluster != directory_cluster {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "directory header cluster does not match its directory".to_owned(),
        ));
    }
    if entry_count == 0 || entry_count > available_entries {
        return Err(Ps2McError::InvalidDirectoryEntry(format!(
            "directory entry count {entry_count} exceeds capacity {available_entries}"
        )));
    }
    Ok(())
}

fn read_entry_chain<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    entry: &DirectoryEntry,
    is_directory: bool,
    limits: &InspectionLimits,
) -> Result<Vec<u32>, Ps2McError> {
    if is_directory {
        return read_fat_chain(reader, entry.cluster, limits.max_chain_length);
    }
    if entry.length == 0 {
        if entry.cluster != u32::MAX {
            return Err(Ps2McError::InvalidDirectoryEntry(
                "zero-length file has an allocated cluster".to_owned(),
            ));
        }
        return Ok(Vec::new());
    }
    if entry.cluster == u32::MAX {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "nonempty file has no allocated cluster".to_owned(),
        ));
    }

    let chain = read_fat_chain(reader, entry.cluster, limits.max_chain_length)?;
    let cluster_capacity = DATA_BYTES_PER_PAGE
        .checked_mul(usize::from(reader.superblock().pages_per_cluster))
        .ok_or(Ps2McError::OffsetOverflow)?;
    let file_length = usize::try_from(entry.length).map_err(|_| Ps2McError::OffsetOverflow)?;
    let capacity_adjustment = cluster_capacity
        .checked_sub(1)
        .ok_or(Ps2McError::OffsetOverflow)?;
    let required_clusters = file_length
        .checked_add(capacity_adjustment)
        .ok_or(Ps2McError::OffsetOverflow)?
        / cluster_capacity;
    if chain.len() < required_clusters {
        return Err(Ps2McError::InvalidDirectoryEntry(format!(
            "file length requires {required_clusters} clusters but the chain has {}",
            chain.len()
        )));
    }
    Ok(chain)
}

fn read_allocation_cluster<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    relative_cluster: u32,
) -> Result<Vec<u8>, Ps2McError> {
    if relative_cluster >= reader.superblock().alloc_end {
        return Err(Ps2McError::ReadOutOfRange {
            offset: u64::from(relative_cluster),
            length: 1,
            input_len: u64::from(reader.superblock().alloc_end),
        });
    }
    let raw_cluster = reader
        .superblock()
        .alloc_offset
        .checked_add(relative_cluster)
        .ok_or(Ps2McError::OffsetOverflow)?;
    reader.read_logical_cluster(raw_cluster)
}

fn parse_entry(data: &[u8]) -> Result<DirectoryEntry, Ps2McError> {
    if data.len() < DIRECTORY_ENTRY_SIZE {
        return Err(Ps2McError::ReadOutOfRange {
            offset: 0,
            length: DIRECTORY_ENTRY_SIZE as u64,
            input_len: data.len() as u64,
        });
    }
    let name_bytes = data.get(0x40..0x60).ok_or(Ps2McError::ReadOutOfRange {
        offset: 0x40,
        length: 0x20,
        input_len: data.len() as u64,
    })?;
    let name_end = name_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name_bytes.len());
    let name = String::from_utf8(name_bytes[..name_end].to_vec())
        .map_err(|_| Ps2McError::InvalidDirectoryEntry("name is not UTF-8".to_owned()))?;

    Ok(DirectoryEntry {
        raw_mode: read_u32(data, 0)?,
        length: read_u32(data, 4)?,
        created: parse_timestamp(data, 8)?,
        cluster: read_u32(data, 16)?,
        modified: parse_timestamp(data, 24)?,
        name,
    })
}

fn parse_timestamp(data: &[u8], offset: usize) -> Result<String, Ps2McError> {
    let end = offset.checked_add(8).ok_or(Ps2McError::OffsetOverflow)?;
    let raw = data.get(offset..end).ok_or(Ps2McError::ReadOutOfRange {
        offset: offset as u64,
        length: 8,
        input_len: data.len() as u64,
    })?;
    let year = read_u16(raw, 6)?;
    let second = byte_at(raw, 1)?;
    let minute = byte_at(raw, 2)?;
    let hour = byte_at(raw, 3)?;
    let day = byte_at(raw, 4)?;
    let month = byte_at(raw, 5)?;
    validate_timestamp(year, month, day, hour, minute, second)?;
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        month, day, hour, minute, second
    ))
}

fn validate_timestamp(
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> Result<(), Ps2McError> {
    if second > 59 || minute > 59 || hour > 23 || !(1..=12).contains(&month) {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "timestamp component is outside its valid range".to_owned(),
        ));
    }
    let maximum_day = days_in_month(year, month);
    if day == 0 || day > maximum_day {
        return Err(Ps2McError::InvalidDirectoryEntry(
            "timestamp day is not valid for its month".to_owned(),
        ));
    }
    Ok(())
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn byte_at(data: &[u8], offset: usize) -> Result<u8, Ps2McError> {
    data.get(offset).copied().ok_or(Ps2McError::ReadOutOfRange {
        offset: offset as u64,
        length: 1,
        input_len: data.len() as u64,
    })
}

fn validate_name(name: &str) -> Result<(), Ps2McError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return Err(Ps2McError::InvalidDirectoryEntry(format!(
            "invalid path name: {name:?}"
        )));
    }
    Ok(())
}

fn is_dot_name(name: &str) -> bool {
    matches!(name, "." | "..")
}
