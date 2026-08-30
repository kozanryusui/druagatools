use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{
    CAPTURE_SCHEMA_VERSION, CaptureError, CaptureFrame, CaptureLimits, CaptureManifest,
    CaptureStopReason, EvidenceSource, ExpectedResult, FrameDirection, MANIFEST_FILE_NAME,
    io_error, validate_case_id, validate_frame_evidence,
};

#[derive(Debug)]
pub struct CaptureSession {
    manifest_path: PathBuf,
    manifest: CaptureManifest,
    limits: CaptureLimits,
    total_bytes: usize,
}

impl CaptureSession {
    pub fn create(
        capture_dir: &Path,
        case_id: &str,
        expected_result: ExpectedResult,
        limits: CaptureLimits,
    ) -> Result<Self, CaptureError> {
        validate_case_id(case_id)?;
        ensure_empty_capture_dir(capture_dir)?;
        let frames_dir = capture_dir.join("frames");
        fs::create_dir(&frames_dir)
            .map_err(|source| io_error("create capture directory", &frames_dir, source))?;
        let manifest_path = capture_dir.join(MANIFEST_FILE_NAME);
        let session = Self {
            manifest_path,
            manifest: CaptureManifest {
                schema_version: CAPTURE_SCHEMA_VERSION,
                case_id: case_id.to_owned(),
                expected_result,
                bounded_stop_reason: None,
                frames: Vec::new(),
            },
            limits,
            total_bytes: 0,
        };
        if let Err(error) = session.write_manifest() {
            let _ = fs::remove_dir(&frames_dir);
            return Err(error);
        }
        Ok(session)
    }

    pub fn record_frame(
        &mut self,
        direction: FrameDirection,
        timing_us: u64,
        evidence_source: EvidenceSource,
        mutation: bool,
        raw: &[u8],
    ) -> Result<Option<CaptureFrame>, CaptureError> {
        if self.manifest.bounded_stop_reason.is_some() {
            return Ok(None);
        }
        validate_frame_evidence(self.manifest.expected_result, &evidence_source, mutation)?;
        if self.manifest.frames.len() >= self.limits.max_frames {
            self.stop(CaptureStopReason::FrameCount)?;
            return Ok(None);
        }
        let sequence =
            u32::try_from(self.manifest.frames.len()).map_err(|_| CaptureError::FrameLimit {
                attempted: self.manifest.frames.len().saturating_add(1),
                limit: self.limits.max_frames,
            })?;
        if raw.len() > self.limits.max_frame_bytes {
            self.stop(CaptureStopReason::FrameBytes)?;
            return Ok(None);
        }
        let attempted_total = self
            .total_bytes
            .checked_add(raw.len())
            .ok_or(CaptureError::ByteCountOverflow)?;
        if attempted_total > self.limits.max_total_bytes {
            self.stop(CaptureStopReason::TotalBytes)?;
            return Ok(None);
        }

        let relative_path = PathBuf::from(format!("frames/{sequence:03}.bin"));
        let absolute_path = self
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&relative_path);
        write_new_frame(&absolute_path, raw)?;
        let frame = CaptureFrame {
            sequence,
            direction,
            file: relative_path,
            timing_us,
            evidence_source,
            mutation,
        };
        self.manifest.frames.push(frame.clone());
        self.total_bytes = attempted_total;
        if let Err(error) = self.write_manifest() {
            self.manifest.frames.pop();
            self.total_bytes = self.total_bytes.saturating_sub(raw.len());
            let _ = fs::remove_file(&absolute_path);
            return Err(error);
        }
        Ok(Some(frame))
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    fn stop(&mut self, reason: CaptureStopReason) -> Result<(), CaptureError> {
        let previous_reason = self.manifest.bounded_stop_reason.replace(reason);
        if let Err(error) = self.write_manifest() {
            self.manifest.bounded_stop_reason = previous_reason;
            return Err(error);
        }
        Ok(())
    }

    fn write_manifest(&self) -> Result<(), CaptureError> {
        let text =
            toml::to_string_pretty(&self.manifest).map_err(CaptureError::ManifestSerialize)?;
        fs::write(&self.manifest_path, text)
            .map_err(|source| io_error("write manifest", &self.manifest_path, source))
    }
}

fn ensure_empty_capture_dir(capture_dir: &Path) -> Result<(), CaptureError> {
    let mut entries = match fs::read_dir(capture_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(capture_dir)
                .map_err(|source| io_error("create capture directory", capture_dir, source))?;
            return Ok(());
        }
        Err(source) => return Err(io_error("read capture directory", capture_dir, source)),
    };
    match entries.next() {
        Some(Ok(_)) => Err(CaptureError::OutputDirectoryNotEmpty {
            path: capture_dir.display().to_string(),
        }),
        None => Ok(()),
        Some(Err(source)) => Err(io_error("read capture directory", capture_dir, source)),
    }
}

fn write_new_frame(path: &Path, raw: &[u8]) -> Result<(), CaptureError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                CaptureError::FrameAlreadyExists {
                    path: path.display().to_string(),
                }
            } else {
                io_error("create frame", path, source)
            }
        })?;
    file.write_all(raw)
        .map_err(|source| io_error("write frame", path, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync frame", path, source))
}
