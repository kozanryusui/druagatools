use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

static SIBLING_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum AtomicOutputError {
    #[error("atomic output failed for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("staged output {path} has length {actual}, expected {expected}")]
    Length {
        path: String,
        actual: u64,
        expected: u64,
    },
    #[error("staged output operation failed: {0}")]
    Operation(String),
}

#[derive(Debug)]
pub struct StagedOutput {
    destination: PathBuf,
    staged: PathBuf,
    expected_length: u64,
}

#[derive(Debug)]
struct Backup {
    destination: PathBuf,
    path: PathBuf,
}

impl StagedOutput {
    #[doc(hidden)]
    pub fn staged_path(&self) -> &Path {
        &self.staged
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.staged);
    }
}

pub fn stage_bytes(destination: &Path, bytes: &[u8]) -> Result<StagedOutput, AtomicOutputError> {
    stage_file(
        destination,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        |path| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|source| io_error(path, source))?;
            file.write_all(bytes)
                .map_err(|source| io_error(path, source))?;
            Ok(())
        },
    )
}

pub fn stage_file<F>(
    destination: &Path,
    expected_length: u64,
    writer: F,
) -> Result<StagedOutput, AtomicOutputError>
where
    F: FnOnce(&Path) -> Result<(), AtomicOutputError>,
{
    let staged = unique_sibling_path(destination, "tmp")?;
    let result = writer(&staged).and_then(|()| validate_and_sync(&staged, expected_length));
    if let Err(error) = result {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok(StagedOutput {
        destination: destination.to_path_buf(),
        staged,
        expected_length,
    })
}

pub fn write_bytes(destination: &Path, bytes: &[u8]) -> Result<(), AtomicOutputError> {
    let staged = stage_bytes(destination, bytes)?;
    replace_all(vec![staged])
}

pub fn replace_all(outputs: Vec<StagedOutput>) -> Result<(), AtomicOutputError> {
    replace_all_with_rename(outputs, |source, destination| {
        fs::rename(source, destination)
    })
}

#[doc(hidden)]
pub fn replace_all_with_rename<F>(
    outputs: Vec<StagedOutput>,
    mut rename: F,
) -> Result<(), AtomicOutputError>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    for output in &outputs {
        if let Err(error) = validate_staged(output) {
            cleanup_staged(&outputs);
            return Err(error);
        }
    }

    let mut backups = Vec::with_capacity(outputs.len());
    for output in &outputs {
        if output.destination.exists() {
            let backup = unique_sibling_path(&output.destination, "bak")?;
            if let Err(source) = rename(&output.destination, &backup) {
                restore_backups(&backups, &mut rename);
                cleanup_staged(&outputs);
                return Err(io_error(&output.destination, source));
            }
            backups.push(Some(Backup {
                destination: output.destination.clone(),
                path: backup,
            }));
        } else {
            backups.push(None);
        }
    }

    for (installed, output) in outputs.iter().enumerate() {
        if let Err(source) = rename(&output.staged, &output.destination) {
            for installed_output in outputs.iter().take(installed) {
                let _ = fs::remove_file(&installed_output.destination);
            }
            restore_backups(&backups, &mut rename);
            cleanup_staged(&outputs);
            return Err(io_error(&output.destination, source));
        }
    }

    for backup in backups.into_iter().flatten() {
        fs::remove_file(&backup.path).map_err(|source| io_error(&backup.path, source))?;
    }
    Ok(())
}

fn validate_staged(output: &StagedOutput) -> Result<(), AtomicOutputError> {
    validate_length(&output.staged, output.expected_length)
}

fn validate_and_sync(path: &Path, expected_length: u64) -> Result<(), AtomicOutputError> {
    validate_length(path, expected_length)?;
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| io_error(path, source))
}

fn validate_length(path: &Path, expected_length: u64) -> Result<(), AtomicOutputError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if !metadata.file_type().is_file() {
        return Err(AtomicOutputError::Operation(format!(
            "staged output is not a regular file: {}",
            path.display()
        )));
    }
    let actual = metadata.len();
    if actual != expected_length {
        return Err(AtomicOutputError::Length {
            path: path.display().to_string(),
            actual,
            expected: expected_length,
        });
    }
    Ok(())
}

fn restore_backups<F>(backups: &[Option<Backup>], rename: &mut F)
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    for backup in backups.iter().rev().flatten() {
        if backup.destination.exists() {
            let _ = fs::remove_file(&backup.destination);
        }
        let _ = rename(&backup.path, &backup.destination);
    }
}

fn cleanup_staged(outputs: &[StagedOutput]) {
    for output in outputs {
        let _ = fs::remove_file(&output.staged);
    }
}

fn unique_sibling_path(destination: &Path, kind: &str) -> Result<PathBuf, AtomicOutputError> {
    let file_name = destination.file_name().ok_or_else(|| {
        AtomicOutputError::Operation(format!(
            "output path must name a file: {}",
            destination.display()
        ))
    })?;
    let sequence = SIBLING_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(destination.with_file_name(format!(
        ".{}.{}.{}.{}",
        file_name.to_string_lossy(),
        std::process::id(),
        sequence,
        kind
    )))
}

fn io_error(path: &Path, source: std::io::Error) -> AtomicOutputError {
    AtomicOutputError::Io {
        path: path.display().to_string(),
        source,
    }
}
