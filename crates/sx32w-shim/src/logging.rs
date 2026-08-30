//! Bounded process log for Sentinel and bootstrap diagnostics.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn log(message: &str) {
    let Ok(_guard) = LOG_LOCK.lock() else {
        return;
    };
    let path = std::env::var_os("DRUAGA_SX32W_LOG").unwrap_or_else(|| "sx32w-shim.log".into());
    let file = OpenOptions::new().create(true).append(true).open(path);
    if let Ok(mut file) = file {
        let current_length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let remaining = MAX_LOG_BYTES.saturating_sub(current_length) as usize;
        if remaining == 0 {
            return;
        }
        let mut record = message.as_bytes().to_vec();
        record.push(b'\n');
        record.truncate(remaining);
        let _written = file.write_all(&record);
    }
}
