"""What the overlay shows and when. Maps dictation transitions to commands and plays them out
over time. Pure and clock-free, so tests drive it without a window."""

from dataclasses import dataclass

import numpy as np
import numpy.typing as npt

from .overlay_frame import BEAT_S, CANVAS_SIZE, OverlayFrame, Phase
from .reducer import AwaitingRelease, Cue, Effect, Idle, Mode, OtherKeyDown, Paste, PlaySound, Polishing, Recording, Transcribing, Transition

# A recording only appears after this long, so the hotkey pressed as part of a shortcut such as
# Right Ctrl+C never flashes the window. The start tone is the instant feedback.
SHOW_DELAY_S = 0.25
FADE_IN_S = 0.15
FADE_OUT_S = 0.2
# Logical px between the bottom of the canvas and the bottom of the work area.
BOTTOM_MARGIN = 48


@dataclass(frozen=True)
class Show:
    phase: Phase
    hands_free: bool = False


@dataclass(frozen=True)
class Finish:
    """Ends the dictation: plays the beat for BEAT_S, then fades out. No beat fades out at once."""

    beat: Phase | None


@dataclass(frozen=True)
class Vanish:
    """Ends the dictation with no beat and no fade."""


type Command = Show | Finish | Vanish


def command_for(transition: Transition) -> Command | None:
    match transition.state:
        case Recording(mode=mode):
            return Show(Phase.RECORDING, hands_free=mode is Mode.TOGGLE)
        case Transcribing() | AwaitingRelease():
            return Show(Phase.TRANSCRIBING)
        case Polishing():
            return Show(Phase.POLISHING)
        case Idle() if isinstance(transition.previous, Idle):
            return None
        case Idle() if isinstance(transition.event, OtherKeyDown):
            return Vanish()
        case Idle():
            return Finish(_beat(transition.effects))


def _beat(effects: tuple[Effect, ...]) -> Phase | None:
    for effect in effects:
        match effect:
            case Paste():
                return Phase.DONE
            case PlaySound(Cue.CANCEL):
                return Phase.CANCELLED
            case PlaySound(Cue.ERROR):
                return Phase.FAILED
    return None


@dataclass(frozen=True)
class Placement:
    """Physical pixels on the virtual screen."""

    x: int
    y: int
    width: int
    height: int
    scale: float


def place(work: tuple[int, int, int, int], dpi: int) -> Placement:
    """Bottom center of a monitor's work area (left, top, right, bottom) at the monitor's DPI."""
    scale = dpi / 96
    width, height = round(CANVAS_SIZE[0] * scale), round(CANVAS_SIZE[1] * scale)
    left, _, right, bottom = work
    return Placement(left + (right - left - width) // 2, bottom - round(BOTTOM_MARGIN * scale) - height, width, height, scale)


@dataclass
class _Appearance:
    """One dictation on screen, from its fade-in to the end of its fade-out."""

    shows_at: float
    recording_started: float
    placement: Placement
    phase: Phase
    phase_started: float
    hands_free: bool
    recording_ended: float | None = None
    fades_at: float | None = None
    # Presence at shows_at: above 0 when the previous dictation was still fading out.
    rise_from: float = 0.0

    def presence(self, now: float) -> float:
        def rising(t: float) -> float:
            return min(1.0, self.rise_from + (t - self.shows_at) / FADE_IN_S)

        if now < self.shows_at:
            return 0.0
        if self.fades_at is None or now < self.fades_at:
            return rising(now)
        return max(0.0, rising(self.fades_at) - (now - self.fades_at) / FADE_OUT_S)

    def gone(self, now: float) -> bool:
        return self.fades_at is not None and now >= self.fades_at and self.presence(now) <= 0.0


class Scene:
    def __init__(self) -> None:
        # The latest dictation, possibly still waiting for SHOW_DELAY_S.
        self._current: _Appearance | None = None
        # The dictation before it, still ending on screen while _current waits.
        self._outgoing: _Appearance | None = None

    def apply(self, command: Command, t: float, placement: Placement) -> None:
        """placement is where a dictation that starts with this command will show."""
        self._advance(t)
        current = self._current
        if current is None or current.fades_at is not None:
            if isinstance(command, Show):
                self._outgoing = current
                self._current = _Appearance(
                    t + SHOW_DELAY_S, t, placement, command.phase, t + SHOW_DELAY_S, command.hands_free
                )
            return
        match command:
            case Show(phase, hands_free):
                if phase is not current.phase:
                    current.phase, current.phase_started = phase, max(t, current.shows_at)
                if phase is not Phase.RECORDING and current.recording_ended is None:
                    current.recording_ended = t
                current.hands_free |= hands_free
            case Finish() | Vanish() if t < current.shows_at:
                # Never shown, so it never will be: whatever was ending on screen plays on untouched.
                self._current, self._outgoing = self._outgoing, None
            case Finish(beat):
                if current.recording_ended is None:
                    current.recording_ended = t
                if beat is None:
                    current.fades_at = t
                else:
                    current.phase, current.phase_started, current.fades_at = beat, t, t + BEAT_S
            case Vanish():
                self._current = None

    def frame(
        self, now: float, levels: npt.NDArray[np.float32], spectrum: npt.NDArray[np.float32]
    ) -> tuple[OverlayFrame, Placement] | None:
        """The frame to draw at now, or None when the window should be hidden."""
        self._advance(now)
        shown = self._outgoing or self._current
        if shown is None or now < shown.shows_at:
            return None
        frame = OverlayFrame(
            phase=shown.phase,
            phase_elapsed=now - shown.phase_started,
            presence=shown.presence(now),
            hands_free=shown.hands_free,
            recording_elapsed=(now if shown.recording_ended is None else shown.recording_ended) - shown.recording_started,
            levels=levels,
            spectrum=spectrum,
            scale=shown.placement.scale,
        )
        return frame, shown.placement

    def next_show(self) -> float | None:
        """When frame() returns None: the time a dictation waiting for SHOW_DELAY_S appears, if any."""
        return self._current.shows_at if self._current is not None else None

    def _advance(self, now: float) -> None:
        current = self._current
        if current is not None and self._outgoing is not None and now >= current.shows_at:
            current.rise_from = self._outgoing.presence(current.shows_at)
            self._outgoing = None
        if self._outgoing is not None and self._outgoing.gone(now):
            self._outgoing = None
        if current is not None and current.gone(now):
            self._current = None
