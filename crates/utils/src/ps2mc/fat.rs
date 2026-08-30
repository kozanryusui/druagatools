use std::collections::HashSet;
use std::io::{Read, Seek};

use super::superblock::read_u32;
use super::{MemoryCardReader, Ps2McError};

const FAT_VALUE_MASK: u32 = 0x7fff_ffff;
const FAT_END: u32 = FAT_VALUE_MASK;

pub fn read_fat_chain<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    start_cluster: u32,
    max_chain_length: usize,
) -> Result<Vec<u32>, Ps2McError> {
    if start_cluster == u32::MAX || start_cluster & FAT_VALUE_MASK == FAT_END {
        return Ok(Vec::new());
    }

    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cluster = start_cluster & FAT_VALUE_MASK;
    loop {
        if cluster >= reader.superblock().alloc_end {
            return Err(Ps2McError::ReadOutOfRange {
                offset: u64::from(cluster),
                length: 1,
                input_len: u64::from(reader.superblock().alloc_end),
            });
        }
        if !visited.insert(cluster) {
            return Err(Ps2McError::FatCycle { cluster });
        }
        if chain.len() >= max_chain_length {
            return Err(Ps2McError::LimitExceeded {
                limit: "chain length",
                maximum: max_chain_length,
            });
        }
        chain.push(cluster);

        let next = read_fat_entry(reader, cluster)? & FAT_VALUE_MASK;
        if next == FAT_END {
            return Ok(chain);
        }
        cluster = next;
    }
}

fn read_fat_entry<R: Read + Seek>(
    reader: &mut MemoryCardReader<R>,
    cluster: u32,
) -> Result<u32, Ps2McError> {
    let bytes_per_cluster = usize::from(reader.superblock().pages_per_cluster)
        .checked_mul(usize::from(reader.superblock().page_len))
        .ok_or(Ps2McError::OffsetOverflow)?;
    let entries_per_cluster = bytes_per_cluster
        .checked_div(4)
        .ok_or(Ps2McError::OffsetOverflow)?;
    let cluster_index = usize::try_from(cluster).map_err(|_| Ps2McError::OffsetOverflow)?;
    let indirect_index = cluster_index / entries_per_cluster;
    let fat_entry_index = cluster_index % entries_per_cluster;

    let indirect_cluster = reader.superblock().first_indirect_fat_cluster;
    let indirect_data = reader.read_logical_cluster(indirect_cluster)?;
    let indirect_offset = indirect_index
        .checked_mul(4)
        .ok_or(Ps2McError::OffsetOverflow)?;
    let fat_cluster = read_u32(&indirect_data, indirect_offset)? & FAT_VALUE_MASK;
    if fat_cluster >= reader.superblock().alloc_offset {
        return Err(Ps2McError::ReadOutOfRange {
            offset: u64::from(fat_cluster),
            length: 1,
            input_len: u64::from(reader.superblock().alloc_offset),
        });
    }

    let fat_data = reader.read_logical_cluster(fat_cluster)?;
    let fat_offset = fat_entry_index
        .checked_mul(4)
        .ok_or(Ps2McError::OffsetOverflow)?;
    read_u32(&fat_data, fat_offset)
}
