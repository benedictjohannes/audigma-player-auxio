// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! M7 packer for the FiiO JM21 (`DSD_U16_LE` stereo, `docs/dsd-transport-spec.md`).
//!
//! Stopgap: audigma's `DSD_U32_BE` packer followed by an in-place byte permutation of every
//! 8-byte frame. From chronological MSB-first DSD bytes `L0 L1 L2 L3` / `R0 R1 R2 R3`:
//! `U32_BE` is `L0 L1 L2 L3 R0 R1 R2 R3`, M7 is `L1 L0 R1 R0 L3 L2 R3 R2`.

use audigma_format::{FormatError, STEREO_DSD_U32_FRAME_BYTES, pack_stereo_dsd_u32_be};

/// Bytes per channel in one DSF block.
pub const DSF_BLOCK: usize = audigma_format::DSF_BLOCK_SIZE_PER_CHANNEL as usize;
/// Bytes of one stereo DSF block pair (L block then R block).
pub const DSF_BLOCK_PAIR: usize = 2 * DSF_BLOCK;

/// Packs chronological stereo DSD bytes into M7 frames (8 output bytes per 4 input bytes per
/// channel). `bitrev` reverses the bits of every byte: `true` for DSF (LSB-first), `false` for
/// MSB-first input. Returns the number of bytes written.
pub fn pack_m7(
    left: &[u8],
    right: &[u8],
    bitrev: bool,
    out: &mut [u8],
) -> Result<usize, FormatError> {
    let n = pack_stereo_dsd_u32_be(left, right, bitrev, out)?;
    for f in out[..n].chunks_exact_mut(STEREO_DSD_U32_FRAME_BYTES) {
        let [l0, l1, l2, l3, r0, r1, r2, r3] = *<&[u8; 8]>::try_from(&*f).unwrap();
        f.copy_from_slice(&[l1, l0, r1, r0, l3, l2, r3, r2]);
    }
    Ok(n)
}

/// Packs raw DSF data (`blocks` pairs of `DSF_BLOCK` bytes L then `DSF_BLOCK` bytes R, LSB-first)
/// into M7. `src.len()` must be a multiple of `DSF_BLOCK_PAIR`; `dst` must hold `src.len()` bytes.
pub fn pack_m7_dsf_blocks(src: &[u8], dst: &mut [u8]) -> Result<usize, FormatError> {
    if src.len() % DSF_BLOCK_PAIR != 0 {
        return Err(FormatError::UnalignedDsdByteCount {
            count: src.len(),
            required_multiple: DSF_BLOCK_PAIR,
        });
    }
    if dst.len() < src.len() {
        return Err(FormatError::OutputBufferTooSmall {
            required: src.len(),
            provided: dst.len(),
        });
    }
    for (s, d) in src
        .chunks_exact(DSF_BLOCK_PAIR)
        .zip(dst.chunks_exact_mut(DSF_BLOCK_PAIR))
    {
        pack_m7(&s[..DSF_BLOCK], &s[DSF_BLOCK..], true, d)?;
    }
    Ok(src.len())
}

/// Bytes of one M7 frame on the track (2 channels x 32 DSD bits).
pub const M7_FRAME: usize = 8;

/// Streaming planar packer: chronological per-channel DSD bytes to M7. M7 frames take 4 bytes
/// per channel, so up to 3 bytes per channel are carried to the next call; consecutive calls of
/// one stream (and gapless successors) pack sample-exactly.
pub struct M7Stream {
    /// Reverse the bits of every byte: `true` for LSB-first input (DSF), `false` for MSB-first.
    bitrev: bool,
    l: [u8; 4],
    r: [u8; 4],
    carried: usize,
}

impl M7Stream {
    pub fn new(bitrev: bool) -> M7Stream {
        M7Stream {
            bitrev,
            l: [0; 4],
            r: [0; 4],
            carried: 0,
        }
    }

    /// M7 bytes [`Self::pack`] writes for `per_channel` input bytes per channel.
    pub fn packed_len(&self, per_channel: usize) -> usize {
        (self.carried + per_channel) / 4 * M7_FRAME
    }

    /// Drops the carried bytes (seek, flush).
    pub fn clear(&mut self) {
        self.carried = 0;
    }

    /// Packs `l` and `r` (equal lengths) into `dst`; returns bytes written.
    pub fn pack(
        &mut self,
        mut l: &[u8],
        mut r: &[u8],
        dst: &mut [u8],
    ) -> Result<usize, FormatError> {
        let mut o = 0;
        if self.carried > 0 {
            let k = (4 - self.carried).min(l.len());
            self.l[self.carried..self.carried + k].copy_from_slice(&l[..k]);
            self.r[self.carried..self.carried + k].copy_from_slice(&r[..k]);
            self.carried += k;
            (l, r) = (&l[k..], &r[k..]);
            if self.carried < 4 {
                return Ok(0);
            }
            o += pack_m7(&self.l, &self.r, self.bitrev, &mut dst[..M7_FRAME])?;
            self.carried = 0;
        }
        let m = l.len() / 4 * 4;
        o += pack_m7(&l[..m], &r[..m], self.bitrev, &mut dst[o..])?;
        let rest = l.len() - m;
        self.l[..rest].copy_from_slice(&l[m..]);
        self.r[..rest].copy_from_slice(&r[m..]);
        self.carried = rest;
        Ok(o)
    }
}

/// Streaming DSF-to-M7 packer for consecutive pushes of one stream (and gapless successors).
///
/// Input per push: whole DSF block pairs, optionally ending in one short pair of `v` bytes L then
/// `v` bytes R (a file's last block truncated to its sample count). Partial frames are carried by
/// [`M7Stream`], so a track ending mid-frame joins the next one sample-exactly.
pub struct DsfPacker(M7Stream);

impl Default for DsfPacker {
    fn default() -> Self {
        DsfPacker(M7Stream::new(true))
    }
}

impl DsfPacker {
    /// M7 bytes [`Self::pack`] writes for `src_len` input bytes; `None` if `src_len` is not whole
    /// pairs plus an even short pair.
    pub fn packed_len(&self, src_len: usize) -> Option<usize> {
        let tail = src_len % DSF_BLOCK_PAIR;
        if !tail.is_multiple_of(2) {
            return None;
        }
        Some(self.0.packed_len(src_len / DSF_BLOCK_PAIR * DSF_BLOCK + tail / 2))
    }

    /// Packs `src` into `dst` (which must hold [`Self::packed_len`] bytes); returns bytes written.
    pub fn pack(&mut self, src: &[u8], dst: &mut [u8]) -> Result<usize, FormatError> {
        let Some(need) = self.packed_len(src.len()) else {
            return Err(FormatError::UnalignedDsdByteCount {
                count: src.len(),
                required_multiple: 2,
            });
        };
        if dst.len() < need {
            return Err(FormatError::OutputBufferTooSmall {
                required: need,
                provided: dst.len(),
            });
        }
        let split = src.len() / DSF_BLOCK_PAIR * DSF_BLOCK_PAIR;
        let (pairs, tail) = src.split_at(split);
        let mut o = 0;
        for pair in pairs.chunks_exact(DSF_BLOCK_PAIR) {
            let (l, r) = pair.split_at(DSF_BLOCK);
            o += self.0.pack(l, r, &mut dst[o..])?;
        }
        if !tail.is_empty() {
            let (l, r) = tail.split_at(tail.len() / 2);
            o += self.0.pack(l, r, &mut dst[o..])?;
        }
        Ok(o)
    }

    /// Drops the carried bytes (seek, flush).
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent restatement of `tools/m7b.py`: per 2 source bytes of each channel,
    /// `rev(L1) rev(L0) rev(R1) rev(R0)`.
    fn reference(src: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for blk in src.chunks_exact(DSF_BLOCK_PAIR) {
            let (l, r) = blk.split_at(DSF_BLOCK);
            for i in (0..DSF_BLOCK).step_by(2) {
                out.extend([
                    l[i + 1].reverse_bits(),
                    l[i].reverse_bits(),
                    r[i + 1].reverse_bits(),
                    r[i].reverse_bits(),
                ]);
            }
        }
        out
    }

    #[test]
    fn dsf_blocks_match_m7b_formula() {
        let src: Vec<u8> = (0..3 * DSF_BLOCK_PAIR as u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let mut dst = vec![0u8; src.len()];
        assert_eq!(pack_m7_dsf_blocks(&src, &mut dst), Ok(src.len()));
        assert_eq!(dst, reference(&src));
    }

    #[test]
    fn msb_first_golden_frame() {
        let mut out = [0u8; 8];
        pack_m7(
            &[0x10, 0x11, 0x12, 0x13],
            &[0x20, 0x21, 0x22, 0x23],
            false,
            &mut out,
        )
        .unwrap();
        assert_eq!(out, [0x11, 0x10, 0x21, 0x20, 0x13, 0x12, 0x23, 0x22]);
    }

    #[test]
    fn rejects_unaligned_and_short() {
        let mut dst = vec![0u8; DSF_BLOCK_PAIR];
        assert!(pack_m7_dsf_blocks(&dst[..100], &mut [0u8; 100]).is_err());
        assert!(pack_m7_dsf_blocks(&dst.clone(), &mut dst[..8]).is_err());
    }

    /// A file ending in a short pair (sample count not on a block or frame boundary) followed by
    /// the next track: the output equals packing the two tracks' channels as one stream.
    #[test]
    fn short_last_pair_joins_next_track_sample_exactly() {
        let bytes = |n: usize, seed: u32| -> Vec<u8> {
            (0..n as u32)
                .map(|i| (i.wrapping_add(seed).wrapping_mul(2654435761) >> 13) as u8)
                .collect()
        };
        // Track A: 2 full pairs + a short pair of 1001 bytes per channel; track B: 3 full pairs.
        let (a_l, a_r) = (
            bytes(2 * DSF_BLOCK + 1001, 1),
            bytes(2 * DSF_BLOCK + 1001, 2),
        );
        let (b_l, b_r) = (bytes(3 * DSF_BLOCK, 3), bytes(3 * DSF_BLOCK, 4));
        let planar = |l: &[u8], r: &[u8]| -> Vec<u8> {
            let mut v = Vec::new();
            for (cl, cr) in l.chunks(DSF_BLOCK).zip(r.chunks(DSF_BLOCK)) {
                v.extend_from_slice(cl);
                v.extend_from_slice(cr);
            }
            v
        };
        let (a, b) = (planar(&a_l, &a_r), planar(&b_l, &b_r));

        let mut p = DsfPacker::default();
        let mut got = Vec::new();
        // Push A as one full pair, then one full pair plus the short pair; then B in one push.
        for chunk in [&a[..DSF_BLOCK_PAIR], &a[DSF_BLOCK_PAIR..], &b[..]] {
            let mut d = vec![0u8; p.packed_len(chunk.len()).unwrap()];
            assert_eq!(p.pack(chunk, &mut d), Ok(d.len()));
            got.extend(d);
        }

        let (l, r) = ([a_l, b_l].concat(), [a_r, b_r].concat());
        let m = l.len() / 4 * 4;
        let mut want = vec![0u8; 2 * m];
        pack_m7(&l[..m], &r[..m], true, &mut want).unwrap();
        assert_eq!(got, want);
        assert!(p.packed_len(DSF_BLOCK_PAIR + 1).is_none());
    }
}
