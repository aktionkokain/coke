"""Rank x86 code matches for CokeRange anchors from a working title."""

import argparse
from collections import Counter
from configparser import ConfigParser, Error as ConfigError
from pathlib import Path
import re
import struct



CODE_ANCHORS = (
    "draw_call", "draw_flush", "trace", "ribbon", "hud_call", "hud_draw",
    "label_draw", "label_width", "label_background",
)


MAX_PE_SIZE = 512 * 1024 * 1024


def read_code(path):
    with path.open("rb") as image:
        data = image.read(MAX_PE_SIZE + 1)
    if len(data) > MAX_PE_SIZE:
        raise ValueError(f"{path}: image exceeds {MAX_PE_SIZE} bytes")
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"{path}: expected a DOS header")
    pe, = struct.unpack_from("<I", data, 0x3C)
    if not 64 <= pe <= len(data) - 24 or data[pe:pe + 4] != b"PE\0\0":
        raise ValueError(f"{path}: expected a complete PE header")
    machine, count = struct.unpack_from("<HH", data, pe + 4)
    optional_size, = struct.unpack_from("<H", data, pe + 20)
    optional = pe + 24
    table = optional + optional_size
    if not 1 <= count <= 96 or optional_size < 96 or table + count * 40 > len(data):
        raise ValueError(f"{path}: invalid section table")
    magic, = struct.unpack_from("<H", data, optional)
    image_size, = struct.unpack_from("<I", data, optional + 56)
    if machine != 0x14C or magic != 0x10B:
        raise ValueError(f"{path}: expected a 32-bit x86 PE")
    sections = []
    for start in range(table, table + count * 40, 40):
        name, size, rva, raw_size, raw_offset = struct.unpack_from("<8sIIII", data, start)
        flags, = struct.unpack_from("<I", data, start + 36)
        if flags & 0x20000000 and raw_size:
            size = min(size or raw_size, raw_size)
            if raw_offset < table + count * 40 or raw_offset + raw_size > len(data):
                raise ValueError(f"{path}: invalid executable section file span")
            if rva == 0 or rva + size > image_size:
                raise ValueError(f"{path}: invalid executable section RVA span")
            sections.append((name.split(b"\0", 1)[0], rva, data[raw_offset:raw_offset + size]))
    if not sections:
        raise ValueError(f"{path}: no executable sections with raw data")
    sections.sort(key=lambda section: section[1])
    if any(a[1] + len(a[2]) > b[1] for a, b in zip(sections, sections[1:])):
        raise ValueError(f"{path}: overlapping executable sections")
    return sections


def masked_code(disassembler, code, rva):
    masked = []
    for instruction in disassembler.disasm(code, rva):
        raw = list(instruction.bytes)
        for offset, size in (
            (instruction.disp_offset, instruction.disp_size),
            (instruction.imm_offset, instruction.imm_size),
        ):
            if size == 4:
                raw[offset:offset + size] = [None] * size
        masked.extend(raw)
    return masked


def candidates(masked, sections):
    votes = Counter()
    for offset in range(0, min(320, len(masked) - 27), 5):
        window = masked[offset:offset + 28]
        runs = re.finditer(r"x+", "".join(" " if byte is None else "x" for byte in window))
        anchor = max(runs, key=lambda run: len(run.group()), default=None)
        if anchor is None or len(anchor.group()) < 5:
            continue
        needle = bytes(window[anchor.start():anchor.end()])
        pattern = re.compile(
            b"".join(b"." if byte is None else re.escape(bytes([byte])) for byte in window),
            re.DOTALL,
        )
        for _, rva, section in sections:
            hit = section.find(needle)
            while hit >= 0:
                start = hit - anchor.start()
                if start >= offset and pattern.match(section, start):
                    votes[rva + start - offset] += 1
                hit = section.find(needle, hit + 1)
    return votes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference_image", type=Path, help="PE or rebuilt PE of a working build")
    parser.add_argument("reference_profile", type=Path, help="Working build's profile INI")
    parser.add_argument("target_image", type=Path, help="PE or rebuilt PE to search")
    args = parser.parse_args()
    profile = ConfigParser(interpolation=None, inline_comment_prefixes=(";", "#"))
    try:
        import capstone

        profile.read_string(args.reference_profile.read_text(encoding="utf-8-sig"))
        reference = read_code(args.reference_image)
        target = read_code(args.target_image)
        disassembler = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        disassembler.detail = True
        for key in CODE_ANCHORS:
            value = int(profile["range"][key], 0)
            code = next(
                (data[value - rva:value - rva + 420]
                 for _, rva, data in reference if rva <= value < rva + len(data)),
                None,
            )
            if code is None:
                raise ValueError(f"{key}: reference RVA {value:#010x} is outside executable sections")
            matches = candidates(masked_code(disassembler, code, value), target)
            ranked = sorted(matches.items(), key=lambda item: (-item[1], item[0]))[:4]
            top = ", ".join(f"{rva:#010x} ({count} windows)" for rva, count in ranked)
            print(f"{key:18} {value:#010x} -> {top or 'no candidates'}")
    except ImportError:
        parser.error("install the Capstone version listed in scripts/requirements-dev.txt")
    except (ConfigError, OSError, ValueError, KeyError, struct.error) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
