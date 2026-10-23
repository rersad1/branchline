"""Generate the tiny original Windows ICO used by the Tauri application."""

from pathlib import Path
import struct


SIZE = 32
BACKGROUND = (29, 42, 63, 255)  # RGBA
ACCENT = (80, 210, 190, 255)


def pixel(x: int, y: int) -> tuple[int, int, int, int]:
    # A simple linked-worktree glyph: trunk, branch, and three round nodes.
    on_line = (x == 9 and 7 <= y <= 25) or (9 <= x <= 22 and y == 13) or (x == 22 and 13 <= y <= 23)
    on_node = any((x - cx) ** 2 + (y - cy) ** 2 <= 2**2 for cx, cy in ((9, 7), (9, 13), (22, 23)))
    red, green, blue, alpha = ACCENT if on_line or on_node else BACKGROUND
    return blue, green, red, alpha


def main() -> None:
    # ICO embeds a 32-bit BMP. Its doubled height accounts for the AND mask.
    pixels = b"".join(bytes(pixel(x, y)) for y in reversed(range(SIZE)) for x in range(SIZE))
    mask = bytes(((SIZE + 31) // 32) * 4 * SIZE)
    image_size = 40 + len(pixels) + len(mask)
    bmp_header = struct.pack("<IiiHHIIiiII", 40, SIZE, SIZE * 2, 1, 32, 0, len(pixels), 0, 0, 0, 0)
    directory = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", SIZE, SIZE, 0, 0, 1, 32, image_size, 22)
    output = Path(__file__).resolve().parents[1] / "icons" / "icon.ico"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(directory + entry + bmp_header + pixels + mask)
    print(f"Wrote {output} ({output.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
