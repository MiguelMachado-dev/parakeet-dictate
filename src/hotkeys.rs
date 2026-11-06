//! Key names for `hotkey` in config.toml and the virtual-key codes the keyboard hook reports.
//! The names are pynput's, which configurations written for the Python app already use.

const NAMED: &[(&str, u32)] = &[
    ("alt", 0x12),
    ("alt_l", 0xA4),
    ("alt_r", 0xA5),
    ("alt_gr", 0xA5),
    ("backspace", 0x08),
    ("caps_lock", 0x14),
    ("cmd", 0x5B),
    ("cmd_l", 0x5B),
    ("cmd_r", 0x5C),
    ("ctrl", 0x11),
    ("ctrl_l", 0xA2),
    ("ctrl_r", 0xA3),
    ("delete", 0x2E),
    ("down", 0x28),
    ("end", 0x23),
    ("enter", 0x0D),
    ("esc", 0x1B),
    ("home", 0x24),
    ("left", 0x25),
    ("page_down", 0x22),
    ("page_up", 0x21),
    ("right", 0x27),
    ("shift", 0xA0),
    ("shift_l", 0xA0),
    ("shift_r", 0xA1),
    ("space", 0x20),
    ("tab", 0x09),
    ("up", 0x26),
    ("media_play_pause", 0xB3),
    ("media_stop", 0xB2),
    ("media_volume_mute", 0xAD),
    ("media_volume_down", 0xAE),
    ("media_volume_up", 0xAF),
    ("media_previous", 0xB1),
    ("media_next", 0xB0),
    ("insert", 0x2D),
    ("menu", 0x5D),
    ("num_lock", 0x90),
    ("pause", 0x13),
    ("print_screen", 0x2C),
    ("scroll_lock", 0x91),
];

const LABELS: &[(&str, &str)] = &[
    ("ctrl_l", "Left Ctrl"),
    ("ctrl_r", "Right Ctrl"),
    ("shift_l", "Left Shift"),
    ("shift_r", "Right Shift"),
    ("alt_l", "Left Alt"),
    ("alt_r", "Right Alt"),
    ("alt_gr", "AltGr"),
    ("cmd_l", "Left Windows"),
    ("cmd_r", "Right Windows"),
];

pub fn vk_of(name: &str) -> Option<u32> {
    if let Some(number) = name.strip_prefix('f').and_then(|digits| digits.parse::<u32>().ok()) {
        let canonical = format!("f{number}");
        return (canonical == name && (1..=24).contains(&number)).then_some(0x6F + number);
    }
    NAMED.iter().find(|(key, _)| *key == name).map(|(_, vk)| *vk)
}

/// The hotkey as people name it: "Right Ctrl", "F13", "Scroll Lock".
pub fn label_of(name: &str) -> String {
    if let Some((_, label)) = LABELS.iter().find(|(key, _)| *key == name) {
        return (*label).to_owned();
    }
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}
