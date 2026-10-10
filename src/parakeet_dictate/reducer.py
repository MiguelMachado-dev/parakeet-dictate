from dataclasses import dataclass, field
from enum import Enum


class Mode(Enum):
    HOLD = "hold"
    TOGGLE = "toggle"


class Cue(Enum):
    START = "start"
    STOP = "stop"
    CANCEL = "cancel"
    ERROR = "error"


@dataclass(frozen=True)
class Idle:
    key_down: bool = False


@dataclass(frozen=True)
class Recording:
    mode: Mode
    pressed_at: float

    @property
    def key_down(self) -> bool:
        return self.mode is Mode.HOLD


@dataclass(frozen=True)
class Transcribing:
    key_down: bool


@dataclass(frozen=True)
class AwaitingRelease:
    text: str = field(repr=False)

    @property
    def key_down(self) -> bool:
        return True


type State = Idle | Recording | Transcribing | AwaitingRelease


@dataclass(frozen=True)
class HotkeyDown:
    t: float


@dataclass(frozen=True)
class HotkeyUp:
    t: float


@dataclass(frozen=True)
class OtherKeyDown:
    pass


@dataclass(frozen=True)
class Cancel:
    pass


@dataclass(frozen=True)
class TranscriptReady:
    text: str = field(repr=False)


@dataclass(frozen=True)
class RecordingFailed:
    error: str


type Event = HotkeyDown | HotkeyUp | OtherKeyDown | Cancel | TranscriptReady | RecordingFailed


@dataclass(frozen=True)
class StartRecording:
    pass


@dataclass(frozen=True)
class StopAndTranscribe:
    pass


@dataclass(frozen=True)
class DiscardRecording:
    pass


@dataclass(frozen=True)
class Paste:
    text: str = field(repr=False)


@dataclass(frozen=True)
class PlaySound:
    cue: Cue


type Effect = StartRecording | StopAndTranscribe | DiscardRecording | Paste | PlaySound


def step(state: State, event: Event, tap_threshold: float = 0.3) -> tuple[State, list[Effect]]:
    match state, event:
        case Idle(key_down=False), HotkeyDown(t):
            return Recording(Mode.HOLD, t), [StartRecording(), PlaySound(Cue.START)]
        case Idle(), HotkeyUp():
            return Idle(), []

        case Recording(mode=Mode.HOLD, pressed_at=pressed_at), HotkeyUp(t):
            if t - pressed_at < tap_threshold:
                return Recording(Mode.TOGGLE, pressed_at), []
            return Transcribing(key_down=False), [StopAndTranscribe(), PlaySound(Cue.STOP)]
        case Recording(mode=Mode.TOGGLE), HotkeyDown():
            return Transcribing(key_down=True), [StopAndTranscribe(), PlaySound(Cue.STOP)]
        case Recording(mode=Mode.HOLD), OtherKeyDown():
            return Idle(key_down=True), [DiscardRecording()]
        case Recording(key_down=key_down), Cancel():
            return Idle(key_down), [DiscardRecording(), PlaySound(Cue.CANCEL)]
        case Recording(key_down=key_down), RecordingFailed():
            return Idle(key_down), [DiscardRecording(), PlaySound(Cue.ERROR)]

        case Transcribing(), HotkeyDown():
            return Transcribing(key_down=True), []
        case Transcribing(), HotkeyUp():
            return Transcribing(key_down=False), []
        case Transcribing(key_down=key_down), TranscriptReady(text):
            if not text:
                return Idle(key_down), []
            if key_down:
                return AwaitingRelease(text), []
            return Idle(), [Paste(text)]
        case Transcribing(key_down=key_down), RecordingFailed():
            return Idle(key_down), [PlaySound(Cue.ERROR)]

        case AwaitingRelease(text), HotkeyUp():
            return Idle(), [Paste(text)]
        case AwaitingRelease(), Cancel():
            return Idle(key_down=True), [PlaySound(Cue.CANCEL)]

    return state, []
