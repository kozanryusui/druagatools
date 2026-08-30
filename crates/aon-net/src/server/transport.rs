use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

pub(super) async fn read_frame(stream: &mut TcpStream) -> Result<Option<Vec<u8>>, std::io::Error> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let payload_length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let mut frame = Vec::with_capacity(header.len() + payload_length);
    frame.extend_from_slice(&header);
    frame.resize(header.len() + payload_length, 0);
    stream.read_exact(&mut frame[header.len()..]).await?;
    Ok(Some(frame))
}
