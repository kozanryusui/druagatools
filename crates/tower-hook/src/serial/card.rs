//! File-backed Tower integrated circuit (IC) cards.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use blowfish::Blowfish;
use blowfish::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use md5::{Digest, Md5};
use thiserror::Error;

pub const CARD_IMAGE_SIZE: usize = 0x300;
const FACTORY_HEADER_SIZE: usize = 0x30;
const FACTORY_VERSION: u16 = 0x0110;
const KEYFILE_IDENTIFIER: &[u8; 6] = b"V32401";
const FACTORY_GENERATION: u32 = 0x7fff_ffff;

// Keep the key bytes visible in this file. This macro converts each pair of
// hexadecimal digits at compile time and rejects a key with the wrong length.
macro_rules! hex {
    ($value:literal) => {{
        const TEXT: &[u8] = $value.as_bytes();
        const fn digit(value: u8) -> u8 {
            match value {
                b'0'..=b'9' => value - b'0',
                b'a'..=b'f' => value - b'a' + 10,
                _ => panic!("invalid hexadecimal key digit"),
            }
        }
        const fn decode() -> [u8; 56] {
            assert!(TEXT.len() == 112, "invalid Blowfish key length");
            let mut output = [0; 56];
            let mut index = 0;
            while index < output.len() {
                output[index] = digit(TEXT[index * 2]) * 16 + digit(TEXT[index * 2 + 1]);
                index += 1;
            }
            output
        }
        decode()
    }};
}

/// The first 56 bytes of each `V32401` key record.
///
/// Ghidra shows that Tower hashes the 16-byte card identifier with Message
/// Digest 5 (MD5). It adds the 16 digest bytes modulo 5. The result selects one
/// of these five 448-bit Blowfish keys. The records are 0x80 bytes apart in the
/// original `NMCK` key file.
const CARD_KEYS: [[u8; 56]; 5] = [
    hex!(
        "18bb438014b93954ef729fd856d0f1ad9bb034d63095da6c6e7afea8437bcace859754adf9562eb1304ca7d7aede6b88a7028a817aa3c983"
    ),
    hex!(
        "c3dd24ed05832be2c14327d0cc87c78400eaf91e4a0d2e5429af5a734973f750b5506fe04a823900b3f112be4da64aff346fd30885206c1e"
    ),
    hex!(
        "8b703a531d7200cc24eb6d3bce4d9dc9a7fe2e25bbdafff16f8db8449d8087e4bfb227944d9f59c50d3171d521a7c603fe5aaf07999ef70e"
    ),
    hex!(
        "05f1796d01d7249638ab1686c710a70a458b85fdbfd5ae9436a683b5f97fc330daeee72ea51d5a716cfc1e9497cfec4f98e8fefe422e4c0e"
    ),
    hex!(
        "d770dee1114dada36fa60b1c581673deb03dddea7fea732a5634e8eca0db87ebd19bc7ea9c8419052b14417fc8205a41e76e5c3f32ef6617"
    ),
];

#[derive(Debug, Error)]
pub enum CardError {
    #[error("card number {0} is outside the range 0 through 9")]
    Number(u8),
    #[error("card file {path} has {actual} bytes; expected {CARD_IMAGE_SIZE}")]
    Size { path: PathBuf, actual: usize },
    #[error("card file operation failed for {path}: {source}")]
    File {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the Tower Blowfish key is invalid")]
    BlowfishKey,
    #[error("the card header has an invalid layout")]
    Header,
}

#[derive(Debug)]
pub struct MountedCard {
    number: u8,
    path: PathBuf,
    image: [u8; CARD_IMAGE_SIZE],
}

impl MountedCard {
    /// Load `cardN.bin`. Create a factory-empty image when the file is absent.
    pub fn load_or_create(directory: &Path, number: u8) -> Result<Self, CardError> {
        let path = card_path(directory, number)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(directory).map_err(|source| CardError::File {
                    path: directory.to_owned(),
                    source,
                })?;
                let image = factory_empty_image(number)?;
                persist_image(&path, &image)?;
                image.to_vec()
            }
            Err(source) => return Err(CardError::File { path, source }),
        };
        let actual = bytes.len();
        let image = bytes.try_into().map_err(|_| CardError::Size {
            path: path.clone(),
            actual,
        })?;
        Ok(Self {
            number,
            path,
            image,
        })
    }

    pub const fn number(&self) -> u8 {
        self.number
    }

    pub fn read_block<const N: usize>(&self, block: u8) -> Option<[u8; N]> {
        let start = usize::from(block).checked_mul(N)?;
        let end = start.checked_add(N)?;
        self.image.get(start..end)?.try_into().ok()
    }

    /// Save one complete reader block before the command is acknowledged.
    pub fn write_block<const N: usize>(
        &mut self,
        block: u8,
        data: [u8; N],
    ) -> Result<bool, CardError> {
        let Some(start) = usize::from(block).checked_mul(N) else {
            return Ok(false);
        };
        let Some(end) = start.checked_add(N) else {
            return Ok(false);
        };
        let Some(target) = self.image.get_mut(start..end) else {
            return Ok(false);
        };
        let mut old = [0; N];
        old.copy_from_slice(target);
        target.copy_from_slice(&data);
        if let Err(error) = persist_image(&self.path, &self.image) {
            self.image[start..end].copy_from_slice(&old);
            return Err(error);
        }
        Ok(true)
    }

    /// Apply the generation commit used by reader control operation 7.
    ///
    /// Tower writes bytes 0x30 through 0x2ff, then sends selector 2 with value
    /// one. The physical reader decrements the redundant generation fields. It
    /// does not receive the 0x30-byte header through a normal block write.
    pub fn decrement_generation(&mut self, amount: u32) -> Result<bool, CardError> {
        let generation = u32::from_le_bytes(
            self.image[0x20..0x24]
                .try_into()
                .map_err(|_| CardError::Header)?,
        );
        let complement = u32::from_le_bytes(
            self.image[0x24..0x28]
                .try_into()
                .map_err(|_| CardError::Header)?,
        );
        let repeated = u32::from_le_bytes(
            self.image[0x28..0x2c]
                .try_into()
                .map_err(|_| CardError::Header)?,
        );
        if generation != repeated || generation != !complement {
            return Ok(false);
        }

        let previous = self.image;
        let generation = generation.wrapping_sub(amount);
        self.image[0x20..0x24].copy_from_slice(&generation.to_le_bytes());
        self.image[0x24..0x28].copy_from_slice(&(!generation).to_le_bytes());
        self.image[0x28..0x2c].copy_from_slice(&generation.to_le_bytes());
        if let Err(error) = persist_image(&self.path, &self.image) {
            self.image = previous;
            return Err(error);
        }
        Ok(true)
    }
}

fn card_path(directory: &Path, number: u8) -> Result<PathBuf, CardError> {
    if number > 9 {
        return Err(CardError::Number(number));
    }
    Ok(directory.join(format!("card{number}.bin")))
}

/// Create the exact 0x300-byte image that Tower classifies as factory-empty.
///
/// An all-zero image is invalid. Tower requires this header before it creates
/// and writes the first logical card record.
fn factory_empty_image(number: u8) -> Result<[u8; CARD_IMAGE_SIZE], CardError> {
    let identifier = *factory_identifier(number)?;
    let mut image = [0_u8; CARD_IMAGE_SIZE];
    image[..0x10].copy_from_slice(&identifier);
    image[0x10..0x12].copy_from_slice(&FACTORY_VERSION.to_le_bytes());
    image[0x12..0x18].copy_from_slice(KEYFILE_IDENTIFIER);
    image[0x18..0x20].copy_from_slice(&encrypt_factory_header_data(&identifier)?);
    image[0x20..0x24].copy_from_slice(&FACTORY_GENERATION.to_le_bytes());
    image[0x24..0x28].copy_from_slice(&(!FACTORY_GENERATION).to_le_bytes());
    image[0x28..0x2c].copy_from_slice(&FACTORY_GENERATION.to_le_bytes());
    debug_assert!(image[FACTORY_HEADER_SIZE..].iter().all(|byte| *byte == 0));
    Ok(image)
}

fn factory_identifier(number: u8) -> Result<&'static [u8; 16], CardError> {
    const IDENTIFIERS: [[u8; 16]; 10] = [
        *b"DRUAGA-CARD-0000",
        *b"DRUAGA-CARD-0001",
        *b"DRUAGA-CARD-0002",
        *b"DRUAGA-CARD-0003",
        *b"DRUAGA-CARD-0004",
        *b"DRUAGA-CARD-0005",
        *b"DRUAGA-CARD-0006",
        *b"DRUAGA-CARD-0007",
        *b"DRUAGA-CARD-0008",
        *b"DRUAGA-CARD-0009",
    ];
    IDENTIFIERS
        .get(usize::from(number))
        .ok_or(CardError::Number(number))
}

fn encrypt_factory_header_data(identifier: &[u8; 16]) -> Result<[u8; 8], CardError> {
    let digest = Md5::digest(identifier);
    let key_index = digest
        .iter()
        .fold(0_usize, |sum, byte| sum + usize::from(*byte))
        % CARD_KEYS.len();
    let cipher: Blowfish =
        Blowfish::new_from_slice(&CARD_KEYS[key_index]).map_err(|_| CardError::BlowfishKey)?;

    // The clear block contains seven zero data bytes and a zero XOR checksum.
    let mut block = GenericArray::from([0_u8; 8]);
    cipher.encrypt_block(&mut block);
    let mut protected: [u8; 8] = block.into();

    // Tower stores each Blowfish 32-bit word in little-endian byte order.
    // RustCrypto emits the standard big-endian representation.
    protected[..4].reverse();
    protected[4..].reverse();
    Ok(protected)
}

fn persist_image(path: &Path, image: &[u8; CARD_IMAGE_SIZE]) -> Result<(), CardError> {
    let temporary = path.with_extension("bin.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|source| CardError::File {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(image).map_err(|source| CardError::File {
        path: temporary.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| CardError::File {
        path: temporary.clone(),
        source,
    })?;
    replace_file(&temporary, path).map_err(|source| CardError::File {
        path: path.to_owned(),
        source,
    })
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Both paths are valid, null-terminated UTF-16 strings.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
