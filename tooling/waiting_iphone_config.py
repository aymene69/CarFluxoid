"""Rebuild a waiting screen under the iPhone's own H.264 SPS/PPS (offline, no re-encoding).

The Uconnect AA player keeps the first codec configuration of a session, and falls back to its
native screen when no picture arrives for ~5 s. The waiting screen must therefore start at once
with exactly the configuration CarPlay will use later. Usage:
  python3 -I waiting_iphone_config.py X264_IDR.h264 CONFIG_HEX OUT.h264
X264_IDR.h264: x264 High CABAC IDR pictures (8x8 transform, chroma offsets 0, no scaling lists).
CONFIG_HEX: Annex-B SPS+PPS sent by the bridge for CarPlay (POC type 2, after h264_poc rewrite).
Only slice headers change: frame_num width, idr_pic_id and slice_qp_delta follow the target
SPS/PPS; the byte-aligned CABAC payload is copied unchanged.
"""
import re, sys

def nals(data):
    starts = [m.end() for m in re.finditer(b'\x00\x00\x01', data)]
    for i, s in enumerate(starts):
        end = starts[i + 1] - 3 if i + 1 < len(starts) else len(data)
        while end > s and data[end - 1] == 0:
            end -= 1
        yield data[s:end]

def unescape(b):
    return re.sub(b'\x00\x00\x03', b'\x00\x00', b)

def escape(b):
    out, zeros = bytearray(), 0
    for x in b:
        if zeros >= 2 and x <= 3:
            out.append(3); zeros = 0
        out.append(x); zeros = zeros + 1 if x == 0 else 0
    return bytes(out)

class Reader:
    def __init__(self, data): self.d, self.i = data, 0
    def u(self, n):
        v = 0
        for _ in range(n):
            v = v << 1 | (self.d[self.i >> 3] >> (7 - (self.i & 7)) & 1); self.i += 1
        return v
    def ue(self):
        z = 0
        while self.u(1) == 0: z += 1
        return (1 << z) - 1 + self.u(z)
    def se(self):
        k = self.ue()
        return (k + 1) // 2 if k & 1 else -(k // 2)

class Writer:
    def __init__(self): self.bits = []
    def u(self, n, v): self.bits += [(v >> (n - 1 - k)) & 1 for k in range(n)]
    def ue(self, v):
        n = (v + 1).bit_length(); self.u(n - 1, 0); self.u(n, v + 1)
    def se(self, v): self.ue(2 * v - 1 if v > 0 else -2 * v)
    def bytes(self):
        assert len(self.bits) % 8 == 0
        return bytes(int(''.join(map(str, self.bits[k:k + 8])), 2) for k in range(0, len(self.bits), 8))

HIGH = {100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135}

def sps_params(nal):
    r = Reader(unescape(nal[1:])); profile = r.u(8); r.u(16); r.ue()
    if profile in HIGH:
        assert r.ue() == 1 and r.ue() == 0 and r.ue() == 0 and r.u(1) == 0 and r.u(1) == 0, 'unsupported chroma/bit depth/scaling'
    log2_frame_num = r.ue() + 4; poc = r.ue(); assert poc == 2, 'POC type 2 expected'
    r.ue(); r.u(1); w = r.ue(); h = r.ue(); assert r.u(1) == 1, 'frame pictures only'
    return {'log2_frame_num': log2_frame_num, 'size': (w, h)}

def pps_params(nal):
    r = Reader(unescape(nal[1:])); r.ue(); r.ue(); cabac = r.u(1); assert r.u(1) == 0 and r.ue() == 0
    r.ue(); r.ue(); r.u(1); r.u(2); init_qp = 26 + r.se(); r.se(); chroma = r.se(); deblock = r.u(1)
    constrained = r.u(1); redundant = r.u(1); assert not redundant
    rest = unescape(nal[1:]); stop = max(k for k in range(len(rest) * 8) if rest[k >> 3] >> (7 - (k & 7)) & 1)
    t8 = second = None
    if r.i < stop:
        t8 = r.u(1); assert r.u(1) == 0, 'scaling lists unsupported'; second = r.se()
    return {'cabac': cabac, 'init_qp': init_qp, 'chroma': chroma, 'second': chroma if second is None else second,
            'deblock': deblock, 'constrained': constrained, 't8': t8 or 0}

def rewrite_idr(nal, src_sps, src_pps, dst_sps, dst_pps, idr_id):
    raw = unescape(nal[1:]); r = Reader(raw); w = Writer()
    first_mb = r.ue(); slice_type = r.ue(); assert slice_type % 5 == 2, 'I slices only'
    pps_id = r.ue(); r.u(src_sps['log2_frame_num']); r.ue()
    no_output, long_term = r.u(1), r.u(1)
    qp = src_pps['init_qp'] + r.se()
    deblock_mode = r.ue() if src_pps['deblock'] else 0
    if deblock_mode == 1:
        deblock = (1,)
    elif src_pps['deblock']:
        deblock = (deblock_mode, r.se(), r.se())
    else:
        deblock = (0, 0, 0)
    while r.i & 7: assert r.u(1) == 1, 'cabac_alignment_one_bit'
    w.ue(first_mb); w.ue(slice_type); w.ue(pps_id); w.u(dst_sps['log2_frame_num'], 0); w.ue(idr_id)
    w.u(1, no_output); w.u(1, long_term); w.se(qp - dst_pps['init_qp'])
    if dst_pps['deblock']:
        w.ue(deblock[0])
        if deblock[0] != 1: w.se(deblock[1]); w.se(deblock[2])
    else:
        assert deblock == (0, 0, 0), 'target PPS cannot carry deblocking offsets'
    while len(w.bits) & 7: w.u(1, 1)
    return nal[:1] + escape(w.bytes() + raw[r.i >> 3:])

def main(src_path, config_hex, out_path):
    config = bytes.fromhex(config_hex); dst = list(nals(config))
    dst_sps = sps_params(next(n for n in dst if n[0] & 31 == 7)); dst_pps = pps_params(next(n for n in dst if n[0] & 31 == 8))
    out = bytearray(); src_sps = src_pps = None; count = 0
    for nal in nals(open(src_path, 'rb').read()):
        kind = nal[0] & 31
        if kind == 7: src_sps = sps_params(nal)
        elif kind == 8: src_pps = pps_params(nal)
        elif kind == 5:
            assert src_sps['size'] == dst_sps['size'], 'picture size differs from CarPlay'
            for key in ('cabac', 'chroma', 'second', 'constrained', 't8'):
                assert src_pps[key] == dst_pps[key], 'PPS field differs: ' + key
            out += config + b'\x00\x00\x00\x01' + rewrite_idr(nal, src_sps, src_pps, dst_sps, dst_pps, count % 2); count += 1
    assert count >= 2, 'two IDR pictures expected'
    open(out_path, 'wb').write(out); print(f'{count} IDR pictures rebuilt under the CarPlay configuration: {out_path}')

if __name__ == '__main__':
    main(*sys.argv[1:4])
