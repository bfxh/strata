//! 交换格式的承重底座：确定性字节序列化（01-P6 对策的一半——导出即文件）。
//!
//! M0 先钉死"量化 i16 小端 + 头部"这一最底层格式及其**逐位确定性**；
//! png/exr/gltf/USD 封装在 M4 叠在此之上。

use strata_field::HeightTile;

pub const MAGIC: [u8; 4] = *b"SH16";
pub const VERSION: u8 = 1;

/// 头：magic | version | span(u32 LE) | halo(u32 LE)。
fn header(tile: &HeightTile) -> Vec<u8> {
    let mut h = Vec::with_capacity(13);
    h.extend_from_slice(&MAGIC);
    h.push(VERSION);
    h.extend_from_slice(&tile.span.to_le_bytes());
    h.extend_from_slice(&tile.halo.to_le_bytes());
    h
}

/// 写为确定性字节：量化 i16 逐位一致 ⇒ 同 tile 同字节。
pub fn write_height_i16_le(tile: &HeightTile) -> Vec<u8> {
    let mut out = header(tile);
    for v in tile.quantize_i16() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[derive(Debug)]
pub enum ReadError {
    BadMagic,
    Truncated,
}

/// 从字节恢复量化网格（返回 i16 网格与几何参数；浮点重建由调用方按语义做）。
pub fn read_height_i16_le(bytes: &[u8]) -> Result<(u32, u32, u32, Vec<i16>), ReadError> {
    if bytes.len() < 13 {
        return Err(ReadError::Truncated);
    }
    if bytes[0..4] != MAGIC || bytes[4] != VERSION {
        return Err(ReadError::BadMagic);
    }
    let span = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
    let halo = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
    let side = span + 2 * halo;
    let need = 13 + (side * side) as usize * 2;
    if bytes.len() < need {
        return Err(ReadError::Truncated);
    }
    let body = &bytes[13..need];
    let count = (side * side) as usize;
    let mut data = Vec::with_capacity(count);
    for i in 0..count {
        data.push(i16::from_le_bytes([body[i * 2], body[i * 2 + 1]]));
    }
    Ok((side, span, halo, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_geo::TilePos;

    fn tp() -> TilePos {
        TilePos {
            x: 0,
            y: 0,
            level: 0,
        }
    }

    #[test]
    fn write_read_roundtrip_bitwise() {
        let mut t = HeightTile::new(tp(), 2, 1, 0.0);
        t.set(0, 0, 0.5);
        t.set(3, 3, -0.25);
        let bytes = write_height_i16_le(&t);
        let (side, span, halo, data) = read_height_i16_le(&bytes).unwrap();
        assert_eq!((span, halo), (2, 1));
        assert_eq!(side, 4);
        assert_eq!(data, t.quantize_i16());
    }

    #[test]
    fn encoding_is_deterministic() {
        let t = HeightTile::new(tp(), 4, 2, 0.123);
        assert_eq!(write_height_i16_le(&t), write_height_i16_le(&t));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = write_height_i16_le(&HeightTile::new(tp(), 1, 0, 0.0));
        bytes[0] = b'X';
        assert!(matches!(
            read_height_i16_le(&bytes),
            Err(ReadError::BadMagic)
        ));
    }

    #[test]
    fn rejects_truncation() {
        assert!(matches!(
            read_height_i16_le(&[0; 5]),
            Err(ReadError::Truncated)
        ));
    }
}
