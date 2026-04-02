#!/usr/bin/env python3
"""
Standalone LIT container parser — minimal reimplementation of calibre's
reader.py ITSS layer for comparison with the Rust eruditio port.

No calibre dependencies required. Tests:
  1. ITOLITLS header parsing
  2. CAOL/ITSF secondary header
  3. IFCM/AOLL directory listing
  4. Section names (NameList)
  5. Manifest parsing
  6. DRM detection

Usage: python3 test_lit_calibre.py assets/lit/*.lit
"""

import struct
import sys
import os
import time


def u32le(data, off=0):
    return struct.unpack_from('<I', data, off)[0]

def i32le(data, off=0):
    return struct.unpack_from('<i', data, off)[0]

def u16le(data, off=0):
    return struct.unpack_from('<H', data, off)[0]


def encint(data, pos):
    """Variable-length integer (encint) decoder."""
    val = 0
    while pos < len(data):
        b = data[pos]
        pos += 1
        val = (val << 7) | (b & 0x7F)
        if b < 0x80:
            break
    return val, pos


def parse_listing_entries(data, remaining):
    """Parse AOLL listing chunk entries."""
    entries = {}
    pos = 0
    end = len(data) - remaining
    while pos < end:
        # Name: encint length, then UTF-8 bytes
        name_len, pos = encint(data, pos)
        if pos + name_len > end:
            break
        name = data[pos:pos + name_len].decode('utf-8', errors='replace')
        pos += name_len
        # Section
        section, pos = encint(data, pos)
        # Offset
        offset, pos = encint(data, pos)
        # Size
        size, pos = encint(data, pos)
        entries[name] = {
            'name': name,
            'section': section,
            'offset': offset,
            'size': size,
        }
    return entries


def parse_lit_container(data):
    """Parse a LIT (ITOLITLS) container and return directory info."""
    result = {
        'valid': False,
        'error': None,
        'version': None,
        'hdr_len': None,
        'num_pieces': None,
        'sec_hdr_len': None,
        'content_offset': None,
        'entries': {},
        'section_names': [],
        'manifest_items': [],
        'drm_level': 0,
    }

    if len(data) < 48:
        result['error'] = 'File too short'
        return result

    # --- ITOLITLS header ---
    magic = data[0:8]
    if magic != b'ITOLITLS':
        result['error'] = f'Bad magic: {magic!r}'
        return result

    version = u32le(data, 8)
    hdr_len = i32le(data, 12)
    num_pieces = i32le(data, 16)
    sec_hdr_len = i32le(data, 20)

    result['version'] = version
    result['hdr_len'] = hdr_len
    result['num_pieces'] = num_pieces
    result['sec_hdr_len'] = sec_hdr_len

    if version != 1:
        result['error'] = f'Unsupported version: {version}'
        return result

    # --- Secondary header (CAOL + ITSF) ---
    sec_hdr_offset = hdr_len + num_pieces * 16
    if sec_hdr_offset + sec_hdr_len > len(data):
        result['error'] = 'Secondary header out of range'
        return result
    sec_hdr = data[sec_hdr_offset:sec_hdr_offset + sec_hdr_len]

    content_offset = 0
    found_itsf = False
    entry_chunklen = 0

    if len(sec_hdr) >= 8:
        off = i32le(sec_hdr, 4)
        while off + 8 <= len(sec_hdr):
            block_type = sec_hdr[off:off + 4]
            if block_type == b'CAOL' and off + 48 <= len(sec_hdr):
                entry_chunklen = u32le(sec_hdr, off + 20)
                off += 48
            elif block_type == b'ITSF' and off + 20 <= len(sec_hdr):
                content_offset = u32le(sec_hdr, off + 16)
                found_itsf = True
                off += min(48, len(sec_hdr) - off)
            else:
                break

    if not found_itsf:
        result['error'] = 'Missing ITSF block'
        return result

    result['content_offset'] = content_offset

    # --- Header pieces ---
    pieces_start = hdr_len
    entries = {}

    for i in range(num_pieces):
        p = pieces_start + i * 16
        if p + 16 > len(data):
            break
        piece_offset = u32le(data, p)
        piece_size = i32le(data, p + 8)
        if piece_offset + piece_size > len(data):
            continue
        piece = data[piece_offset:piece_offset + piece_size]

        if i == 1:
            # Directory piece — parse IFCM/AOLL
            if len(piece) < 4 or piece[0:4] != b'IFCM':
                result['error'] = 'Piece 1 is not IFCM'
                return result
            if len(piece) >= 28:
                chunk_size = i32le(piece, 8)
                num_chunks = i32le(piece, 24)
                if chunk_size > 0 and len(piece) >= 32:
                    for ci in range(num_chunks):
                        offset = 32 + ci * chunk_size
                        if offset + chunk_size > len(piece):
                            break
                        chunk = piece[offset:offset + chunk_size]
                        if len(chunk) < 48 or chunk[0:4] != b'AOLL':
                            continue
                        remaining_raw = i32le(chunk, 4)
                        remaining = chunk_size - remaining_raw - 48
                        if remaining < 0:
                            remaining = 0
                        entry_data = chunk[48:]
                        try:
                            chunk_entries = parse_listing_entries(entry_data, remaining)
                            entries.update(chunk_entries)
                        except Exception:
                            pass

    result['entries'] = entries

    # --- Section names ---
    if '::DataSpace/NameList' in entries:
        e = entries['::DataSpace/NameList']
        start = content_offset + e['offset']
        end = start + e['size']
        if end <= len(data):
            raw = data[start:end]
            if len(raw) >= 4:
                num_sections = u16le(raw, 2)
                pos = 4
                names = []
                for _ in range(num_sections):
                    if pos + 2 > len(raw):
                        break
                    size = u16le(raw, pos)
                    pos += 2
                    byte_len = size * 2 + 2
                    if pos + byte_len > len(raw):
                        break
                    name = raw[pos:pos + byte_len].decode('utf-16-le', errors='replace').rstrip('\0')
                    names.append(name)
                    pos += byte_len
                result['section_names'] = names

    # --- Manifest ---
    if '/manifest' in entries:
        e = entries['/manifest']
        start = content_offset + e['offset']
        end = start + e['size']
        if end <= len(data):
            raw = data[start:end]
            items = []
            pos = 0
            while pos < len(raw):
                if pos >= len(raw):
                    break
                root_len = raw[pos]
                pos += 1
                if root_len == 0:
                    break
                if pos + root_len > len(raw):
                    break
                root = raw[pos:pos + root_len].decode('utf-8', errors='replace')
                pos += root_len
                for state_name in ['spine', 'not spine', 'css', 'images']:
                    if pos + 4 > len(raw):
                        break
                    num_files = i32le(raw, pos)
                    pos += 4
                    for _ in range(num_files):
                        if pos + 4 > len(raw):
                            break
                        file_offset = u32le(raw, pos)
                        pos += 4
                        internal, pos = consume_sized_utf8_string(raw, pos, False)
                        original, pos = consume_sized_utf8_string(raw, pos, False)
                        mime_type, pos = consume_sized_utf8_string(raw, pos, True)
                        items.append({
                            'internal': internal,
                            'original': original,
                            'mime_type': mime_type.lower(),
                            'state': state_name,
                        })
            result['manifest_items'] = items

    # --- DRM check ---
    if '/DRMStorage/Licenses/EUL' in entries:
        result['drm_level'] = 5
    elif '/DRMStorage/DRMBookplate' in entries:
        result['drm_level'] = 3
    elif '/DRMStorage/DRMSealed' in entries:
        result['drm_level'] = 1

    result['valid'] = True
    return result


def consume_sized_utf8_string(data, pos, zpad):
    """Read a sized UTF-8 string: first char's ordinal = length, then that many chars."""
    if pos >= len(data):
        return '', pos
    # Read one UTF-8 char to get the length
    b = data[pos]
    if b < 0x80:
        char_len = b
        pos += 1
    elif b < 0xE0:
        char_len = ((b & 0x1F) << 6) | (data[pos + 1] & 0x3F)
        pos += 2
    elif b < 0xF0:
        char_len = ((b & 0x0F) << 12) | ((data[pos + 1] & 0x3F) << 6) | (data[pos + 2] & 0x3F)
        pos += 3
    else:
        char_len = ((b & 0x07) << 18) | ((data[pos + 1] & 0x3F) << 12) | ((data[pos + 2] & 0x3F) << 6) | (data[pos + 3] & 0x3F)
        pos += 4

    # Read char_len UTF-8 characters
    result = []
    for _ in range(char_len):
        if pos >= len(data):
            break
        b = data[pos]
        if b < 0x80:
            result.append(chr(b))
            pos += 1
        elif b < 0xE0:
            if pos + 1 < len(data):
                result.append(chr(((b & 0x1F) << 6) | (data[pos + 1] & 0x3F)))
            pos += 2
        elif b < 0xF0:
            if pos + 2 < len(data):
                result.append(chr(((b & 0x0F) << 12) | ((data[pos + 1] & 0x3F) << 6) | (data[pos + 2] & 0x3F)))
            pos += 3
        else:
            if pos + 3 < len(data):
                cp = ((b & 0x07) << 18) | ((data[pos + 1] & 0x3F) << 12) | ((data[pos + 2] & 0x3F) << 6) | (data[pos + 3] & 0x3F)
                result.append(chr(cp))
            pos += 4

    if zpad and pos < len(data) and data[pos] == 0:
        pos += 1

    return ''.join(result), pos


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <lit_file> [lit_file ...]")
        sys.exit(1)

    print("=" * 80)
    print("LIT File Test Report (Python standalone parser)")
    print("=" * 80)

    success = 0
    fail = 0

    for path in sorted(sys.argv[1:]):
        name = os.path.basename(path)
        try:
            data = open(path, 'rb').read()
        except Exception as e:
            print(f"\n--- {name} ---")
            print(f"  ERROR: {e}")
            fail += 1
            continue

        size = len(data)
        magic = data[0:8] if len(data) >= 8 else b''

        print(f"\n--- {name} ({size} bytes) ---")

        if magic != b'ITOLITLS':
            print(f"  SKIP: Not ITOLITLS (magic: {magic!r})")
            continue

        start = time.time()
        result = parse_lit_container(data)
        elapsed = (time.time() - start) * 1000

        if not result['valid']:
            fail += 1
            print(f"  FAIL ({elapsed:.1f}ms): {result['error']}")
            continue

        success += 1
        drm = f" [DRM level {result['drm_level']}]" if result['drm_level'] > 0 else ""
        print(f"  OK ({elapsed:.1f}ms){drm}")
        print(f"  Header:    ver={result['version']} hdr_len={result['hdr_len']} pieces={result['num_pieces']} sec_hdr={result['sec_hdr_len']}")
        print(f"  Content@:  {result['content_offset']}")
        print(f"  Entries:   {len(result['entries'])}")
        print(f"  Sections:  {result['section_names']}")
        print(f"  Manifest:  {len(result['manifest_items'])} items")

        # Show manifest
        for item in result['manifest_items']:
            print(f"    [{item['state']}] {item['internal']} -> {item['original']} ({item['mime_type']})")

        # Show key directory entries
        for key in sorted(result['entries'].keys()):
            e = result['entries'][key]
            if key.startswith('/') or key.startswith('::DataSpace'):
                print(f"    {key}: sec={e['section']} off={e['offset']} size={e['size']}")

    print(f"\n{'=' * 80}")
    print(f"Results: {success} success, {fail} failed")
    print("=" * 80)


if __name__ == '__main__':
    main()
