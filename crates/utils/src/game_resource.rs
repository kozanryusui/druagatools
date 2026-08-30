use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

const INFO_HEADER_SIZE: usize = 16;
const INFO_RECORD_SIZE: usize = 0x30;
const PATH_SIZE: usize = 0x24;
const SECTOR_SIZE: u64 = 2048;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtractionSummary {
    pub file_count: usize,
    pub byte_count: u64,
}

#[derive(Debug, Error)]
pub enum GameResourceError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("INFO.DAT is too short: {actual} bytes; expected at least {minimum}")]
    InfoTooShort { actual: usize, minimum: usize },
    #[error("INFO.DAT record count {count} exceeds its {length}-byte file size")]
    RecordCount { count: u32, length: usize },
    #[error("INFO.DAT record {record} has no null-terminated path")]
    UnterminatedPath { record: usize },
    #[error("INFO.DAT record {record} has a non-ASCII path")]
    NonAsciiPath { record: usize },
    #[error("INFO.DAT record {record} has an unsafe path: {path}")]
    UnsafePath { record: usize, path: String },
    #[error("INFO.DAT contains the path more than once: {path}")]
    DuplicatePath { path: String },
    #[error(
        "INFO.DAT record {record} length {length} exceeds its {sector_count}-sector allocation"
    )]
    AllocationBounds {
        record: usize,
        length: u32,
        sector_count: u32,
    },
    #[error("INFO.DAT record {record} range cannot be represented")]
    RangeOverflow { record: usize },
    #[error(
        "INFO.DAT record {record} ends at GAME.DAT byte {end}, but GAME.DAT has {game_length} bytes"
    )]
    GameBounds {
        record: usize,
        end: u64,
        game_length: u64,
    },
    #[error("output directory already exists: {path}")]
    OutputExists { path: String },
    #[error("output path must name a directory: {path}")]
    InvalidOutput { path: String },
}

#[derive(Debug)]
struct ResourceEntry {
    record: usize,
    relative_path: PathBuf,
    offset: u64,
    length: u64,
}

#[derive(Debug)]
struct StagingDirectory {
    path: PathBuf,
    committed: bool,
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub fn extract_game_resources(
    info_path: &Path,
    game_path: &Path,
    output_path: &Path,
) -> Result<ExtractionSummary, GameResourceError> {
    if output_path
        .try_exists()
        .map_err(|source| io_error(output_path, source))?
    {
        return Err(GameResourceError::OutputExists {
            path: output_path.display().to_string(),
        });
    }

    let encoded_info = fs::read(info_path).map_err(|source| io_error(info_path, source))?;
    let entries = decode_entries(encoded_info)?;
    let game_length = fs::metadata(game_path)
        .map_err(|source| io_error(game_path, source))?
        .len();
    validate_game_bounds(&entries, game_length)?;

    let mut staging = create_staging_directory(output_path)?;
    let game_file = File::open(game_path).map_err(|source| io_error(game_path, source))?;
    let mut game = BufReader::new(game_file);
    let mut byte_count = 0_u64;

    for entry in &entries {
        let destination = staging.path.join(&entry.relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|source| io_error(&destination, source))?;
        game.seek(SeekFrom::Start(entry.offset))
            .map_err(|source| io_error(game_path, source))?;
        let copied = io::copy(&mut game.by_ref().take(entry.length), &mut output)
            .map_err(|source| io_error(&destination, source))?;
        if copied != entry.length {
            return Err(GameResourceError::GameBounds {
                record: entry.record,
                end: entry.offset + entry.length,
                game_length,
            });
        }
        byte_count = byte_count
            .checked_add(copied)
            .ok_or(GameResourceError::RangeOverflow {
                record: entry.record,
            })?;
    }

    if output_path
        .try_exists()
        .map_err(|source| io_error(output_path, source))?
    {
        return Err(GameResourceError::OutputExists {
            path: output_path.display().to_string(),
        });
    }
    fs::rename(&staging.path, output_path).map_err(|source| io_error(output_path, source))?;
    staging.committed = true;

    Ok(ExtractionSummary {
        file_count: entries.len(),
        byte_count,
    })
}

fn decode_entries(mut info: Vec<u8>) -> Result<Vec<ResourceEntry>, GameResourceError> {
    if info.len() < INFO_RECORD_SIZE {
        return Err(GameResourceError::InfoTooShort {
            actual: info.len(),
            minimum: INFO_RECORD_SIZE,
        });
    }

    let mut header = [0_u8; INFO_HEADER_SIZE];
    header.copy_from_slice(&info[..INFO_HEADER_SIZE]);
    for (index, value) in info.iter_mut().enumerate().skip(INFO_HEADER_SIZE) {
        *value = (!value.rotate_left(4)).wrapping_sub(header[index % INFO_HEADER_SIZE]);
    }

    let count = read_u32(&info[0x2c..0x30]);
    let count_usize = usize::try_from(count).map_err(|_| GameResourceError::RecordCount {
        count,
        length: info.len(),
    })?;
    let required_length = count_usize
        .checked_add(1)
        .and_then(|records| records.checked_mul(INFO_RECORD_SIZE))
        .ok_or(GameResourceError::RecordCount {
            count,
            length: info.len(),
        })?;
    if required_length > info.len() {
        return Err(GameResourceError::RecordCount {
            count,
            length: info.len(),
        });
    }

    let mut entries = Vec::with_capacity(count_usize);
    let mut paths = HashSet::with_capacity(count_usize);
    for record in 1..=count_usize {
        let start = record * INFO_RECORD_SIZE;
        let bytes = &info[start..start + INFO_RECORD_SIZE];
        let path_end = bytes[..PATH_SIZE]
            .iter()
            .position(|value| *value == 0)
            .ok_or(GameResourceError::UnterminatedPath { record })?;
        let path_bytes = &bytes[..path_end];
        if !path_bytes.is_ascii() {
            return Err(GameResourceError::NonAsciiPath { record });
        }
        let path = std::str::from_utf8(path_bytes)
            .map_err(|_| GameResourceError::NonAsciiPath { record })?;
        let relative_path = safe_relative_path(record, path)?;
        if !paths.insert(relative_path.clone()) {
            return Err(GameResourceError::DuplicatePath {
                path: path.to_owned(),
            });
        }

        let sector = u64::from(read_u32(&bytes[0x24..0x28]));
        let sector_count = read_u32(&bytes[0x28..0x2c]);
        let length = read_u32(&bytes[0x2c..0x30]);
        let allocated = u64::from(sector_count) * SECTOR_SIZE;
        if u64::from(length) > allocated {
            return Err(GameResourceError::AllocationBounds {
                record,
                length,
                sector_count,
            });
        }
        let offset = sector
            .checked_mul(SECTOR_SIZE)
            .ok_or(GameResourceError::RangeOverflow { record })?;
        entries.push(ResourceEntry {
            record,
            relative_path,
            offset,
            length: u64::from(length),
        });
    }
    Ok(entries)
}

fn safe_relative_path(record: usize, path: &str) -> Result<PathBuf, GameResourceError> {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') {
        return Err(GameResourceError::UnsafePath {
            record,
            path: path.to_owned(),
        });
    }

    let mut result = PathBuf::new();
    for component in path.split(['/', '\\']) {
        if component.is_empty() || component == "." || component == ".." || component.contains(':')
        {
            return Err(GameResourceError::UnsafePath {
                record,
                path: path.to_owned(),
            });
        }
        result.push(component);
    }
    Ok(result)
}

fn validate_game_bounds(
    entries: &[ResourceEntry],
    game_length: u64,
) -> Result<(), GameResourceError> {
    for entry in entries {
        let end =
            entry
                .offset
                .checked_add(entry.length)
                .ok_or(GameResourceError::RangeOverflow {
                    record: entry.record,
                })?;
        if end > game_length {
            return Err(GameResourceError::GameBounds {
                record: entry.record,
                end,
                game_length,
            });
        }
    }
    Ok(())
}

fn create_staging_directory(output: &Path) -> Result<StagingDirectory, GameResourceError> {
    let name = output
        .file_name()
        .ok_or_else(|| GameResourceError::InvalidOutput {
            path: output.display().to_string(),
        })?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging_path = parent.join(format!(
        ".{}.{}.{}.extracting",
        name.to_string_lossy(),
        std::process::id(),
        sequence
    ));
    fs::create_dir(&staging_path).map_err(|source| io_error(&staging_path, source))?;
    Ok(StagingDirectory {
        path: staging_path,
        committed: false,
    })
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn io_error(path: &Path, source: io::Error) -> GameResourceError {
    GameResourceError::Io {
        path: path.display().to_string(),
        source,
    }
}
