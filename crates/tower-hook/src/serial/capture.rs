//! Serial traffic logging and controlled raw capture.

use std::path::PathBuf;
use std::time::Instant;

use druaga_tower_board::{
    CaptureError, CaptureLimits, CaptureSession, EvidenceSource, ExpectedResult, FrameDirection,
};

use super::SerialPort;
use crate::platform;

pub(crate) fn log_board_frame(direction: &str, raw: &[u8]) {
    log_record(format_frame_record("board-frame", None, direction, raw));
}

pub(crate) fn log_reader_frame(port: SerialPort, direction: &str, raw: &[u8]) {
    log_record(format_frame_record(
        "reader-frame",
        Some(port),
        direction,
        raw,
    ));
}

pub(crate) fn log_reader_read(port: SerialPort, expected: u32, result: usize) {
    log_record(format!(
        "reader-read port={} expected={expected} result={result}\n",
        port.name()
    ));
}

fn format_frame_record(
    kind: &str,
    port: Option<SerialPort>,
    direction: &str,
    raw: &[u8],
) -> String {
    let bytes = raw
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    match port {
        Some(port) => format!(
            "{kind} port={} direction={direction} bytes={bytes}\n",
            port.name()
        ),
        None => format!("{kind} direction={direction} bytes={bytes}\n"),
    }
}

fn log_record(mut record: String) {
    if record.ends_with('\n') {
        record.pop();
    }
    platform::log(&record);
}

pub(crate) struct SerialCapture {
    session: Option<CaptureSession>,
    started: Instant,
}

impl SerialCapture {
    pub(crate) fn new() -> Self {
        Self {
            session: None,
            started: Instant::now(),
        }
    }

    pub(crate) fn prepare(&mut self) -> Result<(), CaptureError> {
        if self.session.is_some() {
            return Ok(());
        }
        let Some(directory) = std::env::var_os("DRUAGA_TOWER_CAPTURE_DIR") else {
            return Ok(());
        };
        self.session = Some(CaptureSession::create(
            &PathBuf::from(directory),
            "tower-160-io-board-startup",
            ExpectedResult::ObservedFailure,
            CaptureLimits::default(),
        )?);
        self.started = Instant::now();
        Ok(())
    }

    pub(crate) fn record_board_request(&mut self, raw: &[u8]) -> Result<(), CaptureError> {
        self.record_board_frame(FrameDirection::TowerToBoard, raw)
    }

    pub(crate) fn record_board_response(&mut self, raw: &[u8]) -> Result<(), CaptureError> {
        self.record_board_frame(FrameDirection::BoardToTower, raw)
    }

    fn record_board_frame(
        &mut self,
        direction: FrameDirection,
        raw: &[u8],
    ) -> Result<(), CaptureError> {
        let timing_us = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        session.record_frame(
            direction,
            timing_us,
            EvidenceSource("controlled-wine-capture:02-06-io-board-startup".to_owned()),
            false,
            raw,
        )?;
        Ok(())
    }
}
