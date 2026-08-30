use super::{DATA_BYTES_PER_PAGE, Ps2McError};

const MAGIC: &[u8; 28] = b"Sony PS2 Memory Card Format ";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Superblock {
    pub version: [u8; 12],
    pub page_len: u16,
    pub pages_per_cluster: u16,
    pub pages_per_block: u16,
    pub clusters_per_card: u32,
    pub alloc_offset: u32,
    pub alloc_end: u32,
    pub rootdir_cluster: u32,
    pub backup_block1: u32,
    pub backup_block2: u32,
    pub first_indirect_fat_cluster: u32,
}

impl Superblock {
    pub fn parse(data: &[u8]) -> Result<Self, Ps2McError> {
        require_range(data, 0, 0x54)?;
        if data.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(unsupported("magic", 0));
        }

        let mut version = [0_u8; 12];
        let version_bytes = data.get(0x1c..0x28).ok_or(Ps2McError::ReadOutOfRange {
            offset: 0x1c,
            length: 12,
            input_len: data.len() as u64,
        })?;
        version.copy_from_slice(version_bytes);

        let result = Self {
            version,
            page_len: read_u16(data, 0x28)?,
            pages_per_cluster: read_u16(data, 0x2a)?,
            pages_per_block: read_u16(data, 0x2c)?,
            clusters_per_card: read_u32(data, 0x30)?,
            alloc_offset: read_u32(data, 0x34)?,
            alloc_end: read_u32(data, 0x38)?,
            rootdir_cluster: read_u32(data, 0x3c)?,
            backup_block1: read_u32(data, 0x40)?,
            backup_block2: read_u32(data, 0x44)?,
            first_indirect_fat_cluster: read_u32(data, 0x50)?,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<(), Ps2McError> {
        if usize::from(self.page_len) != DATA_BYTES_PER_PAGE {
            return Err(unsupported("page_len", u64::from(self.page_len)));
        }
        if self.pages_per_cluster != 2 {
            return Err(unsupported(
                "pages_per_cluster",
                u64::from(self.pages_per_cluster),
            ));
        }
        if self.pages_per_block != 16 {
            return Err(unsupported(
                "pages_per_block",
                u64::from(self.pages_per_block),
            ));
        }
        if self.clusters_per_card == 0 {
            return Err(unsupported("clusters_per_card", 0));
        }
        if self.alloc_end == 0 {
            return Err(unsupported("allocation_range", u64::from(self.alloc_end)));
        }
        let allocation_end = self
            .alloc_offset
            .checked_add(self.alloc_end)
            .ok_or(Ps2McError::OffsetOverflow)?;
        if allocation_end > self.clusters_per_card {
            return Err(unsupported("allocation_range", u64::from(allocation_end)));
        }
        if self.rootdir_cluster >= self.alloc_end {
            return Err(unsupported(
                "rootdir_cluster",
                u64::from(self.rootdir_cluster),
            ));
        }
        if self.first_indirect_fat_cluster >= self.alloc_offset {
            return Err(unsupported(
                "first_indirect_fat_cluster",
                u64::from(self.first_indirect_fat_cluster),
            ));
        }
        Ok(())
    }
}

fn require_range(data: &[u8], offset: usize, length: usize) -> Result<(), Ps2McError> {
    let end = offset
        .checked_add(length)
        .ok_or(Ps2McError::OffsetOverflow)?;
    if end > data.len() {
        return Err(Ps2McError::ReadOutOfRange {
            offset: offset as u64,
            length: length as u64,
            input_len: data.len() as u64,
        });
    }
    Ok(())
}

pub(crate) fn read_u16(data: &[u8], offset: usize) -> Result<u16, Ps2McError> {
    require_range(data, offset, 2)?;
    let bytes = data
        .get(offset..offset + 2)
        .ok_or(Ps2McError::ReadOutOfRange {
            offset: offset as u64,
            length: 2,
            input_len: data.len() as u64,
        })?;
    let array = <[u8; 2]>::try_from(bytes).map_err(|_| Ps2McError::ReadOutOfRange {
        offset: offset as u64,
        length: 2,
        input_len: data.len() as u64,
    })?;
    Ok(u16::from_le_bytes(array))
}

pub(crate) fn read_u32(data: &[u8], offset: usize) -> Result<u32, Ps2McError> {
    require_range(data, offset, 4)?;
    let bytes = data
        .get(offset..offset + 4)
        .ok_or(Ps2McError::ReadOutOfRange {
            offset: offset as u64,
            length: 4,
            input_len: data.len() as u64,
        })?;
    let array = <[u8; 4]>::try_from(bytes).map_err(|_| Ps2McError::ReadOutOfRange {
        offset: offset as u64,
        length: 4,
        input_len: data.len() as u64,
    })?;
    Ok(u32::from_le_bytes(array))
}

fn unsupported(field: &'static str, value: u64) -> Ps2McError {
    Ps2McError::UnsupportedDongleGeometry { field, value }
}
