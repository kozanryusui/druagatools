use std::fs;
use std::path::{Component, Path};

use super::{
    CAPTURE_SCHEMA_VERSION, CaptureError, CaptureLimits, CaptureManifest, MAX_REPLAY_FRAMES,
    io_error, validate_case_id, validate_frame_evidence,
};

pub fn load_capture_manifest(
    manifest_path: &Path,
    limits: CaptureLimits,
) -> Result<CaptureManifest, CaptureError> {
    let manifest_metadata = fs::symlink_metadata(manifest_path)
        .map_err(|source| io_error("read metadata", manifest_path, source))?;
    if !manifest_metadata.file_type().is_file() {
        return Err(CaptureError::NonRegularFrame {
            path: manifest_path.display().to_string(),
        });
    }
    let text = fs::read_to_string(manifest_path)
        .map_err(|source| io_error("read manifest", manifest_path, source))?;
    let manifest: CaptureManifest =
        toml::from_str(&text).map_err(|source| CaptureError::ManifestParse {
            path: manifest_path.display().to_string(),
            source,
        })?;
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    validate_manifest(&manifest, root, limits)?;
    Ok(manifest)
}

pub fn load_replay_frames(
    manifest_path: &Path,
) -> Result<(CaptureManifest, Vec<Vec<u8>>), CaptureError> {
    let replay_limits = CaptureLimits {
        max_frames: MAX_REPLAY_FRAMES,
        ..CaptureLimits::default()
    };
    let manifest = load_capture_manifest(manifest_path, replay_limits)?;
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut raw_frames = Vec::with_capacity(manifest.frames.len());
    for frame in &manifest.frames {
        let path = root.join(&frame.file);
        let raw = fs::read(&path).map_err(|source| io_error("read frame", &path, source))?;
        raw_frames.push(raw);
    }
    Ok((manifest, raw_frames))
}

fn validate_manifest(
    manifest: &CaptureManifest,
    root: &Path,
    limits: CaptureLimits,
) -> Result<(), CaptureError> {
    if manifest.schema_version != CAPTURE_SCHEMA_VERSION {
        return Err(CaptureError::SchemaVersion {
            actual: manifest.schema_version,
            expected: CAPTURE_SCHEMA_VERSION,
        });
    }
    validate_case_id(&manifest.case_id)?;
    if manifest.frames.len() > limits.max_frames {
        return Err(CaptureError::FrameLimit {
            attempted: manifest.frames.len(),
            limit: limits.max_frames,
        });
    }

    let mut total_bytes = 0_usize;
    for (index, frame) in manifest.frames.iter().enumerate() {
        let expected = u32::try_from(index).map_err(|_| CaptureError::FrameLimit {
            attempted: manifest.frames.len(),
            limit: limits.max_frames,
        })?;
        if frame.sequence != expected {
            return Err(CaptureError::Sequence {
                actual: frame.sequence,
                expected,
            });
        }
        validate_frame_path(&frame.file)?;
        validate_frame_evidence(
            manifest.expected_result,
            &frame.evidence_source,
            frame.mutation,
        )?;
        let path = root.join(&frame.file);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(CaptureError::MissingFrame {
                    path: path.display().to_string(),
                });
            }
            Err(source) => return Err(io_error("read frame metadata", &path, source)),
        };
        if !metadata.file_type().is_file() {
            return Err(CaptureError::NonRegularFrame {
                path: path.display().to_string(),
            });
        }
        let actual = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if actual > limits.max_frame_bytes {
            return Err(CaptureError::FrameTooLarge {
                sequence: frame.sequence,
                actual,
                limit: limits.max_frame_bytes,
            });
        }
        total_bytes = total_bytes
            .checked_add(actual)
            .ok_or(CaptureError::ByteCountOverflow)?;
        if total_bytes > limits.max_total_bytes {
            return Err(CaptureError::TotalLimit {
                attempted: total_bytes,
                limit: limits.max_total_bytes,
            });
        }
    }
    Ok(())
}

fn validate_frame_path(path: &Path) -> Result<(), CaptureError> {
    if path.is_absolute() {
        return Err(CaptureError::AbsoluteFramePath {
            path: path.display().to_string(),
        });
    }
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CaptureError::FramePathTraversal {
            path: path.display().to_string(),
        });
    }
    Ok(())
}
