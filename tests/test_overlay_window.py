import ctypes
import time

import numpy as np
from PIL import Image, ImageGrab

from parakeet_dictate.meter import LevelMeter
from parakeet_dictate.overlay import Overlay, _Window, premultiplied_bgra
from parakeet_dictate.overlay_frame import FPS, OverlayFrame
from parakeet_dictate.overlay_scene import SHOW_DELAY_S, Placement
from parakeet_dictate.reducer import HotkeyDown, Idle, Transition, step

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


class Counting:
    def __init__(self) -> None:
        self.frames = 0

    def __call__(self, frame: OverlayFrame) -> Image.Image:
        self.frames += 1
        return Image.new("RGBA", frame.size, STRAIGHT)


def wait_for(probe, what: str, timeout_s: float = 2.0) -> None:
    deadline = time.perf_counter() + timeout_s
    while not probe():
        assert time.perf_counter() < deadline, f"timed out waiting for {what}"
        time.sleep(0.002)


def start_recording(overlay: Overlay) -> None:
    event = HotkeyDown(time.perf_counter())
    state, effects = step(Idle(), event)
    overlay.notify(Transition(Idle(), state, event, tuple(effects)))


def test_restyle_swaps_the_renderer_mid_dictation_and_off_hides_the_window_until_restyled():
    first, second = Counting(), Counting()
    overlay = Overlay(first, LevelMeter(16_000).snapshot)
    visible = lambda: bool(user32.IsWindowVisible(overlay.hwnd))
    try:
        start_recording(overlay)
        wait_for(visible, "the recording to show")
        overlay.restyle(second)
        wait_for(lambda: second.frames > 0, "the second renderer to draw")
        drawn_by_first = first.frames
        overlay.restyle(None)
        wait_for(lambda: not visible(), "off to hide the window", timeout_s=3 / FPS)
        assert first.frames == drawn_by_first

        start_recording(overlay)
        time.sleep(SHOW_DELAY_S + 0.1)
        assert not visible(), "a transition while off showed the window"

        overlay.restyle(first)
        start_recording(overlay)
        wait_for(visible, "the window to show again once a renderer is set")
        assert first.frames > drawn_by_first
    finally:
        overlay.close()
