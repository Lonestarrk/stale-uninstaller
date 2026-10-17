use core::ffi::c_void;

const COMPRESSION_FORMAT_XPRESS_HUFF: u16 = 4;
/// Upper bound on the stated uncompressed size, guards against corrupt headers.
const MAX_UNCOMPRESSED: u32 = 64 * 1024 * 1024;

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetCompressionWorkSpaceSize(format: u16, buf_ws: *mut u32, frag_ws: *mut u32) -> i32;
    fn RtlDecompressBufferEx(
        format: u16,
        out: *mut u8,
        out_len: u32,
        inp: *const u8,
        in_len: u32,
        final_len: *mut u32,
        workspace: *mut c_void,
    ) -> i32;
}

/// Decompresses a Windows 10+ "MAM\x04" (XPRESS Huffman) Prefetch file.
/// Data without the "MAM" magic is returned unchanged (older uncompressed .pf).
pub fn decompress_mam(data: &[u8]) -> Result<Vec<u8>, String> {
    if !data.starts_with(b"MAM") {
        return Ok(data.to_vec());
    }
    if data.len() < 8 {
        return Err("MAM header truncated".into());
    }
    if data[3] != 0x04 {
        return Err(format!("unsupported MAM compression type {:#04x}", data[3]));
    }
    let size = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if size > MAX_UNCOMPRESSED {
        return Err(format!("stated size {size} exceeds limit"));
    }
    let payload = &data[8..];
    let in_len = u32::try_from(payload.len()).map_err(|_| "payload too large".to_string())?;

    let (mut buf_ws, mut frag_ws) = (0u32, 0u32);
    // SAFETY: both out-pointers point to live, writable u32s.
    let status = unsafe { RtlGetCompressionWorkSpaceSize(COMPRESSION_FORMAT_XPRESS_HUFF, &mut buf_ws, &mut frag_ws) };
    if status != 0 {
        return Err(format!("RtlGetCompressionWorkSpaceSize failed: {status:#010x}"));
    }
    // Sized for the larger of the two requirements; u64 elements keep it 8-byte aligned.
    let ws_bytes = buf_ws.max(frag_ws) as usize;
    let mut workspace = vec![0u64; ws_bytes.div_ceil(8).max(1)];
    let mut out = vec![0u8; size as usize];
    let mut final_len = 0u32;
    // SAFETY: `out` holds `size` bytes, `payload` holds `in_len` bytes, and `workspace`
    // is at least `max(buf_ws, frag_ws)` bytes as requested by ntdll; all outlive the call.
    let status = unsafe {
        RtlDecompressBufferEx(
            COMPRESSION_FORMAT_XPRESS_HUFF,
            out.as_mut_ptr(),
            size,
            payload.as_ptr(),
            in_len,
            &mut final_len,
            workspace.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(format!("RtlDecompressBufferEx failed: {status:#010x}"));
    }
    if final_len != size {
        return Err(format!("decompressed {final_len} bytes, header states {size}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_rejects_truncated_header() {
        assert!(decompress_mam(b"MAM\x04\x10").is_err());
    }

    #[test]
    fn decompress_passes_through_uncompressed_data() {
        assert_eq!(decompress_mam(b"\x1e\0\0\0SCCA").unwrap(), b"\x1e\0\0\0SCCA");
    }
}
