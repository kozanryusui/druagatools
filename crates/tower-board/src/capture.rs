use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod load;
mod session;

pub use load::{load_capture_manifest, load_replay_frames};
pub use session::CaptureSession;

pub const MAX_CAPTURE_FRAMES: usize = 256;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const MAX_REPLAY_FRAMES: usize = MAX_CAPTURE_FRAMES + 2;
const CAPTURE_SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE_NAME: &str = "capture.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameDirection {
    TowerToBoard,
    BoardToTower,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceSource(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExpectedResult {
    BoardReady,
    ObservedFailure,
    MutationRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureStopReason {
    FrameCount,
    FrameBytes,
    TotalBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureFrame {
    pub sequence: u32,
    pub direction: FrameDirection,
    pub file: PathBuf,
    pub timing_us: u64,
    pub evidence_source: EvidenceSource,
    pub mutation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureManifest {
    pub schema_version: u32,
    pub case_id: String,
    pub expected_result: ExpectedResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounded_stop_reason: Option<CaptureStopReason>,
    #[serde(rename = "frame", default)]
    pub frames: Vec<CaptureFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureLimits {
    max_frames: usize,
    max_frame_bytes: usize,
    max_total_bytes: usize,
}

impl CaptureLimits {
    pub fn bounded(
        max_frames: usize,
        max_frame_bytes: usize,
        max_total_bytes: usize,
    ) -> Result<Self, CaptureError> {
        validate_limit("max_frames", max_frames, MAX_CAPTURE_FRAMES)?;
        validate_limit("max_frame_bytes", max_frame_bytes, MAX_FRAME_BYTES)?;
        validate_limit("max_total_bytes", max_total_bytes, MAX_CAPTURE_BYTES)?;
        Ok(Self {
            max_frames,
            max_frame_bytes,
            max_total_bytes,
        })
    }

    pub fn max_frames(self) -> usize {
        self.max_frames
    }

    pub fn max_frame_bytes(self) -> usize {
        self.max_frame_bytes
    }

    pub fn max_total_bytes(self) -> usize {
        self.max_total_bytes
    }
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_frames: MAX_CAPTURE_FRAMES,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_total_bytes: MAX_CAPTURE_BYTES,
        }
    }
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture input/output failed during {operation} for {path}: {source}")]
    Io {
        operation: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("capture manifest parse failed for {path}: {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("capture manifest serialization failed: {0}")]
    ManifestSerialize(#[source] toml::ser::Error),
    #[error("capture schema version {actual} is not supported; expected {expected}")]
    SchemaVersion { actual: u32, expected: u32 },
    #[error("capture case_id must not be empty")]
    EmptyCaseId,
    #[error("capture limit {name} must be from 1 through {maximum}; received {actual}")]
    InvalidLimit {
        name: &'static str,
        actual: usize,
        maximum: usize,
    },
    #[error("capture frame path must be relative: {path}")]
    AbsoluteFramePath { path: String },
    #[error("capture frame path contains a non-normal component: {path}")]
    FramePathTraversal { path: String },
    #[error("capture frame file is missing: {path}")]
    MissingFrame { path: String },
    #[error("capture frame path is not a regular file: {path}")]
    NonRegularFrame { path: String },
    #[error("capture frame sequence is {actual}; expected {expected}")]
    Sequence { actual: u32, expected: u32 },
    #[error("frame {sequence} has {actual} bytes; the limit is {limit}")]
    FrameTooLarge {
        sequence: u32,
        actual: usize,
        limit: usize,
    },
    #[error("capture has {attempted} frames; the limit is {limit}")]
    FrameLimit { attempted: usize, limit: usize },
    #[error("capture data would total {attempted} bytes; the limit is {limit}")]
    TotalLimit { attempted: usize, limit: usize },
    #[error("capture byte count overflowed")]
    ByteCountOverflow,
    #[error("capture mutation evidence requires mutation = true")]
    MutationRequired,
    #[error("a capture frame requires a nonempty evidence source")]
    EvidenceSourceRequired,
    #[error("a mutation frame evidence source must identify its original observation")]
    MutationEvidenceRequired,
    #[error("capture output already exists: {path}")]
    FrameAlreadyExists { path: String },
    #[error("capture output directory is not empty: {path}")]
    OutputDirectoryNotEmpty { path: String },
}

fn validate_limit(name: &'static str, actual: usize, maximum: usize) -> Result<(), CaptureError> {
    if actual == 0 || actual > maximum {
        return Err(CaptureError::InvalidLimit {
            name,
            actual,
            maximum,
        });
    }
    Ok(())
}

fn validate_case_id(case_id: &str) -> Result<(), CaptureError> {
    if case_id.trim().is_empty() {
        return Err(CaptureError::EmptyCaseId);
    }
    Ok(())
}

fn validate_evidence_source(source: &EvidenceSource, mutation: bool) -> Result<(), CaptureError> {
    if source.0.trim().is_empty() {
        return Err(CaptureError::EvidenceSourceRequired);
    }
    if mutation {
        let identifies_original = source.0.split_once(':').is_some_and(|(kind, reference)| {
            !kind.trim().is_empty() && !reference.trim().is_empty()
        });
        if !identifies_original {
            return Err(CaptureError::MutationEvidenceRequired);
        }
    }
    Ok(())
}

fn validate_frame_evidence(
    expected_result: ExpectedResult,
    source: &EvidenceSource,
    mutation: bool,
) -> Result<(), CaptureError> {
    validate_evidence_source(source, mutation)?;
    if expected_result == ExpectedResult::MutationRejected && !mutation {
        return Err(CaptureError::MutationRequired);
    }
    Ok(())
}

fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> CaptureError {
    CaptureError::Io {
        operation,
        path: path.display().to_string(),
        source,
    }
}
