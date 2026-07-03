#!/usr/bin/env python3
"""Minimal po -> lmo compiler compatible with LuCI's lmo reader
(luci-base src/lib/lmo.c). Entries are (key_id, val_id, offset, length)
big-endian u32 quads sorted by key_id, followed by a u32 index offset.
key_id is Paul Hsieh's SuperFastHash of the msgid (with msgctxt as
"ctx\1msgid" when present).

usage: po2lmo.py input.po output.lmo
       po2lmo.py --dump file.lmo          (debug: list entries)
"""

import struct
import sys


def sfh(data: bytes) -> int:
    """Paul Hsieh's SuperFastHash, as used by LuCI lmo."""
    length = len(data)
    h = length & 0xFFFFFFFF
    if length == 0:
        return 0

    def u16(b, i):
        return b[i] | (b[i + 1] << 8)

    rem = length & 3
    n = length >> 2
    i = 0
    for _ in range(n):
        h = (h + u16(data, i)) & 0xFFFFFFFF
        tmp = ((u16(data, i + 2) << 11) ^ h) & 0xFFFFFFFF
        h = ((h << 16) ^ tmp) & 0xFFFFFFFF
        i += 4
        h = (h + (h >> 11)) & 0xFFFFFFFF

    if rem == 3:
        h = (h + u16(data, i)) & 0xFFFFFFFF
        h = (h ^ (h << 16)) & 0xFFFFFFFF
        b = data[i + 2]
        if b >= 128:  # signed char in the C code
            b -= 256
        h = (h ^ ((b << 18) & 0xFFFFFFFF)) & 0xFFFFFFFF
        h = (h + (h >> 11)) & 0xFFFFFFFF
    elif rem == 2:
        h = (h + u16(data, i)) & 0xFFFFFFFF
        h = (h ^ (h << 11)) & 0xFFFFFFFF
        h = (h + (h >> 17)) & 0xFFFFFFFF
    elif rem == 1:
        b = data[i]
        if b >= 128:
            b -= 256
        h = (h + b) & 0xFFFFFFFF
        h = (h ^ (h << 10)) & 0xFFFFFFFF
        h = (h + (h >> 1)) & 0xFFFFFFFF

    h = (h ^ (h << 3)) & 0xFFFFFFFF
    h = (h + (h >> 5)) & 0xFFFFFFFF
    h = (h ^ (h << 4)) & 0xFFFFFFFF
    h = (h + (h >> 17)) & 0xFFFFFFFF
    h = (h ^ (h << 25)) & 0xFFFFFFFF
    h = (h + (h >> 6)) & 0xFFFFFFFF
    return h


def parse_po(path):
    """Parse a .po file into (msgctxt, msgid, msgstr) tuples."""
    entries = []
    ctx = mid = mstr = None
    state = None

    def flush():
        nonlocal ctx, mid, mstr
        if mid is not None and mstr:
            entries.append((ctx, mid, mstr))
        ctx = mid = mstr = None

    def unquote(line):
        line = line.strip()
        if line.startswith('"') and line.endswith('"'):
            body = line[1:-1]
            return (
                body.replace('\\n', '\n')
                .replace('\\t', '\t')
                .replace('\\"', '"')
                .replace('\\\\', '\\')
            )
        return ''

    with open(path, encoding='utf-8') as f:
        for line in f:
            ls = line.strip()
            if not ls or ls.startswith('#'):
                continue
            if ls.startswith('msgctxt'):
                flush()
                ctx = unquote(ls[7:])
                state = 'ctx'
            elif ls.startswith('msgid_plural'):
                state = 'skip'  # plurals unsupported (unused by photondns)
            elif ls.startswith('msgid'):
                if state != 'ctx':
                    flush()
                mid = unquote(ls[5:])
                state = 'id'
            elif ls.startswith('msgstr'):
                rest = ls[6:].strip()
                if rest.startswith('['):
                    state = 'skip'
                    continue
                mstr = unquote(rest)
                state = 'str'
            elif ls.startswith('"'):
                if state == 'ctx':
                    ctx += unquote(ls)
                elif state == 'id':
                    mid += unquote(ls)
                elif state == 'str':
                    mstr += unquote(ls)
    flush()
    # drop the po header (empty msgid)
    return [(c, i, s) for c, i, s in entries if i]


def write_lmo(entries, path):
    data = bytearray()
    index = []
    for ctx, mid, mstr in entries:
        key = (f'{ctx}\1{mid}' if ctx else mid).encode('utf-8')
        val = mstr.encode('utf-8')
        key_id = sfh(key)
        val_id = sfh(val)
        offset = len(data)
        data.extend(val)
        # pad values to 4-byte alignment like po2lmo.c
        while len(data) % 4:
            data.append(0)
        index.append((key_id, val_id, offset, len(val)))
    index.sort(key=lambda e: e[0])
    with open(path, 'wb') as f:
        f.write(bytes(data))
        idx_offset = len(data)
        for e in index:
            f.write(struct.pack('>IIII', *e))
        f.write(struct.pack('>I', idx_offset))
    return len(index)


def dump_lmo(path):
    raw = open(path, 'rb').read()
    idx_offset = struct.unpack('>I', raw[-4:])[0]
    table = raw[idx_offset:-4]
    out = []
    for i in range(0, len(table), 16):
        key_id, val_id, off, ln = struct.unpack('>IIII', table[i:i + 16])
        out.append((key_id, val_id, raw[off:off + ln].decode('utf-8', 'replace')))
    return out


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--dump':
        for key_id, _, s in dump_lmo(sys.argv[2]):
            print('%08x %s' % (key_id, s[:60].replace('\n', ' ')))
    elif len(sys.argv) == 3:
        n = write_lmo(parse_po(sys.argv[1]), sys.argv[2])
        print('%s: %d entries' % (sys.argv[2], n))
    else:
        print(__doc__)
        sys.exit(2)
