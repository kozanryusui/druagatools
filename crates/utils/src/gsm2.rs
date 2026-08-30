use std::io::{self, Write};

use thiserror::Error;

const PALETTE_SIZE: usize = 256 * 4;
const FOOTER_SIZE: usize = 32;
const TRAILER_SIZE: usize = 16;

#[derive(Debug, Error)]
pub enum Error {
    #[error("the GSM2 file is too short")]
    TooShort,
    #[error("the GSM2 footer signature is not present")]
    BadSignature,
    #[error("unsupported GSM2 format: {0} bits per pixel")]
    UnsupportedDepth(u16),
    #[error("the GSM2 pixel data length does not match {width}x{height}")]
    BadPixelLength { width: u16, height: u16 },
    #[error("PNG encoding failed: {0}")]
    Png(#[from] png::EncodingError),
    #[error("output failed: {0}")]
    Io(#[from] io::Error),
}

pub struct Image<'a> {
    pub width: u16,
    pub height: u16,
    palette: &'a [u8],
    pixels: &'a [u8],
}

impl<'a> Image<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < PALETTE_SIZE + FOOTER_SIZE + TRAILER_SIZE {
            return Err(Error::TooShort);
        }

        let footer = data.len() - FOOTER_SIZE;
        if &data[footer..footer + 4] != b"GSM2" {
            return Err(Error::BadSignature);
        }

        let width = u16::from_le_bytes([data[footer + 6], data[footer + 7]]);
        let height = u16::from_le_bytes([data[footer + 8], data[footer + 9]]);
        let depth = u16::from_le_bytes([data[footer + 10], data[footer + 11]]);
        if depth != 8 {
            return Err(Error::UnsupportedDepth(depth));
        }

        let pixel_len = usize::from(width) * usize::from(height);
        let pixel_end = data.len() - FOOTER_SIZE - TRAILER_SIZE;
        if pixel_end != PALETTE_SIZE + pixel_len {
            return Err(Error::BadPixelLength { width, height });
        }

        Ok(Self {
            width,
            height,
            palette: &data[..PALETTE_SIZE],
            pixels: &data[PALETTE_SIZE..pixel_end],
        })
    }

    pub fn write_png(&self, output: impl Write) -> Result<(), Error> {
        let mut encoder = png::Encoder::new(output, u32::from(self.width), u32::from(self.height));
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;

        let rgba = self.rgba_pixels();
        writer.write_image_data(&rgba)?;
        Ok(())
    }

    pub fn rgba_pixels(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.pixels.len() * 4);
        for y in 0..usize::from(self.height) {
            for x in 0..usize::from(self.width) {
                let index = self.pixels[swizzled_pixel_offset(x, y, usize::from(self.width))];
                let palette_index = unswizzled_palette_index(usize::from(index));
                let offset = palette_index * 4;
                rgba.extend_from_slice(&self.palette[offset..offset + 3]);
                rgba.push(self.palette[offset + 3].saturating_mul(2));
            }
        }
        rgba
    }
}

fn swizzled_pixel_offset(x: usize, y: usize, width: usize) -> usize {
    let block = (y & !0x0f) * width + (x & !0x0f) * 2;
    let swap = (((y + 2) >> 2) & 1) * 4;
    let row = (((y & !3) >> 1) + (y & 1)) & 7;
    let column = row * width * 2 + ((x + swap) & 7) * 4;
    let byte = ((y >> 1) & 1) + ((x >> 2) & 2);
    block + column + byte
}

fn unswizzled_palette_index(index: usize) -> usize {
    (index & !0x18) | ((index & 0x08) << 1) | ((index & 0x10) >> 1)
}
