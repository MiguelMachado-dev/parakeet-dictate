import ctypes
import time

import numpy as np
from PIL import Image, ImageGrab

from parakeet_dictate.overlay import _Window, premultiplied_bgra
from parakeet_dictate.overlay_scene import Placement

user32 = ctypes.WinDLL("user32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi")
GWL_EXSTYLE = -20
# WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
NEVER_FOCUSED_CLICK_THROUGH = 0x00080000 | 0x00000020 | 0x08000000 | 0x00000080 | 0x00000008
STRAIGHT = (255, 128, 0, 128)


def test_premultiplied_bgra_on_a_known_pixel():
    pixel = np.array([[STRAIGHT]], np.uint8)
    # 255 * 128 / 255 = 128 and 128 * 128 / 255 = 64.25, in B, G, R, A order.
    assert premultiplied_bgra(pixel).tolist() == [[[0, 64, 128, 128]]]


def test_premultiplied_bgra_never_exceeds_alpha():
    rgba = np.random.default_rng(0).integers(0, 256, (64, 64, 4), dtype=np.uint8)
    bgra = premultiplied_bgra(rgba)
    assert (bgra[..., :3] <= bgra[..., 3:4]).all()
    assert (bgra[rgba[..., 3] == 255][:, :3] == rgba[rgba[..., 3] == 255][:, 2::-1]).all()


def test_update_layered_window_composites_the_pixel_as_straight_alpha_over_the_desktop():
    """Draw one known pixel through the real window and read the desktop back."""
    box = (0, 0, 8, 8)
    window = _Window()
    try:
        style = user32.GetWindowLongW(window.hwnd, GWL_EXSTYLE)
        assert style & NEVER_FOCUSED_CLICK_THROUGH == NEVER_FOCUSED_CLICK_THROUGH
        background = ImageGrab.grab(box, include_layered_windows=True).convert("RGB")
        window.draw(Image.new("RGBA", (8, 8), STRAIGHT), Placement(0, 0, 8, 8, 1.0))
        dwmapi.DwmFlush()
        time.sleep(0.05)
        shown = ImageGrab.grab(box, include_layered_windows=True).convert("RGB")
    finally:
        window.hide()
        window.destroy()
    expected = Image.alpha_composite(background.convert("RGBA"), Image.new("RGBA", (8, 8), STRAIGHT)).convert("RGB")
    difference = np.abs(np.asarray(shown, np.int16) - np.asarray(expected, np.int16))
    assert difference.max() <= 2, f"desktop {np.asarray(shown)[0, 0]} vs straight-alpha composite {np.asarray(expected)[0, 0]}"
