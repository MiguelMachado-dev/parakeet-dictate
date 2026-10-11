# Parakeet Dictate

Local push-to-talk dictation for Windows 11. Hold Right Ctrl, speak English or Portuguese, release, and the text is pasted into the focused app. Speech recognition runs on your machine with NVIDIA Parakeet TDT 0.6B v3 through [onnx-asr](https://github.com/istupakov/onnx-asr), on the GPU when CUDA works and on the CPU otherwise.

## Set up

You need [uv](https://docs.astral.sh/uv/) and, for GPU inference, an NVIDIA driver recent enough for CUDA 13. The CUDA and cuDNN libraries come from pip wheels.

1. Install the dependencies:

	```powershell
	uv sync
	```

2. Download the model (about 3.2 GB, fp32 and int8) into `models\parakeet-tdt-0.6b-v3`. Rerunning the command skips files that are already there.

	```powershell
	uv run python scripts/download_model.py
	```

3. Confirm the model loads on the GPU and transcribes the test clips:

	```powershell
	uv run python scripts/check_model.py --provider cuda --quantization none
	```

## Run

Start the app with a console:

```powershell
uv run parakeet-dictate
```

Start it in the background, with no console window:

```powershell
.venv\Scripts\pythonw.exe -m parakeet_dictate
```

Stop the running instance:

```powershell
uv run parakeet-dictate --quit
```

A tray icon shows the state. A hollow ring means the model is loading, which takes about 7 seconds. Gray means ready, red means recording, amber means transcribing, and violet means GLM is post-processing the transcript. A red ring means startup failed. Hover over the icon to see the state and the active provider (`CUDA` or `CPU int8`). Right-click it for **Post-process with GLM**, **Overlay**, **Open log**, and **Quit**. Only one instance runs at a time. If you start the app while it runs, the running app shows a notification that it is already running, and the new launch exits.

## Dictate

- **Hold to talk.** Hold the hotkey, speak, and release. The text is pasted when you release.
- **Hands-free.** Tap the hotkey (release it within 300 ms). Recording continues until you press the hotkey again.
- **Cancel.** Press Esc while recording. Nothing is pasted.
- **Skip post-processing.** Press Esc while the icon is violet. The raw transcript is pasted right away.
- **Shortcuts still work.** If you press another key while holding the hotkey, such as Right Ctrl+C, the recording is discarded and the shortcut goes through.

The microphone opens when you press the hotkey and closes when the recording stops. A Bluetooth headset stays in its high-quality mode between dictations, and the Windows microphone indicator is lit only while you record.

## Overlay

While you dictate, a small window above the taskbar shows your microphone level live, then whether the app is transcribing or post-processing. It appears on the monitor of the window you are dictating into, 48 px above the bottom of the work area, scaled to that monitor's DPI. When the text is pasted, cancelled, or fails, it shows that for about half a second and fades out.

- **It never takes focus.** Clicks go through it to the window below, and it never becomes the active window, so the paste always lands where you were typing.
- **It waits 250 ms before it appears.** A shortcut such as Right Ctrl+C never flashes it. The start tone plays at once. If you press another key after the overlay appears, it disappears at once.
- **It costs nothing while hidden.** Its thread waits for the next dictation and uses no CPU.

It comes in two styles. `fluid` draws translucent ribbons in a night-blue capsule, and `bars` draws mirrored bars in a smoked-glass capsule. Pick one with `overlay = "fluid"` or `overlay = "bars"` in `config.toml`, or turn it off with `overlay = "off"`. To switch while the app runs, right-click the tray icon and pick **Overlay** > **Fluid**, **Bars**, or **Off**. The change applies at once, even in the middle of a dictation, and the app saves it in `tray_choices.json` ahead of `config.toml`. To preview a style without dictating, run `uv run python scripts/preview_overlay.py --style bars --out previews/bars` and open `previews/bars/video.mp4`.

## Post-process with GLM

The app can send each transcript to GLM-5.3 on [Z.ai](https://z.ai) before it pastes. GLM removes fillers such as um, uh, tipo, and né. It removes false starts and repeated words, keeps only the correction when you correct yourself ("quinta, não, sexta"), and fixes punctuation and capitalization. It keeps your language and your wording. It does not answer questions or follow requests that you dictate.

To turn it on:

1. Get an API key for the Z.ai GLM Coding Plan.
2. Put the key in a file named `.env` in the project root. `.env` is gitignored. A `ZAI_API_KEY` environment variable takes priority over the file.

	```
	ZAI_API_KEY=your-key
	```

3. Send five sample dictations through GLM and check each reply:

	```powershell
	uv run python scripts/check_glm.py
	```

4. Restart the app. Post-processing is on by default once the app finds a key. The `ready` line in `dictate.log` states whether it is on.

With post-processing on:

- **Each dictation takes longer.** A GLM request took 1.4 to 5.5 s, 2.5 s at the median, over 25 measured requests. The icon is violet while GLM works. The app opens its connection to Z.ai when you press the hotkey, so a dictation after a pause does not also wait about 0.9 s for DNS, TCP, and TLS.
- **Esc skips GLM.** Press Esc while the icon is violet, and the raw transcript is pasted immediately. The app keeps that Esc from reaching the focused app, which is where the text goes.
- **A dictation is never lost.** If GLM does not reply within `timeout_s` (8 s by default), returns an error, or cannot be reached, the app pastes the raw transcript. It does the same when the reply is empty or much longer than what you said, which means GLM wrote new text instead of cleaning yours. Each fallback writes a warning to `dictate.log`.
- **Text still waits for the hotkey.** If you are holding the hotkey when the text is ready, the app pastes it when you release the key. This applies to polished text, to the raw fallback, and to Esc.
- **Your transcripts go to Z.ai.** The app sends the transcript text over HTTPS. Audio never leaves your machine. To keep everything local, turn post-processing off. The log records character counts and timings, never the text.

To turn it off or on, right-click the tray icon and click **Post-process with GLM**. The app saves your choice in `tray_choices.json` in the project root and uses it on the next start, ahead of the `enabled` setting in `config.toml`. Without an API key, the item is unavailable and reads **Post-process with GLM (no ZAI_API_KEY)**.

## Dictate from a Mac

A Mac on the same network can dictate through this PC. The Mac app records, sends the audio here, and pastes the text that comes back, transcribed by Parakeet and post-processed by GLM when **Post-process with GLM** is on. Local dictation keeps working while the server runs, and the two never transcribe at the same time: a request that arrives during a local dictation waits for it. The Mac app is a separate project; [docs/mac-client.md](docs/mac-client.md) has the HTTP contract and the brief for building it.

1. Right-click the tray icon and click **Server for Mac**. The server listens on port 8765 on every network adapter. On first start the app creates a random token in `server_token.txt` in the project root. Keep that file private: anyone with the token on your network can use your GLM key. To use a new token, delete the file and turn the server off and on.
2. Run `.\scripts\allow_mac.ps1` once. It asks for administrator rights and creates the Windows Firewall rule **Parakeet Dictate (Mac)**, which allows the port from the local subnet only, on every network profile. Run it again after changing the port. `.\scripts\allow_mac.ps1 -Remove` deletes the rule.
3. Click **Copy Mac connection info** in the tray. The clipboard then holds the URL, such as `http://192.168.3.2:8765`, on the first line and the token on the second. The app picks the address of the physical adapter with the default route, so a VPN tunnel such as NordLynx is skipped.

If the port is in use, the app plays the error tone, writes the reason to `dictate.log`, and leaves **Server for Mac** off. The log has one line per request with the method, path, status, audio length, and the transcription and GLM times, never the text or the token. The transcription time includes any wait for a local dictation to finish.

## Configure

Copy `config.example.toml` to `config.toml` and uncomment the settings to change. Restart the app after editing.

| Setting | Default | Meaning |
| --- | --- | --- |
| `hotkey` | `"ctrl_r"` | A [pynput `Key`](https://pynput.readthedocs.io/en/latest/keyboard.html#pynput.keyboard.Key) name with a side, such as `ctrl_r`, `shift_r`, `f13`, or `pause`. Do not use `alt_r` on layouts where Right Alt is AltGr. |
| `tap_threshold_ms` | `300` | A release sooner than this after the press switches to hands-free. |
| `provider` | `"cuda"` | `"cuda"` runs fp32 on the GPU. `"cpu"` runs int8 on the CPU. |
| `sounds` | `true` | Short tones for start, stop, cancel, and error. |
| `append_space` | `true` | Adds a space after each dictation. |
| `overlay` | `"fluid"` | The live microphone visualizer above the taskbar while you dictate. `"fluid"` or `"bars"` picks the style. With `"off"`, the app creates no window until you pick a style from the tray. |
| `model_dir` | `"models/parakeet-tdt-0.6b-v3"` | Relative to the project root, or absolute. |

Post-processing settings go in a `[postprocess]` table at the end of `config.toml`:

| Setting | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Whether post-processing starts on, until you change it from the tray. |
| `model` | `"glm-5.3"` | The Z.ai model name. |
| `endpoint` | `"https://api.z.ai/api/coding/paas/v4/chat/completions"` | The GLM Coding Plan chat completions URL. |
| `reasoning_effort` | `"low"` | `"low"`, `"high"`, or `"max"`. GLM-5.3 always reasons, and higher effort is slower. At `"max"`, five sample requests took 5.2 to 7.5 s, close to the 8 s `timeout_s`, so raise `timeout_s` too. |
| `timeout_s` | `8.0` | Seconds to wait for GLM before pasting the raw transcript. |
| `extra_instructions` | `""` | Text added to the end of the system prompt. List names and terms that GLM must spell your way, for example `"Names: Miguel, João. Terms: Parakeet, Z.ai."` |

Server settings go in a `[server]` table after `[postprocess]`:

| Setting | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | Whether the server for Mac starts on, until you change it from the tray. |
| `port` | `8765` | The TCP port. |
| `bind` | `"0.0.0.0"` | The IPv4 address to listen on. `"0.0.0.0"` is every adapter. |

If `config.toml` has an unknown setting or an invalid value, the app writes the reason to `dictate.log` and exits.

## Tray choices

The app saves what you pick in the tray menu, **Post-process with GLM**, **Overlay** and **Server for Mac**, in `tray_choices.json` in the project root. A saved choice wins over its setting in `config.toml`, on this start and the next. A setting you never changed from the tray keeps following `config.toml`. Delete `tray_choices.json` to go back to the config settings for all of them. If the file holds a value the app does not recognize, the app logs a warning and uses the config setting for that key.

## Install

Double-click `Install.cmd` in the project folder, or run:

```powershell
.\scripts\install.ps1
```

The app keeps running from the project folder and its `.venv`. The installer does the following:

- It runs `uv sync` if `.venv` is missing, and downloads the model, about 3 GB, if `models\parakeet-tdt-0.6b-v3` is missing.
- It puts a **Parakeet Dictate** shortcut on the Desktop and in the Start menu. When you start the app from either one, it shows a notification once the model is loaded.
- It adds Parakeet Dictate to **Settings** > **Apps** > **Installed apps**.

Running the installer again gives the same result. The icon is `assets\parakeet.ico`, drawn by `scripts/make_icon.py`.

To also start the app at sign-in, with no notification, add `-Startup`. A later run without `-Startup` keeps the sign-in shortcut.

```powershell
.\scripts\install.ps1 -Startup
```

To uninstall, click **Uninstall** next to Parakeet Dictate in **Installed apps**, or run `.\scripts\install.ps1 -Uninstall`. It stops the app, removes the shortcuts that start this folder's app, and removes the Installed apps entry. It leaves the project folder, the model, and `tray_choices.json` in place. Delete the folder to remove them too.

## Troubleshoot

The app writes to `dictate.log` in the project root, rotated at 1 MB. The log records state changes, timings, the provider, and errors. It does not record what you said.

These scripts check each part on its own:

| Command | Checks |
| --- | --- |
| `uv run pytest` | The dictation state machine, config parsing, the saved tray choices, the tray's Overlay menu, the already-running and ready notifications, the GLM client against a fake server, the overlay's timing and placement, and one pixel drawn through the overlay window and read back from the desktop. |
| `uv run python scripts/mic_check.py` | Records 2 s from the default microphone and prints the device, rate, frame count, level, the overlay meter's readings, and what the audio callback costs. |
| `uv run python scripts/overlay_check.py` | Shows the overlay through 10 scripted dictations and a 300-frame recording. Prints GDI and USER handle counts, frame rate, render time, and the overlay thread's CPU while hidden. Add `--style bars` to measure the other style. It then switches one recording fluid, bars, off, fluid as the tray does, and checks that only the chosen style draws, that off hides the window, and that handle counts stay flat. Add `--screenshots <folder>` to save a desktop grab per style. It sends no keys. |
| `uv run python scripts/bench_hook_latency.py` | Times key events from send to the keyboard hook with the overlay hidden and animating. Add `--style bars` to try another overlay style. It sends F24 to a small window; stop the app first and do not use the keyboard or mouse while it runs. |
| `uv run python scripts/check_model.py --provider cuda --quantization none` | Model load, provider, accuracy, and latency on the test clips. |
| `uv run python scripts/check_glm.py` | Sends five sample dictations to GLM through the app's client and checks each reply. Add `--text "..."` to try your own. |
| `uv run python scripts/e2e_check.py` | The full pipeline with fixture audio in place of the microphone, with and without GLM, including the fallbacks, Esc, and shortcuts. It opens a small window, sends the hotkey, and checks what gets pasted. It also checks that the overlay never takes focus or clicks, shows on the right monitor, and never appears for a shortcut. Add `--screenshots <folder>` to save desktop grabs of the overlay while it records. It needs `ZAI_API_KEY`. Stop the app first. Do not use the keyboard or mouse while it runs. |
| `uv run python scripts/bench_latency.py` | Transcription latency on audio lengths the model has not seen since warm-up. |
| `uv run python scripts/bench_preconnect.py` | Key-up to paste for polished dictations after 35 s idle, with and without pre-connecting to Z.ai. It takes about 7 minutes and installs no keyboard hook. |

## Known limits

- **Only clipboard text is restored.** The app saves the clipboard text, pastes, and puts the text back after 200 ms. An image, a file list, or rich formatting on the clipboard is lost, and the clipboard is left empty. An app that reads the clipboard more slowly than 200 ms can paste your old clipboard text instead.
- **Admin windows do not work.** Windows blocks the keyboard hook and the paste for apps that run as administrator, unless this app runs as administrator too.
- **Esc reaches the app in hands-free mode.** While you hold the hotkey, the app swallows Esc so that Ctrl+Esc does not open Start. While GLM works, it swallows Esc because that Esc pastes the raw transcript. In hands-free recording the focused app also receives the Esc.
- **CUDA fallback happens only at startup.** If CUDA fails while the app runs, for example after a driver reset, each dictation plays the error tone until you restart the app.
- **The first moments after the press are not recorded.** Opening the microphone takes about 60 ms.
- **Hands-free has no time limit.** A forgotten hands-free recording keeps the microphone open until the next press. Recordings of several minutes are untested.
- **A failed paste shows as done on the overlay.** If the clipboard stays locked and the paste fails, the error tone plays, but the overlay has already started its done beat.
- **Device names are truncated.** The Windows MME audio API cuts names to 31 characters, so `dictate.log` can show `Voice Chat Mic (BEACN Mix Creat`.
