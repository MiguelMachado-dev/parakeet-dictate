# Mac client brief

The app this brief asked for is now in [mac/](../mac/README.md). It also transcribes on the Mac itself, so it keeps the reducer's `Polishing` state for that mode; the HTTP contract below is unchanged.

You are building a native macOS menu-bar app that dictates through a Windows PC on the same network. The PC runs Parakeet Dictate, the Python app in this repository. It transcribes speech with NVIDIA Parakeet and cleans the transcript with GLM or Claude, using the provider, key, and settings in the PC's `config.toml`. The Mac's provider choice does not select the PC's. Your app owns the hotkey, the recording, the overlay, and the paste. The PC only turns audio into text.

This document is self-contained. Read it in full before writing code. File references point into this repository, which is cloned on the Mac; the Windows code is the reference behavior to replicate. Paths are relative to the repository root.

## The HTTP contract

The server is part of the Windows app. The user turns it on from the tray menu with **Server for Mac**. Its source is `src/parakeet_dictate/server.py`, and its tests in `tests/test_server.py` pin every behavior below.

### Connection

- Base URL: `http://<PC LAN IPv4>:<port>`, for example `http://192.168.3.2:8765`. Plain HTTP, no TLS. The default port is 8765.
- The tray item **Copy Mac connection info** puts two lines on the Windows clipboard: the base URL, then the token. Accept that two-line text in your settings window so the user can paste both at once.
- Every response carries `Connection: close`. Open a new connection per request; do not rely on keep-alive.
- Every response body is JSON, `Content-Type: application/json; charset=utf-8`.
- Every error response has the shape `{"error": {"message": "<human-readable reason>"}}`, the OpenAI error shape.

### Authentication

Every endpoint needs the header `Authorization: Bearer <token>`. A missing or wrong token gets `401` with `{"error": {"message": "missing or wrong bearer token"}}`. The token is 43 URL-safe characters. Store it in the Keychain.

### `GET /v1/health`

Returns `200` and:

```json
{"status": "ok", "postprocess": true}
```

- `postprocess` is `true` when post-processing is on in the Windows tray and the PC has the selected provider's API key. The reply does not identify the provider.
- Side effect: when `postprocess` is `true`, the PC opens its post-processing connection in the background, without delaying this response. For GLM this saved about 0.9 s of DNS, TCP and TLS. Both clients keep idle connections for 15 s.
- The response does not say whether the model has finished loading. A transcription request answers that with `503`.

Call it at two moments:

1. At hotkey-down, fire and forget, in parallel with starting the recording. This mirrors the Windows app, which warms its post-processing connection when a recording starts (`src/parakeet_dictate/app.py`). Do not wait for the reply before recording.
2. To show the connection state in the menu: on launch, after the settings change, and on a slow timer while idle, such as every 60 s.

### `POST /v1/audio/transcriptions`

OpenAI-compatible multipart upload, so tools such as VoiceInk also work against it.

Request:

- `Content-Type: multipart/form-data; boundary=...`, with a `Content-Length` header. Chunked uploads are not supported and get `411`.
- Field `file`, required: a WAV file of 16-bit PCM samples. Mono or stereo; stereo is averaged to mono. Any sample rate; the server resamples. Send 16 kHz mono 16-bit, which is what the model uses and the smallest upload.
- Field `postprocess`, optional: exactly `true` or `false`. Omitted means the Windows tray choice. `true` on a PC without the selected provider's key returns the raw text with `polished: false`.
- Field `instructions`, optional: UTF-8 text that the PC's selected provider follows for this dictation only, after its usual cleanup rules and taking precedence over them, such as `Translate to English.` The Mac sends the rules set for the app the text goes to. It only matters when the text is post-processed, so send `postprocess=true` with it. An older PC ignores it.
- Fields `model`, `language`, `response_format` and `temperature` are accepted and ignored.

Responses:

| Status | When | Body |
| --- | --- | --- |
| `200` | Transcribed | `{"text": "<final text>", "raw": "<Parakeet transcript>", "polished": true}` |
| `200` | Post-processing failed, or gave no reply within its timeout (8 s by default) | `{"text": "<raw>", "raw": "<raw>", "polished": false}` |
| `200` | No speech, or audio under 0.1 s | `{"text": "", "raw": "", "polished": false}` |
| `400` | The file is not a 16-bit PCM WAV (24-bit, 32-bit float, not a WAV), no `file` field, not multipart, `postprocess` is neither `true` nor `false`, or `instructions` is not UTF-8 | error naming the expected format |
| `401` | Missing or wrong token | error |
| `404` | Unknown path | error |
| `411` | No `Content-Length` | error |
| `413` | Body over 115,265,536 bytes, about 10 minutes of 48 kHz stereo | error |
| `503` | The model is still loading, which takes a few seconds after the Windows app starts | error |

Paste `text`. A post-processing failure is never a 5xx: it is a `200` with `polished: false`, and `text` is the raw transcript. Treat empty `text` as "nothing to paste": play no paste and no error tone.

The PC runs one transcription at a time. A request that arrives while the PC transcribes a local dictation or another request waits for it, then runs. Set the request timeout to at least 60 s. Measured on the PC over loopback with the CPU model, a 6 s clip took 2.2 to 3.1 s to transcribe, and GLM added 1.5 to 1.8 s. The GPU model, which the app uses by default, is faster; no figure was measured for this brief.

### Examples

```sh
URL=http://192.168.3.2:8765
TOKEN=...   # second line of "Copy Mac connection info"

curl -sS -H "Authorization: Bearer $TOKEN" "$URL/v1/health"
# {"status": "ok", "postprocess": true}

curl -sS -H "Authorization: Bearer $TOKEN" \
  -F "file=@speech.wav;type=audio/wav" -F postprocess=false \
  "$URL/v1/audio/transcriptions"
# {"text": "The quick brown fox jumps over the lazy dog.", "raw": "The quick brown fox jumps over the lazy dog.", "polished": false}

curl -sS -H "Authorization: Bearer $TOKEN" \
  -F "file=@speech.wav;type=audio/wav" \
  "$URL/v1/audio/transcriptions"
# {"text": "...", "raw": "...", "polished": true}
```

`tests/fixtures/en.wav` and `tests/fixtures/pt.wav` are 16 kHz mono 16-bit speech with their expected text in the `.txt` files next to them. Use them for the first request.

## Behavior to replicate

The Windows app's dictation logic is a pure state machine in `src/parakeet_dictate/reducer.py`. Port it to Swift as an enum of states, an enum of events, and a pure `step(state, event) -> (state, [effect])` function, then run its effects in one place. `tests/test_reducer.py` lists the cases; port those tests too. Replace the local "transcribe" effect with the HTTP request, and drop the `Polish` effect and the `Polishing` state: the server polishes inside the request.

- **Hold to talk, or tap for hands-free.** Hotkey-down starts recording (`reducer.py:148-149`). Releasing at least 300 ms after the press stops and transcribes. Releasing sooner switches to hands-free: recording continues until the next hotkey press (`reducer.py:153-158`). The 300 ms threshold is `tap_threshold_ms` in `src/parakeet_dictate/config.py`.
- **Esc cancels.** During a recording, Esc discards the audio and plays the cancel tone (`reducer.py:161-162`). While the text is on its way, the Windows app pastes the raw transcript on Esc (`reducer.py:185-186`); your app has no raw transcript until the reply, so on Esc during the request, cancel the request and paste nothing. Swallow the Esc only when the hotkey is held or a request is in flight, so Esc keeps working in other apps (`src/parakeet_dictate/app.py:149-175`).
- **Other keys during a short press.** In hold mode, any other key pressed while the hotkey is held means the user typed a shortcut, such as Right Ctrl+C. Discard the recording silently: no tone, no request (`reducer.py:159-160`, `app.py:169`).
- **The hotkey during a request.** Pressing the hotkey while a request is in flight does not start a new recording. If the reply arrives while the hotkey is still held, wait for the release before pasting, so the paste is not combined with the held modifier (`reducer.py:166-169`, `reducer.py:188-189`, `reducer.py:196-199`).
- **Sounds.** Four short tones (`app.py:282-318`): start (600 Hz then 900 Hz), stop (900 then 600), cancel (450), and error (300, 300). Each note lasts 60 ms with an 8 ms fade and a 20 ms gap, at 12% volume. Play start on recording start, stop when the recording stops, cancel on Esc, and error on any failure. Synthesize them with `AVAudioEngine`, or render them once to files and play them with `NSSound`.
- **Paste through the clipboard.** Save the current clipboard, put the text on it, post Cmd+V, wait 200 ms, then restore the saved clipboard, but only if nothing else changed the clipboard meanwhile (`src/parakeet_dictate/inject.py:89-96`). On macOS compare `NSPasteboard.general.changeCount` before and after, and save every pasteboard item type, not only text. Mark the transient write with the `org.nspasteboard.TransientType` type so clipboard managers skip it.
- **Append a space.** Paste `text + " "` so consecutive dictations do not run together (`app.py:237`, setting `append_space`, default on). Make it a setting.
- **Network failure.** On a connection error, a timeout, or any non-200 status, play the error tone, show the failed state in the overlay, log the status and the error message, and paste nothing. Never paste partial text. On `503`, say in the menu that the PC is still loading. On `401`, say the token is wrong. A failed `/v1/health` call only updates the menu's connection state; it plays no tone and never blocks a recording.

## The overlay to port

The overlay is a small animated visualizer near the bottom of the screen while you dictate. The Windows code is split so that everything except the window is portable.

- `src/parakeet_dictate/meter.py` turns microphone samples into levels and a spectrum: 20 ms hops, 96 levels of history, 24 bands from 80 Hz to 8 kHz. Feed it from the audio tap.
- `src/parakeet_dictate/overlay_frame.py` defines the frame a renderer draws: a 400 by 120 logical-pixel canvas, 60 fps, and the phases (recording, transcribing, polishing, done, cancelled, failed). The done, cancelled and failed beats last 0.45 s.
- `src/parakeet_dictate/overlay_scene.py` maps each state transition to a command (show, finish with a beat, vanish) and plays it out over time. It is pure and clock-free; port it as is.
- `src/parakeet_dictate/styles/fluid.py` (the default) and `src/parakeet_dictate/styles/bars.py` draw a frame into an RGBA image with NumPy. Port the drawing to Core Graphics or Metal, keeping the shapes, colors and timings. Use the polishing look for the whole request, since the Mac cannot tell transcription from post-processing.
- Timings: a recording appears only after 250 ms (`SHOW_DELAY_S`), so a hotkey pressed as part of a shortcut never flashes the window; the start tone is the instant feedback. Fade in over 150 ms, fade out over 200 ms. A shortcut (another key during a short press) vanishes with no fade.
- Placement: bottom center of the screen that holds the focused window, 48 points above the bottom of the visible frame (`overlay_scene.py:81-86`, `src/parakeet_dictate/overlay.py:249-261`). On macOS use `NSScreen.visibleFrame`, which excludes the Dock.
- Window: it must never take focus or receive clicks. On Windows that is a layered, transparent, no-activate, topmost tool window (`overlay.py:33-38`). On macOS use a borderless `NSPanel` with the `.nonactivatingPanel` style mask, `ignoresMouseEvents = true`, `isOpaque = false`, a clear background, a level such as `.statusBar`, and `collectionBehavior` of `[.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]`, so it shows on every Space and over full-screen apps. Order it front with `orderFrontRegardless()`, never `makeKeyAndOrderFront`.
- Animate only while visible. The Windows overlay sleeps with no timer while hidden.

## Suggested shape

- A Swift Package Manager executable, a menu-bar app with `NSStatusItem` and no Dock icon (`NSApplication.ActivationPolicy.accessory`). The menu shows the connection state, a **Settings…** item, and **Quit**.
- Capture with `AVAudioEngine`: tap the input node, convert with `AVAudioConverter` to 16 kHz mono, and write a WAV of 16-bit little-endian PCM in memory (a 44-byte RIFF header plus the samples). Feed the meter from the same tap.
- The global hotkey through a `CGEventTap` on key-down, key-up and flags-changed events. Make it an active tap (`.defaultTap`, not `.listenOnly`) so the callback can swallow Esc by returning `nil`. It needs the Accessibility permission, which posting Cmd+V with `CGEvent` also needs. A modifier key used as the hotkey arrives as flags-changed events with its key code. The Windows default is Right Ctrl; a reasonable Mac default is Right Command, configurable. The tap must return quickly and hand events to your state machine, as `app.py:149-175` does. Tag the Cmd+V you post (for example in `CGEvent` field `eventSourceUserData`) and ignore tagged events in the tap, as `inject.INJECTED_TAG` does on Windows.
- The Microphone permission: `NSMicrophoneUsageDescription` in `Info.plist`. A plain SwiftPM executable has no bundle, so build it as an `.app` bundle with an `Info.plist`, and sign it, so macOS remembers the Accessibility and Microphone grants.
- Settings: the base URL and the token, stored in the Keychain, with a field that accepts the two-line text from **Copy Mac connection info**; the hotkey; the append-space toggle; and the post-processing choice (follow the PC, on, or off), sent as the `postprocess` field.
- App Transport Security. This app uses plain `http://` to a raw LAN IP. The user expects that to work without an ATS exception, and a hostname such as `mypc.local` may not. This brief cites no Apple source for either; both are unverified. Check it with the first request from the app itself, since `curl` is not subject to ATS. If `URLSession` fails with `NSURLErrorAppTransportSecurityRequiresSecureConnection` (-1022, from memory), add `NSAppTransportSecurity` with `NSAllowsLocalNetworking` set to `true` to `Info.plist`. Also unverified: whether macOS shows a Local Network privacy prompt for this app; if it does, add `NSLocalNetworkUsageDescription`.

## Verify on the Mac

1. On the PC, the user turns on **Server for Mac**, runs `.\scripts\allow_mac.ps1` once to open the Windows Firewall port for the local subnet, and clicks **Copy Mac connection info**.
2. From the Mac terminal, check the network path before writing app code:

   ```sh
   curl -sS -m 5 -H "Authorization: Bearer $TOKEN" "$URL/v1/health"
   ```

   A `200` with `{"status": "ok", ...}` means the path works. A timeout usually means the firewall rule is missing or the URL has the wrong address; a VPN on either machine can also route the LAN address away. A `401` means the token is wrong.
3. Post a fixture and compare the text with `tests/fixtures/en.txt`:

   ```sh
   curl -sS -H "Authorization: Bearer $TOKEN" -F "file=@tests/fixtures/en.wav;type=audio/wav" -F postprocess=false "$URL/v1/audio/transcriptions"
   ```

4. Then run the app: hold the hotkey, say a sentence, release, and check that the text lands in a text editor followed by a space, and that the clipboard holds what it held before. Then check a quick tap for hands-free, Esc during a recording, a shortcut such as the hotkey plus C, and an unplugged network, which must play the error tone and paste nothing.
