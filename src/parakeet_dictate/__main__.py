import sys

if sys.argv[1:] == ["--settings"]:
    # The Settings window needs none of the audio, model and tray modules the app imports.
    from .settings_window import main
else:
    from .app import main

raise SystemExit(main())
