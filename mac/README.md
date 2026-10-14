# Parakeet Dictate for Mac

A menu bar app with the Windows app's dictation: hold a key, talk, release, and the text is pasted where you type. It transcribes in one of two places, chosen in the menu:

- **This Mac.** Parakeet TDT 0.6B v3, the PC's model, converted to Core ML and run on the Neural Engine through [FluidAudio](https://github.com/FluidInference/FluidAudio). A 6 s clip takes about 50 ms on an M5 Pro. The transcript then goes from the Mac to GLM through Z.ai or to Claude through Anthropic, whichever Settings names, with the same prompt and checks as `src/parakeet_dictate/polish.py`.
- **The PC.** The audio goes to the Windows app's **Server for Mac**, which transcribes and post-processes it and sends back the text. See [Dictate from a Mac](../README.md#dictate-from-a-mac) to turn the server on, and [docs/mac-client.md](../docs/mac-client.md) for the HTTP contract.

## Build

Needs macOS 14 or later on Apple Silicon and the Command Line Tools (`xcode-select --install`); Xcode is not needed.

```sh
mac/scripts/build-app.sh            # builds mac/build/Parakeet Dictate.app
mac/scripts/build-app.sh --install  # also copies it to /Applications and opens it
```

The script signs the app with a certificate it creates on first run, in its own keychain under `~/Library/Application Support/ParakeetDictate/signing`. macOS ties the Microphone and Accessibility permissions to the signature; with a stable certificate they survive rebuilds, where an ad-hoc signature would make macOS ask again after every build. The certificate signs nothing else. Delete that folder to remove it; the next build makes a new one, and macOS asks for the permissions once more.

## First start

1. macOS asks for the microphone. Allow it.
2. macOS asks for Accessibility, which the app needs to see the hotkey and to paste with Cmd+V. Open System Settings, turn on **Parakeet Dictate** under Privacy & Security, Accessibility. The app notices within a second; no restart.
3. On This Mac, the first start downloads the model, about 470 MB, to `~/Library/Application Support/FluidAudio/Models`, and compiles it for the Neural Engine, about 12 s more. The menu shows the progress. Later starts, rebuilds included, load it in under half a second.

## Dictate

- Hold **Right Command**, talk, and release. The text is pasted followed by a space, and the clipboard gets back what it held.
- Tap the key, shorter than 300 ms, for hands-free: it records until you press the key again.
- **Esc** while recording discards the audio. Esc while the text is on its way: on This Mac, during post-processing, it pastes the raw transcript, like the Windows app; on the PC it cancels the request and pastes nothing.
- Another key or a click while holding the hotkey, such as Command-C or Command-click, is a shortcut: the recording is dropped without a sound.
- The overlay, the Windows app's visualizer, shows near the bottom of the screen that holds the focused window, or just under the menu bar if you choose Top: it follows your voice while recording, then shows the text on its way, a check when it is pasted, grey on cancel, and red on failure. It appears 250 ms after the press, so a shortcut never flashes it, and it never takes focus or clicks. On the PC, the whole request shows the post-processing look when post-processing is on, since the Mac cannot tell the two apart.
- Music or a video that is playing pauses when the overlay appears and plays again when the recording stops. Only what macOS shows as Now Playing pauses, the app that the keyboard's play key controls, not every sound. Nothing starts when nothing was playing. This uses Apple's private MediaRemote framework, so a macOS update could break it; turn it off with **Pause Media While Recording** in the menu.
- The menu bar icon shows the state: a waveform when ready, a microphone while recording, an ellipsis while the text is on its way, and a warning sign when something needs attention. The menu's first line says what.

If another dictation app is running with the same hotkey, such as Superwhisper on Right Command, quit it or change one of the hotkeys.

## Settings

Open **Settings…** from the menu.

| Setting | What it does |
| --- | --- |
| Transcribe on | This Mac or the PC. Also in the menu. |
| Hotkey | Right Command (default), Right Option, Right Control, Right Shift, or F13. |
| Add a space after each dictation | On by default. |
| Overlay | Fluid (default), Bars, or Off. Also in the menu. |
| Overlay position | Bottom (default) or Top, just under the menu bar. A Mac addition. Also in the menu's Overlay submenu. |
| Sounds | The start, stop, cancel, and error tones of the Windows app. |
| Pause media while recording | On by default. A Mac addition. Also in the menu. |
| URL and Token | The PC's server. **Paste Connection Info** reads the two lines that the tray's **Copy Mac connection info** copies on the PC. **Test Connection** checks them. |
| Post-process | On This Mac, needs the chosen provider's key. On the PC, sent with each request, and the PC post-processes with its own provider, key, and settings, whatever provider is chosen here. Also in the menu. |
| Provider | Z.ai (GLM), the default, through the GLM Coding Plan, or Anthropic (Claude). The Windows app chooses this in `config.toml`. |
| Z.ai API key, Anthropic API key | Kept in the Keychain, like the PC token. Each shows when its provider is chosen. |
| Model | Anthropic only: Claude Haiku 5.5 (default), Claude Sonnet 5.5, or Other, for any model ID typed below it. |
| Thinking | Anthropic only. Off (default) is the fastest: the model writes the cleaned text without reasoning first. On, it reasons first, for as long as the effort asks. |
| Effort | Anthropic only: Low (default), Medium, High, Extra high, or Max. With thinking off the API takes High or below, so the higher two show only with thinking on. |
| Extra instructions | Appended to the prompt, for names and jargon, as in `config.toml`. |
| App rules | Opens **App Rules…**, also in the menu. On Windows, open **Settings… > App rules** from the tray. |

Claude Sonnet 5.5 turns thinking off with `between_tools`, the other models with `disabled`; a typed model that cannot turn it off, such as Claude Opus 5.5, fails every request with thinking off, and the log says why. Requests to Claude Sonnet 5.5 also turn on the API's server-side fallback, which re-runs a declined request on Claude Sonnet 5. Claude Haiku 5.5 has none, so a declined request pastes the raw transcript, like any other failure.

The other GLM settings of `config.toml`, and Claude's timeout, are defaults you can change from the terminal, then quit and reopen the app:

```sh
defaults write dev.miguelmachado.parakeet-dictate glmModel glm-5.3
defaults write dev.miguelmachado.parakeet-dictate glmReasoningEffort low
defaults write dev.miguelmachado.parakeet-dictate glmTimeout -float 8
defaults write dev.miguelmachado.parakeet-dictate glmEndpoint https://api.z.ai/api/coding/paas/v4/chat/completions
defaults write dev.miguelmachado.parakeet-dictate claudeTimeout -float 8
```

### App rules

A rule gives the post-processing model instructions for dictations into certain apps, such as `Translate the transcript to English.` for Slack. Add one with **+**, from a preset or blank, then pick its apps from the ones running or from Applications. The rules apply when one of their apps is in front as a dictation ends; several rules for the same app add up, in list order.

The model still cleans up the transcript, then follows the rule, even where it asks to translate or restyle. A rule post-processes even with **Post-process** off. On This Mac it goes to the chosen provider and needs its key; on the PC, the PC's selected provider follows it, and the PC's app must support instructions, or it ignores the rule and returns the text in the language spoken. A reply much longer than what was said is rejected as before, and the raw transcript is pasted.

## Check from the terminal

The app's binary has four checks that use its real code and settings, without the menu bar:

```sh
APP="mac/build/Parakeet Dictate.app/Contents/MacOS/ParakeetDictate"
"$APP" --check-local tests/fixtures/pt.wav tests/fixtures/en.wav   # the local model
"$APP" --check-server tests/fixtures/pt.wav                        # the PC in Settings
"$APP" --check-polish "um so the meeting is uh friday"             # the provider and key in Settings
"$APP" --check-media    # pauses what is playing for 1.5 s, then plays it again
```

`PARAKEET_SERVER="<url> <token>"` points `--check-server` at another server. `PARAKEET_INSTRUCTIONS="Translate the transcript to English."` gives `--check-polish` and `--check-server` a rule's instructions. `mac/scripts/fake_pc_server.py` runs the Windows app's server code on the Mac with a stand-in model and GLM, to check the client without the PC; its docstring has the command.

## Develop

```sh
cd mac
swift test    # the reducer, the PC client, GLM, Claude, WAV, tones, resampling, the meter, the overlay scene and styles
swift build   # a debug build; run the app itself from the bundle so macOS applies its permissions
```

With only the Command Line Tools, `swift test` sometimes fails with "plugin for module 'TestingMacros' not found". Passing the plugin's path fixes it: `swift test -Xswiftc -plugin-path -Xswiftc /Library/Developer/CommandLineTools/usr/lib/swift/host/plugins/testing`.

`Sources/DictateCore` is everything that needs no microphone, keyboard, or window; `Sources/ParakeetDictate` is the app. The dictation logic is `Reducer.swift`, a port of `src/parakeet_dictate/reducer.py` and its tests, with one addition: Esc while transcribing cancels, since a request to the PC can take seconds. The package uses Swift 5 language mode, and no SwiftUI macros such as `@State`, whose plugin ships only with Xcode.

The log is `~/Library/Logs/ParakeetDictate.log` (**Open Log** in the menu). Like the Windows log, it records timings and lengths, never the text.

The overlay styles are pixel ports of `src/parakeet_dictate/styles`. To check them against the Python after a change:

```sh
uv run --no-project --python 3.12 --with numpy --with pillow python mac/scripts/overlay_reference.py /tmp/overlay-ref
OVERLAY_REFERENCE_DIR=/tmp/overlay-ref swift test --package-path mac --filter overlayReferenceFramesMatch
```
