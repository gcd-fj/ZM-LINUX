use std::io::{Read, Write};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use zm_core::{Result, ZmError};

const SYMBOL_CLASS: u16 = 76;
const SHOW_FRAME: u16 = 1;
const DO_ABC: u16 = 82;

pub fn inject_bridge(source: &[u8], abc: &[u8], class_name: &str) -> Result<Vec<u8>> {
    if source.len() < 12 {
        return Err(ZmError::Asset("SWF文件过短".into()));
    }
    let version = source[3];
    let mut body = match &source[..3] {
        b"FWS" => source[8..].to_vec(),
        b"CWS" => {
            let mut decoded = Vec::new();
            ZlibDecoder::new(&source[8..])
                .read_to_end(&mut decoded)
                .map_err(|e| ZmError::Asset(format!("解压SWF失败：{e}")))?;
            decoded
        }
        b"ZWS" => decode_zws(source)?,
        _ => return Err(ZmError::Asset("无效SWF签名".into())),
    };

    let tags_start = frame_header_len(&body)?;
    let mut cursor = tags_start;
    let mut replacement = None;
    while cursor + 2 <= body.len() {
        let record = u16::from_le_bytes([body[cursor], body[cursor + 1]]);
        let code = record >> 6;
        let short_len = (record & 0x3f) as usize;
        let (header_len, body_len) = if short_len == 0x3f {
            if cursor + 6 > body.len() {
                return Err(ZmError::Asset("SWF长标签头损坏".into()));
            }
            (
                6,
                u32::from_le_bytes([
                    body[cursor + 2],
                    body[cursor + 3],
                    body[cursor + 4],
                    body[cursor + 5],
                ]) as usize,
            )
        } else {
            (2, short_len)
        };
        let start = cursor + header_len;
        let end = start
            .checked_add(body_len)
            .ok_or_else(|| ZmError::Asset("SWF标签长度溢出".into()))?;
        if end > body.len() {
            return Err(ZmError::Asset("SWF标签越界".into()));
        }
        if code == SYMBOL_CLASS {
            replacement = Some(rewrite_symbol_class(&body[start..end], class_name)?);
            let replacement = replacement.as_ref().unwrap();
            let mut tag = Vec::with_capacity(replacement.len() + 6);
            if replacement.len() < 0x3f {
                tag.extend_from_slice(
                    &((SYMBOL_CLASS << 6) | replacement.len() as u16).to_le_bytes(),
                );
            } else {
                tag.extend_from_slice(&((SYMBOL_CLASS << 6) | 0x3f).to_le_bytes());
                tag.extend_from_slice(&(replacement.len() as u32).to_le_bytes());
            }
            tag.extend_from_slice(replacement);
            body.splice(cursor..end, tag);
            break;
        }
        cursor = end;
        if code == 0 {
            break;
        }
    }
    if replacement.is_none() {
        return Err(ZmError::Asset("SWF中没有SymbolClass标签".into()));
    }

    let tags_start = frame_header_len(&body)?;
    let insertion = first_tag_offset(&body, tags_start, SHOW_FRAME)?
        .ok_or_else(|| ZmError::Asset("SWF中没有首帧标签".into()))?;
    body.splice(insertion..insertion, do_abc_tag(abc));

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&body)
        .map_err(|e| ZmError::Asset(format!("压缩SWF失败：{e}")))?;
    let compressed = encoder
        .finish()
        .map_err(|e| ZmError::Asset(format!("压缩SWF失败：{e}")))?;
    let mut output = Vec::with_capacity(compressed.len() + 8);
    output.extend_from_slice(b"CWS");
    output.push(version);
    output.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
    output.extend_from_slice(&compressed);
    Ok(output)
}

fn first_tag_offset(body: &[u8], mut cursor: usize, wanted: u16) -> Result<Option<usize>> {
    while cursor + 2 <= body.len() {
        let record = u16::from_le_bytes([body[cursor], body[cursor + 1]]);
        let code = record >> 6;
        if code == wanted {
            return Ok(Some(cursor));
        }
        let short_len = (record & 0x3f) as usize;
        let (header_len, body_len) = if short_len == 0x3f {
            if cursor + 6 > body.len() {
                return Err(ZmError::Asset("SWF长标签头损坏".into()));
            }
            (
                6,
                u32::from_le_bytes([
                    body[cursor + 2],
                    body[cursor + 3],
                    body[cursor + 4],
                    body[cursor + 5],
                ]) as usize,
            )
        } else {
            (2, short_len)
        };
        cursor = cursor
            .checked_add(header_len + body_len)
            .ok_or_else(|| ZmError::Asset("SWF标签长度溢出".into()))?;
        if cursor > body.len() {
            return Err(ZmError::Asset("SWF标签越界".into()));
        }
        if code == 0 {
            break;
        }
    }
    Ok(None)
}

fn do_abc_tag(abc: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(abc.len() + 14);
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload.extend_from_slice(b"ZM-LINUX session bridge\0");
    payload.extend_from_slice(abc);
    let mut tag = Vec::with_capacity(payload.len() + 6);
    tag.extend_from_slice(&((DO_ABC << 6) | 0x3f).to_le_bytes());
    tag.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    tag.extend_from_slice(&payload);
    tag
}

fn decode_zws(source: &[u8]) -> Result<Vec<u8>> {
    if source.len() < 17 {
        return Err(ZmError::Asset("LZMA SWF头损坏".into()));
    }
    let file_len = u32::from_le_bytes([source[4], source[5], source[6], source[7]]) as u64;
    if file_len < 8 {
        return Err(ZmError::Asset("LZMA SWF长度无效".into()));
    }
    let mut lzma_stream = Vec::with_capacity(source.len() + 8);
    lzma_stream.extend_from_slice(&source[12..17]);
    lzma_stream.extend_from_slice(&(file_len - 8).to_le_bytes());
    lzma_stream.extend_from_slice(&source[17..]);
    let mut decoded = Vec::with_capacity((file_len - 8) as usize);
    lzma_rs::lzma_decompress(&mut lzma_stream.as_slice(), &mut decoded)
        .map_err(|e| ZmError::Asset(format!("解压LZMA SWF失败：{e}")))?;
    Ok(decoded)
}

fn frame_header_len(body: &[u8]) -> Result<usize> {
    let first = *body
        .first()
        .ok_or_else(|| ZmError::Asset("SWF缺少RECT".into()))?;
    let nbits = (first >> 3) as usize;
    let rect_bytes = (5 + nbits * 4).div_ceil(8);
    let length = rect_bytes + 4;
    if length > body.len() {
        return Err(ZmError::Asset("SWF帧头越界".into()));
    }
    Ok(length)
}

fn rewrite_symbol_class(tag: &[u8], class_name: &str) -> Result<Vec<u8>> {
    if tag.len() < 2 {
        return Err(ZmError::Asset("SymbolClass标签过短".into()));
    }
    let count = u16::from_le_bytes([tag[0], tag[1]]);
    let mut input = 2;
    let mut output = Vec::with_capacity(tag.len());
    output.extend_from_slice(&count.to_le_bytes());
    let mut replaced = false;
    for _ in 0..count {
        if input + 2 > tag.len() {
            return Err(ZmError::Asset("SymbolClass条目损坏".into()));
        }
        let id = u16::from_le_bytes([tag[input], tag[input + 1]]);
        input += 2;
        let nul = tag[input..]
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| ZmError::Asset("SymbolClass名称未终止".into()))?;
        let old_name = &tag[input..input + nul];
        input += nul + 1;
        output.extend_from_slice(&id.to_le_bytes());
        if id == 0 {
            output.extend_from_slice(class_name.as_bytes());
            replaced = true;
        } else {
            output.extend_from_slice(old_name);
        }
        output.push(0);
    }
    if !replaced {
        return Err(ZmError::Asset("SymbolClass中没有根文档类".into()));
    }
    output.extend_from_slice(&tag[input..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewrites_only_root_entry() {
        let mut tag = vec![2, 0, 0, 0];
        tag.extend_from_slice(b"Main\0");
        tag.extend_from_slice(&7_u16.to_le_bytes());
        tag.extend_from_slice(b"Asset\0");
        let result = rewrite_symbol_class(&tag, "Preload").unwrap();
        assert!(result.windows(8).any(|w| w == b"Preload\0"));
        assert!(result.windows(6).any(|w| w == b"Asset\0"));
        assert!(!result.windows(5).any(|w| w == b"Main\0"));
    }

    #[test]
    fn bridge_abc_tag_preserves_payload() {
        let tag = do_abc_tag(b"ABC bytes");
        assert_eq!(u16::from_le_bytes([tag[0], tag[1]]) >> 6, DO_ABC);
        assert!(tag.windows(9).any(|window| window == b"ABC bytes"));
        assert!(
            tag.windows(24)
                .any(|window| window == b"ZM-LINUX session bridge\0")
        );
    }

    #[test]
    fn patches_document_class_and_inserts_bridge_before_first_frame() {
        let mut body = vec![0x08, 0x00, 0x00, 0x18, 0x01, 0x00];
        let mut symbols = vec![1, 0, 0, 0];
        symbols.extend_from_slice(b"Preload\0");
        body.extend_from_slice(&((SYMBOL_CLASS << 6) | symbols.len() as u16).to_le_bytes());
        body.extend_from_slice(&symbols);
        body.extend_from_slice(&(SHOW_FRAME << 6).to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());

        let mut source = Vec::new();
        source.extend_from_slice(b"FWS");
        source.push(10);
        source.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        source.extend_from_slice(&body);

        let output = inject_bridge(&source, b"bridge-abc", "ZmLinuxZm4Bridge").unwrap();
        let mut decoded = Vec::new();
        ZlibDecoder::new(&output[8..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert!(
            decoded
                .windows(b"ZmLinuxZm4Bridge".len())
                .any(|window| window == b"ZmLinuxZm4Bridge")
        );
        let abc_at = decoded
            .windows(10)
            .position(|window| window == b"bridge-abc")
            .unwrap();
        let show_frame_at =
            first_tag_offset(&decoded, frame_header_len(&decoded).unwrap(), SHOW_FRAME)
                .unwrap()
                .unwrap();
        assert!(abc_at < show_frame_at);
    }
}
