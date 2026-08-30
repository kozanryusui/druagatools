mod directory;
mod fat;
mod superblock;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub use directory::{InspectionLimits, ListingEntry, inspect_structure, render_listing};
pub use fat::read_fat_chain;
pub use superblock::Superblock;
use thiserror::Error;

use crate::atomic_output::{AtomicOutputError, write_bytes};

pub const DATA_BYTES_PER_PAGE: usize = 512;
pub const SPARE_BYTES_PER_PAGE: usize = 16;
pub const RAW_BYTES_PER_PAGE: usize = DATA_BYTES_PER_PAGE + SPARE_BYTES_PER_PAGE;

pub fn inspect_dongle(
    input: &Path,
    output: &Path,
    limits: InspectionLimits,
) -> Result<(), Ps2McError> {
    validate_input_path(input)?;
    validate_output_path(output)?;
    validate_source_metadata(input)?;

    let source = File::open(input)?;
    let mut reader = MemoryCardReader::new(source)?;
    let listing = render_listing(&inspect_structure(&mut reader, limits)?);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    write_bytes(output, listing.as_bytes())?;
    Ok(())
}

pub fn validate_input_path(path: &Path) -> Result<(), Ps2McError> {
    validate_bounded_path(path, "dongle")
        .map_err(|()| Ps2McError::SourcePathOutsideDongle(path.display().to_string()))
}

pub fn validate_output_path(path: &Path) -> Result<(), Ps2McError> {
    validate_bounded_path(path, "work")
        .map_err(|()| Ps2McError::PathOutsideWork(path.display().to_string()))
}

fn validate_bounded_path(path: &Path, required_root: &str) -> Result<(), ()> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || !matches!(path.components().next(), Some(Component::Normal(value)) if value == required_root)
    {
        return Err(());
    }
    Ok(())
}

fn validate_source_metadata(path: &Path) -> Result<(), Ps2McError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(Ps2McError::SourceNotRegular(path.display().to_string()));
    }
    if source_has_write_permission(&metadata.permissions()) {
        return Err(Ps2McError::SourceNotReadOnly(path.display().to_string()));
    }
    Ok(())
}

#[cfg(unix)]
fn source_has_write_permission(permissions: &fs::Permissions) -> bool {
    permissions.mode() & 0o222 != 0
}

#[cfg(not(unix))]
fn source_has_write_permission(permissions: &fs::Permissions) -> bool {
    !permissions.readonly()
}

#[derive(Debug, Error)]
pub enum Ps2McError {
    #[error("memory-card input/output failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    AtomicOutput(#[from] AtomicOutputError),
    #[error("unsupported dongle geometry in {field}: {value}")]
    UnsupportedDongleGeometry { field: &'static str, value: u64 },
    #[error("memory-card offset arithmetic overflowed")]
    OffsetOverflow,
    #[error(
        "memory-card read is outside the input: offset {offset}, length {length}, input length {input_len}"
    )]
    ReadOutOfRange {
        offset: u64,
        length: u64,
        input_len: u64,
    },
    #[error("FAT chain contains a cycle at cluster {cluster}")]
    FatCycle { cluster: u32 },
    #[error("directory traversal contains a cycle at cluster {cluster}")]
    DirectoryCycle { cluster: u32 },
    #[error("configured {limit} limit of {maximum} was exceeded")]
    LimitExceeded { limit: &'static str, maximum: usize },
    #[error("invalid directory entry: {0}")]
    InvalidDirectoryEntry(String),
    #[error("input path must be below dongle/: {0}")]
    SourcePathOutsideDongle(String),
    #[error("dongle source is not a regular file: {0}")]
    SourceNotRegular(String),
    #[error("dongle source is not read-only: {0}")]
    SourceNotReadOnly(String),
    #[error("output path must be below work/: {0}")]
    PathOutsideWork(String),
}

pub struct MemoryCardReader<R> {
    source: R,
    input_len: u64,
    superblock: Superblock,
}

impl<R: Read + Seek> MemoryCardReader<R> {
    pub fn new(mut source: R) -> Result<Self, Ps2McError> {
        let input_len = source.seek(SeekFrom::End(0))?;
        source.seek(SeekFrom::Start(0))?;
        let first_page = read_page_from(&mut source, input_len, 0)?;
        let superblock = Superblock::parse(&first_page)?;
        Ok(Self {
            source,
            input_len,
            superblock,
        })
    }

    #[must_use]
    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }

    pub fn read_data_page(&mut self, page: u64) -> Result<[u8; DATA_BYTES_PER_PAGE], Ps2McError> {
        read_page_from(&mut self.source, self.input_len, page)
    }

    pub fn read_logical_cluster(&mut self, cluster: u32) -> Result<Vec<u8>, Ps2McError> {
        if cluster >= self.superblock.clusters_per_card {
            return Err(Ps2McError::ReadOutOfRange {
                offset: u64::from(cluster),
                length: 1,
                input_len: u64::from(self.superblock.clusters_per_card),
            });
        }

        let pages_per_cluster = u64::from(self.superblock.pages_per_cluster);
        let first_page = u64::from(cluster)
            .checked_mul(pages_per_cluster)
            .ok_or(Ps2McError::OffsetOverflow)?;
        let capacity = DATA_BYTES_PER_PAGE
            .checked_mul(usize::from(self.superblock.pages_per_cluster))
            .ok_or(Ps2McError::OffsetOverflow)?;
        let mut cluster_data = Vec::with_capacity(capacity);

        for page_in_cluster in 0..pages_per_cluster {
            let page_number = first_page
                .checked_add(page_in_cluster)
                .ok_or(Ps2McError::OffsetOverflow)?;
            cluster_data.extend_from_slice(&self.read_data_page(page_number)?);
        }
        Ok(cluster_data)
    }
}

fn read_page_from<R: Read + Seek>(
    source: &mut R,
    input_len: u64,
    page: u64,
) -> Result<[u8; DATA_BYTES_PER_PAGE], Ps2McError> {
    let raw_page_len = u64::try_from(RAW_BYTES_PER_PAGE).map_err(|_| Ps2McError::OffsetOverflow)?;
    let data_page_len =
        u64::try_from(DATA_BYTES_PER_PAGE).map_err(|_| Ps2McError::OffsetOverflow)?;
    let offset = page
        .checked_mul(raw_page_len)
        .ok_or(Ps2McError::OffsetOverflow)?;
    let end = offset
        .checked_add(data_page_len)
        .ok_or(Ps2McError::OffsetOverflow)?;
    if end > input_len {
        return Err(Ps2McError::ReadOutOfRange {
            offset,
            length: data_page_len,
            input_len,
        });
    }

    source.seek(SeekFrom::Start(offset))?;
    let mut data = [0_u8; DATA_BYTES_PER_PAGE];
    source.read_exact(&mut data)?;
    Ok(data)
}
