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

A tray icon shows the state. A hollow ring means the model is loading, which takes about 7 seconds. Gray means ready, red means recording, amber means transcribing, and violet means GLM is post-processing the transcript. A red ring means startup failed. Hover over the icon to see the state and the active provider (`CUDA` or `CPU int8`). Right-click it for **Post-process with GLM**, **Open log**, and **Quit**. Only one instance runs at a time. A second launch logs a line and exits.

## Dictate

- **Hold to talk.** Hold the hotkey, speak, and release. The text is pasted when you release.
- **Hands-free.** Tap the hotkey (release it within 300 ms). Recording continues until you press the hotkey again.
- **Cancel.** Press Esc while recording. Nothing is pasted.
- **Skip post-processing.** Press Esc while the icon is violet. The raw transcript is pasted right away.
- **Shortcuts still work.** If you press another key while holding the hotkey, such as Right Ctrl+C, the recording is discarded and the shortcut goes through.

The microphone opens when you press the hotkey and closes when the recording stops. A Bluetooth headset stays in its high-quality mode between dictations, and the Windows microphone indicator is lit only while you record.

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

To turn it off or on, right-click the tray icon and click **Post-process with GLM**. The app saves your choice in `postprocess.json` in the project root and uses it on the next start, ahead of the `enabled` setting in `config.toml`. Delete `postprocess.json` to go back to the config setting. Without an API key, the item is unavailable and reads **Post-process with GLM (no ZAI_API_KEY)**.

## Configure

Copy `config.example.toml` to `config.toml` and uncomment the settings to change. Restart the app after editing.

| Setting | Default | Meaning |
| --- | --- | --- |
| `hotkey` | `"ctrl_r"` | A [pynput `Key`](https://pynput.readthedocs.io/en/latest/keyboard.html#pynput.keyboard.Key) name with a side, such as `ctrl_r`, `shift_r`, `f13`, or `pause`. Do not use `alt_r` on layouts where Right Alt is AltGr. |
| `tap_threshold_ms` | `300` | A release sooner than this after the press switches to hands-free. |
| `provider` | `"cuda"` | `"cuda"` runs fp32 on the GPU. `"cpu"` runs int8 on the CPU. |
| `sounds` | `true` | Short tones for start, stop, cancel, and error. |
| `append_space` | `true` | Adds a space after each dictation. |
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

If `config.toml` has an unknown setting or an invalid value, the app writes the reason to `dictate.log` and exits.

## Start at sign-in

Create a shortcut in your Startup folder that runs the app with `pythonw`:

```powershell
.\scripts\install_startup.ps1
```

Remove the shortcut:

```powershell
.\scripts\install_startup.ps1 -Remove
```

## Troubleshoot

The app writes to `dictate.log` in the project root, rotated at 1 MB. The log records state changes, timings, the provider, and errors. It does not record what you said.

These scripts check each part on its own:

| Command | Checks |
| --- | --- |
| `uv run pytest` | The dictation state machine, config parsing, and the GLM client against a fake server. |
| `uv run python scripts/mic_check.py` | Records 2 s from the default microphone and prints the device, rate, frame count, and level. |
| `uv run python scripts/check_model.py --provider cuda --quantization none` | Model load, provider, accuracy, and latency on the test clips. |
| `uv run python scripts/check_glm.py` | Sends five sample dictations to GLM through the app's client and checks each reply. Add `--text "..."` to try your own. |
| `uv run python scripts/e2e_check.py` | The full pipeline with fixture audio in place of the microphone, with and without GLM, including the fallbacks and Esc. It opens a small window, sends the hotkey, and checks what gets pasted. It needs `ZAI_API_KEY`. Stop the app first. Do not use the keyboard or mouse while it runs. |
| `uv run python scripts/bench_latency.py` | Transcription latency on audio lengths the model has not seen since warm-up. |
| `uv run python scripts/bench_preconnect.py` | Key-up to paste for polished dictations after 35 s idle, with and without pre-connecting to Z.ai. It takes about 7 minutes and installs no keyboard hook. |

## Known limits

- **Only clipboard text is restored.** The app saves the clipboard text, pastes, and puts the text back after 200 ms. An image, a file list, or rich formatting on the clipboard is lost, and the clipboard is left empty. An app that reads the clipboard more slowly than 200 ms can paste your old clipboard text instead.
- **Admin windows do not work.** Windows blocks the keyboard hook and the paste for apps that run as administrator, unless this app runs as administrator too.
- **Esc reaches the app in hands-free mode.** While you hold the hotkey, the app swallows Esc so that Ctrl+Esc does not open Start. While GLM works, it swallows Esc because that Esc pastes the raw transcript. In hands-free recording the focused app also receives the Esc.
- **CUDA fallback happens only at startup.** If CUDA fails while the app runs, for example after a driver reset, each dictation plays the error tone until you restart the app.
- **The first moments after the press are not recorded.** Opening the microphone takes about 60 ms.
- **Hands-free has no time limit.** A forgotten hands-free recording keeps the microphone open until the next press. Recordings of several minutes are untested.
- **Device names are truncated.** The Windows MME audio API cuts names to 31 characters, so `dictate.log` can show `Voice Chat Mic (BEACN Mix Creat`.
