//! Typed Tower board protocol and capture support.

pub mod capture;
pub mod protocol;

pub use capture::{
    CaptureError, CaptureFrame, CaptureLimits, CaptureManifest, CaptureSession, CaptureStopReason,
    EvidenceSource, ExpectedResult, FrameDirection, MAX_CAPTURE_BYTES, MAX_CAPTURE_FRAMES,
    MAX_FRAME_BYTES, load_capture_manifest, load_replay_frames,
};
pub use protocol::{
    BOARD_REQUEST_BYTES, BOARD_RESPONSE_BYTES, BoardClientRequest, BoardCommandId, BoardError,
    BoardExchangeMode, BoardProtocol, BoardResponse, BoardResponseHeader, OperatorAction,
    OperatorInputEvent, OperatorInputState, ProtocolState,
};
