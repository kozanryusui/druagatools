use std::io::Cursor;

use binrw::BinRead;
use thiserror::Error;

const HEADER_SIZE: usize = 4;

#[cfg(test)]
#[derive(BinRead, Clone, Debug, Eq, PartialEq)]
#[br(big)]
pub(crate) struct Frame {
    pub(crate) message_type: u16,
    payload_length: u16,
    #[br(count = payload_length)]
    pub(crate) payload: Vec<u8>,
}

pub(crate) fn read<T>(bytes: &[u8]) -> Result<T, FrameError>
where
    for<'args> T: BinRead<Args<'args> = ()>,
{
    if bytes.len() < HEADER_SIZE {
        return Err(FrameError::HeaderLength);
    }
    let declared = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    let actual = bytes.len() - HEADER_SIZE;
    if actual != declared {
        return Err(FrameError::PayloadLength { declared, actual });
    }

    let mut input = Cursor::new(bytes);
    let value = T::read_be(&mut input).map_err(|error| FrameError::Parse(error.to_string()))?;
    if input.position() != bytes.len() as u64 {
        return Err(FrameError::TrailingBytes);
    }
    Ok(value)
}

#[cfg(test)]
impl Frame {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, FrameError> {
        read(bytes)
    }
}

#[derive(Debug, Error)]
pub(crate) enum FrameError {
    #[error("frame header is incomplete")]
    HeaderLength,
    #[error("frame declares {declared} payload bytes but contains {actual}")]
    PayloadLength { declared: usize, actual: usize },
    #[error("frame cannot be parsed: {0}")]
    Parse(String),
    #[error("frame contains trailing bytes")]
    TrailingBytes,
}
