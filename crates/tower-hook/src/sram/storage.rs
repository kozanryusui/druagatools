//! Safe fixed-size SRAM file storage.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SRAM_CAPACITY: usize = 0x1_0000;
const SRAM_FILE_NAME: &str = "druaga_sram.bin";

#[derive(Clone, Copy)]
pub(super) struct SramOffset(usize);

impl SramOffset {
    fn new(value: u32) -> Result<Self, SramStorageError> {
        let value = usize::try_from(value).map_err(|_| SramStorageError::RangeOutsideDevice)?;
        if value > SRAM_CAPACITY {
            return Err(SramStorageError::RangeOutsideDevice);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy)]
pub(super) struct SramLength(usize);

impl SramLength {
    fn new(value: u32) -> Result<Self, SramStorageError> {
        let value = usize::try_from(value).map_err(|_| SramStorageError::RangeOutsideDevice)?;
        if value > SRAM_CAPACITY {
            return Err(SramStorageError::RangeOutsideDevice);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy)]
pub(super) struct SramRange {
    offset: usize,
    length: usize,
}

impl SramRange {
    fn new(offset: SramOffset, length: SramLength) -> Result<Self, SramStorageError> {
        let end = offset
            .0
            .checked_add(length.0)
            .ok_or(SramStorageError::RangeOutsideDevice)?;
        if end > SRAM_CAPACITY {
            return Err(SramStorageError::RangeOutsideDevice);
        }
        Ok(Self {
            offset: offset.0,
            length: length.0,
        })
    }

    pub(super) fn from_raw(offset: u32, length: u32) -> Result<Self, SramStorageError> {
        Self::new(SramOffset::new(offset)?, SramLength::new(length)?)
    }

    fn seek_position(self) -> Result<u64, SramStorageError> {
        u64::try_from(self.offset).map_err(|_| SramStorageError::RangeOutsideDevice)
    }
}

#[derive(Debug)]
pub(super) enum SramStorageError {
    CurrentDirectory,
    FileOpen,
    FileMetadata,
    FileResize,
    FileSeek,
    FileRead,
    FileWrite,
    FileSync,
    BufferLength,
    RangeOutsideDevice,
    FileTooLarge,
}

fn storage_lock() -> &'static Mutex<()> {
    static STORAGE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    STORAGE_LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn sram_path() -> Result<PathBuf, SramStorageError> {
    std::env::current_dir()
        .map(|directory| directory.join(SRAM_FILE_NAME))
        .map_err(|_| SramStorageError::CurrentDirectory)
}

fn open_storage(path: &Path) -> Result<File, SramStorageError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| SramStorageError::FileOpen)?;
    let file_length = file
        .metadata()
        .map_err(|_| SramStorageError::FileMetadata)?
        .len();
    let capacity = u64::try_from(SRAM_CAPACITY).map_err(|_| SramStorageError::FileResize)?;
    if file_length > capacity {
        return Err(SramStorageError::FileTooLarge);
    }
    if file_length < capacity {
        file.set_len(capacity)
            .map_err(|_| SramStorageError::FileResize)?;
    }
    Ok(file)
}

pub(super) fn read_range(
    path: &Path,
    range: SramRange,
    destination: &mut [u8],
) -> Result<(), SramStorageError> {
    if destination.len() != range.length {
        return Err(SramStorageError::BufferLength);
    }
    let _guard = storage_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = open_storage(path)?;
    file.seek(SeekFrom::Start(range.seek_position()?))
        .map_err(|_| SramStorageError::FileSeek)?;
    file.read_exact(destination)
        .map_err(|_| SramStorageError::FileRead)
}

pub(super) fn write_range(
    path: &Path,
    range: SramRange,
    source: &[u8],
) -> Result<(), SramStorageError> {
    if source.len() != range.length {
        return Err(SramStorageError::BufferLength);
    }
    let _guard = storage_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut file = open_storage(path)?;
    file.seek(SeekFrom::Start(range.seek_position()?))
        .map_err(|_| SramStorageError::FileSeek)?;
    file.write_all(source)
        .map_err(|_| SramStorageError::FileWrite)?;
    file.sync_data().map_err(|_| SramStorageError::FileSync)
}
