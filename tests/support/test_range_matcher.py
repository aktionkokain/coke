"""Exercise the matcher on synthetic PE32 input without game data."""
import runpy
import struct
import tempfile
import unittest
from pathlib import Path


MATCHER = runpy.run_path(str(Path(__file__).parents[2] / "scripts/match-range-rvas.py"))


def pe32(code):
    image = bytearray(0x400)
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, 0x80)
    image[0x80:0x84] = b"PE\x00\x00"
    struct.pack_into("<HH", image, 0x84, 0x14C, 1)
    struct.pack_into("<H", image, 0x94, 0xE0)
    struct.pack_into("<H", image, 0x98, 0x10B)
    struct.pack_into("<I", image, 0x98 + 56, 0x2000)
    struct.pack_into("<8sIIII", image, 0x178, b".text", len(code), 0x1000, 0x200, 0x200)
    struct.pack_into("<I", image, 0x178 + 36, 0x60000020)
    image[0x200:0x200 + len(code)] = code
    return image


class RangeMatcher(unittest.TestCase):
    def test_extracts_and_matches_a_synthetic_section(self):
        code = bytes(range(75))
        with tempfile.TemporaryDirectory() as temp:
            image = Path(temp) / "test.exe"
            image.write_bytes(pe32(code))
            sections = MATCHER["read_code"](image)
        self.assertEqual(sections, [(b".text", 0x1000, code)])
        votes = MATCHER["candidates"](list(code), sections)
        self.assertEqual(list(votes), [0x1000])

    def test_rejects_truncated_sections_and_wrong_optional_header(self):
        with tempfile.TemporaryDirectory() as temp:
            image = Path(temp) / "test.exe"
            valid = pe32(b"\xc3")
            image.write_bytes(valid[:-1])
            with self.assertRaisesRegex(ValueError, "file span"):
                MATCHER["read_code"](image)
            struct.pack_into("<H", valid, 0x98, 0x20B)
            image.write_bytes(valid)
            with self.assertRaisesRegex(ValueError, "32-bit"):
                MATCHER["read_code"](image)


if __name__ == "__main__":
    unittest.main()
