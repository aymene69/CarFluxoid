//! Pure-Rust port of `tooling/waiting_iphone_config.py`.
//!
//! Rewrites x264 High CABAC IDR Annex-B pictures so their slice headers match
//! the iPhone CarPlay SPS/PPS (POC type 2) without re-encoding the macroblock
//! payload. The Uconnect VP2 AA decoder locks onto the first SPS/PPS of a
//! session, so every dashboard picture must share CarPlay's exact codec config.

use std::fmt;

#[derive(Debug)]
pub struct AlignError(pub String);

impl fmt::Display for AlignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AlignError {}

/// Iterate Annex-B NAL units (without start codes).
pub fn iter_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, &s) in starts.iter().enumerate() {
        let mut end = if idx + 1 < starts.len() {
            starts[idx + 1] - 3
        } else {
            data.len()
        };
        while end > s && data[end - 1] == 0 {
            end -= 1;
        }
        out.push(&data[s..end]);
    }
    out
}

fn unescape_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn escape_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut zeros = 0u32;
    for &b in data {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn u(&mut self, n: usize) -> Result<u32, AlignError> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte_idx = self.bit >> 3;
            if byte_idx >= self.data.len() {
                return Err(AlignError("unexpected EOF in H.264 bitstream".into()));
            }
            let b = (self.data[byte_idx] >> (7 - (self.bit & 7))) & 1;
            v = (v << 1) | u32::from(b);
            self.bit += 1;
        }
        Ok(v)
    }

    fn ue(&mut self) -> Result<u32, AlignError> {
        let mut zeros = 0usize;
        while self.u(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(AlignError("invalid Exp-Golomb code".into()));
            }
        }
        if zeros == 0 {
            Ok(0)
        } else {
            Ok(((1u32 << zeros) - 1) + self.u(zeros)?)
        }
    }

    fn se(&mut self) -> Result<i32, AlignError> {
        let k = self.ue()?;
        if k & 1 != 0 {
            Ok(((k + 1) / 2) as i32)
        } else {
            Ok(-((k / 2) as i32))
        }
    }
}

struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bits: Vec::with_capacity(128),
        }
    }

    fn u(&mut self, n: usize, v: u32) {
        for k in 0..n {
            self.bits.push(((v >> (n - 1 - k)) & 1) as u8);
        }
    }

    fn ue(&mut self, v: u32) {
        let val = v + 1;
        let bits = (32 - val.leading_zeros()) as usize;
        self.u(bits - 1, 0);
        self.u(bits, val);
    }

    fn se(&mut self, v: i32) {
        let code = if v > 0 {
            (2 * v - 1) as u32
        } else {
            (-2 * v) as u32
        };
        self.ue(code);
    }

    fn finish_bytes(self) -> Result<Vec<u8>, AlignError> {
        if self.bits.len() % 8 != 0 {
            return Err(AlignError("unaligned H.264 slice header".into()));
        }
        let mut out = Vec::with_capacity(self.bits.len() / 8);
        for chunk in self.bits.chunks_exact(8) {
            let mut b = 0u8;
            for &bit in chunk {
                b = (b << 1) | bit;
            }
            out.push(b);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy)]
struct SpsParams {
    log2_frame_num: usize,
    width_mbs: u32,
    height_mbs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpsParams {
    pub cabac: u32,
    pub init_qp: i32,
    pub chroma: i32,
    pub second: i32,
    pub deblock: u32,
    pub constrained: u32,
    pub t8: u32,
}

fn is_high_profile(profile: u32) -> bool {
    matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn parse_sps(nal: &[u8]) -> Result<SpsParams, AlignError> {
    if nal.is_empty() {
        return Err(AlignError("empty SPS".into()));
    }
    let rbsp = unescape_rbsp(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let profile = r.u(8)?;
    r.u(16)?;
    r.ue()?;
    if is_high_profile(profile) {
        if r.ue()? != 1 || r.ue()? != 0 || r.ue()? != 0 || r.u(1)? != 0 || r.u(1)? != 0 {
            return Err(AlignError("unsupported chroma/bit depth/scaling in SPS".into()));
        }
    }
    let log2_frame_num = r.ue()? as usize + 4;
    let poc = r.ue()?;
    if poc != 2 {
        return Err(AlignError(format!("expected POC type 2, got {poc}")));
    }
    r.ue()?;
    r.u(1)?;
    let width_mbs = r.ue()?;
    let height_mbs = r.ue()?;
    if r.u(1)? != 1 {
        return Err(AlignError("only frame pictures are supported".into()));
    }
    Ok(SpsParams {
        log2_frame_num,
        width_mbs,
        height_mbs,
    })
}

pub fn parse_pps(nal: &[u8]) -> Result<PpsParams, AlignError> {
    if nal.is_empty() {
        return Err(AlignError("empty PPS".into()));
    }
    let rbsp = unescape_rbsp(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    r.ue()?;
    r.ue()?;
    let cabac = r.u(1)?;
    if r.u(1)? != 0 || r.ue()? != 0 {
        return Err(AlignError("unsupported PPS pic_order/slice_groups".into()));
    }
    r.ue()?;
    r.ue()?;
    r.u(1)?;
    r.u(2)?;
    let init_qp = 26 + r.se()?;
    r.se()?;
    let chroma = r.se()?;
    let deblock = r.u(1)?;
    let constrained = r.u(1)?;
    let redundant = r.u(1)?;
    if redundant != 0 {
        return Err(AlignError("redundant_pic_cnt unsupported".into()));
    }
    let stop_bit = (0..rbsp.len() * 8)
        .rev()
        .find(|&k| (rbsp[k >> 3] >> (7 - (k & 7))) & 1 != 0)
        .ok_or_else(|| AlignError("missing RBSP trailing stop bit in PPS".into()))?;

    let mut t8 = 0u32;
    let mut second = chroma;
    if r.bit < stop_bit {
        t8 = r.u(1)?;
        if r.u(1)? != 0 {
            return Err(AlignError("PPS scaling lists unsupported".into()));
        }
        second = r.se()?;
    }
    Ok(PpsParams {
        cabac,
        init_qp,
        chroma,
        second,
        deblock,
        constrained,
        t8,
    })
}

fn rewrite_idr(
    nal: &[u8],
    src_sps: SpsParams,
    src_pps: PpsParams,
    dst_sps: SpsParams,
    dst_pps: PpsParams,
    idr_id: u32,
) -> Result<Vec<u8>, AlignError> {
    let raw = unescape_rbsp(&nal[1..]);
    let mut r = BitReader::new(&raw);
    let mut w = BitWriter::new();

    let first_mb = r.ue()?;
    let slice_type = r.ue()?;
    if slice_type % 5 != 2 {
        return Err(AlignError("expected I slice".into()));
    }
    let pps_id = r.ue()?;
    let _frame_num = r.u(src_sps.log2_frame_num)?;
    let _src_idr = r.ue()?;
    let no_output = r.u(1)?;
    let long_term = r.u(1)?;
    let qp = src_pps.init_qp + r.se()?;

    let deblock_mode = if src_pps.deblock != 0 { r.ue()? } else { 0 };
    let (db_alpha, db_beta) = if deblock_mode == 1 {
        (0, 0)
    } else if src_pps.deblock != 0 {
        (r.se()?, r.se()?)
    } else {
        (0, 0)
    };

    while (r.bit & 7) != 0 {
        if r.u(1)? != 1 {
            return Err(AlignError("invalid cabac_alignment_one_bit".into()));
        }
    }

    w.ue(first_mb);
    w.ue(slice_type);
    w.ue(pps_id);
    w.u(dst_sps.log2_frame_num, 0);
    w.ue(idr_id);
    w.u(1, no_output);
    w.u(1, long_term);
    w.se(qp - dst_pps.init_qp);

    if dst_pps.deblock != 0 {
        w.ue(deblock_mode);
        if deblock_mode != 1 {
            w.se(db_alpha);
            w.se(db_beta);
        }
    } else if deblock_mode != 0 || db_alpha != 0 || db_beta != 0 {
        return Err(AlignError("target PPS cannot carry deblocking offsets".into()));
    }

    while (w.bits.len() & 7) != 0 {
        w.u(1, 1);
    }

    let mut payload = w.finish_bytes()?;
    payload.extend_from_slice(&raw[r.bit >> 3..]);

    let mut out = Vec::with_capacity(1 + payload.len() + 16);
    out.push(nal[0]);
    out.extend_from_slice(&escape_rbsp(&payload));
    Ok(out)
}

/// Extract target Annex-B `SPS + PPS` and chroma QP offset from an existing
/// CarPlay-compatible waiting H.264 file (e.g. `attente-800x480.h264`).
pub fn extract_target_config(reference_h264: &[u8]) -> Result<(Vec<u8>, i32), AlignError> {
    let nals = iter_nals(reference_h264);
    let sps = nals
        .iter()
        .copied()
        .find(|n| !n.is_empty() && (n[0] & 31) == 7)
        .ok_or_else(|| AlignError("reference H.264 missing SPS".into()))?;
    let pps = nals
        .iter()
        .copied()
        .find(|n| !n.is_empty() && (n[0] & 31) == 8)
        .ok_or_else(|| AlignError("reference H.264 missing PPS".into()))?;
    let pps_info = parse_pps(pps)?;

    let mut config = Vec::with_capacity(8 + sps.len() + pps.len());
    config.extend_from_slice(&[0, 0, 0, 1]);
    config.extend_from_slice(sps);
    config.extend_from_slice(&[0, 0, 0, 1]);
    config.extend_from_slice(pps);
    Ok((config, pps_info.chroma))
}

/// Align raw x264 Annex-B IDR stream (`>= 2` IDR frames) to target CarPlay SPS/PPS.
pub fn align_h264(raw_x264: &[u8], target_config: &[u8]) -> Result<Vec<u8>, AlignError> {
    let dst_nals = iter_nals(target_config);
    let dst_sps_nal = dst_nals
        .iter()
        .copied()
        .find(|n| !n.is_empty() && (n[0] & 31) == 7)
        .ok_or_else(|| AlignError("target config missing SPS".into()))?;
    let dst_pps_nal = dst_nals
        .iter()
        .copied()
        .find(|n| !n.is_empty() && (n[0] & 31) == 8)
        .ok_or_else(|| AlignError("target config missing PPS".into()))?;
    let dst_sps = parse_sps(dst_sps_nal)?;
    let dst_pps = parse_pps(dst_pps_nal)?;

    let mut src_sps = None;
    let mut src_pps = None;
    let mut out = Vec::with_capacity(raw_x264.len() + target_config.len() * 2);
    let mut count = 0u32;

    for nal in iter_nals(raw_x264) {
        if nal.is_empty() {
            continue;
        }
        match nal[0] & 31 {
            7 => src_sps = Some(parse_sps(nal)?),
            8 => src_pps = Some(parse_pps(nal)?),
            5 => {
                let ss = src_sps.ok_or_else(|| AlignError("IDR before SPS".into()))?;
                let sp = src_pps.ok_or_else(|| AlignError("IDR before PPS".into()))?;
                if (ss.width_mbs, ss.height_mbs) != (dst_sps.width_mbs, dst_sps.height_mbs) {
                    return Err(AlignError("picture size differs from CarPlay config".into()));
                }
                if sp.cabac != dst_pps.cabac
                    || sp.chroma != dst_pps.chroma
                    || sp.second != dst_pps.second
                    || sp.constrained != dst_pps.constrained
                    || sp.t8 != dst_pps.t8
                {
                    return Err(AlignError("PPS coding flags differ from CarPlay config".into()));
                }
                let rewritten = rewrite_idr(nal, ss, sp, dst_sps, dst_pps, count % 2)?;
                out.extend_from_slice(target_config);
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&rewritten);
                count += 1;
            }
            _ => {}
        }
    }

    if count < 2 {
        return Err(AlignError(format!(
            "expected at least 2 IDR pictures, got {count}"
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_real_waiting_h264_and_realigns_idempotently() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../assets/waiting/attente-800x480.h264");
        if !path.is_file() {
            return;
        }
        let data = std::fs::read(&path).expect("read attente-800x480.h264");
        let (cfg, chroma) = extract_target_config(&data).expect("extract target config");
        assert!(!cfg.is_empty());
        assert!((-12..=12).contains(&chroma));
        let aligned = align_h264(&data, &cfg).expect("realign existing waiting stream");
        assert!(!aligned.is_empty());
        let idr_count = iter_nals(&aligned)
            .into_iter()
            .filter(|n| !n.is_empty() && (n[0] & 31) == 5)
            .count();
        assert!(idr_count >= 2);
    }
}
