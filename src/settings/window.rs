//! The Settings window in native Windows controls. What each control means lives in `form`; this file
//! places the controls and copies values between them and the form.

use std::cell::{Cell, RefCell};
use std::path::Path;

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WAIT_OBJECT_0, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_WINDOW, CLEARTYPE_QUALITY, CreateFontW, DEFAULT_CHARSET, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, DrawTextW, FW_BOLD,
    FW_NORMAL, GetDC, GetSysColor, GetSysColorBrush, HDC, HFONT, InvalidateRect, MapWindowPoints, ReleaseDC, SelectObject, SetBkColor,
    SetTextColor, UpdateWindow,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OPENFILENAMEW};
use windows_sys::Win32::UI::Controls::{
    EM_SETPASSWORDCHAR, EM_SETREADONLY, ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR, SetScrollInfo, SetScrollPos,
    TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT, TCM_GETCURSEL, TCM_INSERTITEMW, TCN_SELCHANGE, WC_TABCONTROLW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE};
use windows_sys::Win32::UI::Shell::{BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BN_SETFOCUS, BS_AUTOCHECKBOX, BS_NOTIFY, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL,
    CB_GETDROPPEDSTATE, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT, CB_SETCURSEL, CBN_DROPDOWN, CBN_EDITCHANGE, CBN_SELCHANGE, CBN_SETFOCUS,
    CBS_AUTOHSCROLL, CBS_DROPDOWN, CBS_DROPDOWNLIST, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, EN_CHANGE, EN_SETFOCUS, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_PASSWORD, ES_WANTRETURN, GetClientRect,
    GetScrollInfo, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HICON, HMENU, ICON_BIG, ICON_SMALL, IDYES, IMAGE_ICON,
    IsDialogMessageW, KillTimer, LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE, LBN_SETFOCUS, LBS_NOINTEGRALHEIGHT,
    LBS_NOTIFY, LR_DEFAULTSIZE, LoadImageW, MB_ICONQUESTION, MB_YESNO, MF_STRING, MINMAXINFO, MSG, MessageBoxW, MsgWaitForMultipleObjects,
    PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT, RegisterClassExW, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
    SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLINFO, SIF_ALL, SIF_DISABLENOSCROLL, SIF_PAGE, SIF_POS, SIF_RANGE, SM_CXSCREEN, SM_CYSCREEN, SW_ERASE,
    SW_HIDE, SW_INVALIDATE, SW_RESTORE, SW_SCROLLCHILDREN, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, ScrollWindowEx, SendMessageW, SetForegroundWindow,
    SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, TrackPopupMenu, TranslateMessage,
    WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO, WM_KEYDOWN, WM_MOUSEWHEEL, WM_NOTIFY, WM_QUIT, WM_SETFONT, WM_SETICON,
    WM_SIZE, WM_TIMER, WM_VSCROLL, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::form::{ENGINES, Editor, Fields, OVERLAYS, PRESETS, PROVIDERS, Tone, rule_title};
use crate::foreground::running_executables;
use crate::instance::Handle;
use crate::settings_store::SettingsStore;
use crate::win::{from_wide, wide};

const MAIN_CLASS: &str = "ParakeetDictateSettings";
const PAGE_CLASS: &str = "ParakeetDictateSettingsPage";
const TITLE: &str = "Parakeet Dictate — Settings";
const TEST_TIMER: usize = 1;
const SS_NOPREFIX: u32 = 0x80;
const SS_ETCHEDHORZ: u32 = 0x10;
const BULLET: usize = 0x2022;
const ROW_IDS: u16 = 1000;
const IDS_PER_ROW: u16 = 4;
const HINT_COLOR: COLORREF = rgb(0x50, 0x50, 0x50);
const SUCCESS_COLOR: COLORREF = rgb(0x27, 0x63, 0x34);
const FAILURE_COLOR: COLORREF = rgb(0xa1, 0x20, 0x20);

const fn rgb(red: u32, green: u32, blue: u32) -> COLORREF {
    red | (green << 8) | (blue << 16)
}

mod id {
    pub const CHOOSE_FOLDER: u16 = 10;
    pub const POLISH_PROVIDER: u16 = 11;
    pub const KEY: u16 = 12;
    pub const SHOW_KEY: u16 = 13;
    pub const TEST: u16 = 14;
    pub const EXAMPLE: u16 = 15;
    pub const RULE_LIST: u16 = 16;
    pub const ADD_RULE: u16 = 17;
    pub const REMOVE_RULE: u16 = 18;
    pub const MOVE_UP: u16 = 19;
    pub const MOVE_DOWN: u16 = 20;
    pub const RULE_NAME: u16 = 21;
    pub const RULE_ENABLED: u16 = 22;
    pub const INSTRUCTIONS: u16 = 23;
    pub const APPS: u16 = 24;
    pub const REMOVE_APP: u16 = 25;
    pub const APP_NAME: u16 = 26;
    pub const ADD_APP: u16 = 27;
    pub const BROWSE: u16 = 28;
    pub const RELOAD: u16 = 29;
    pub const CLOSE: u16 = 30;
    pub const SAVE: u16 = 31;
    pub const THINKING: u16 = 32;
    /// The simple text and check controls, each bound to one field.
    pub const BOUND: u16 = 100;
}

type TextField = fn(&mut Fields) -> &mut String;
type FlagField = fn(&mut Fields) -> &mut bool;

fn set_text(control: HWND, text: &str) {
    unsafe { SetWindowTextW(control, wide(text).as_ptr()) };
}

fn text_of(control: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(control) };
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe { GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32) };
    from_wide(&buffer)
}

fn to_control(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn from_control(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// A combo's value: the chosen entry when one is, because the edit part updates only after the notification.
fn combo_value(control: HWND) -> String {
    let index = unsafe { SendMessageW(control, CB_GETCURSEL, 0, 0) };
    if index < 0 {
        return text_of(control);
    }
    let length = unsafe { SendMessageW(control, CB_GETLBTEXTLEN, index as usize, 0) };
    let mut buffer = vec![0u16; length.max(0) as usize + 1];
    unsafe { SendMessageW(control, CB_GETLBTEXT, index as usize, buffer.as_mut_ptr() as LPARAM) };
    from_wide(&buffer)
}

fn set_combo_values(control: HWND, values: &[String]) {
    unsafe { SendMessageW(control, CB_RESETCONTENT, 0, 0) };
    for value in values {
        unsafe { SendMessageW(control, CB_ADDSTRING, 0, wide(value).as_ptr() as LPARAM) };
    }
}

fn set_combo(control: HWND, value: &str) {
    let values = (0..200).take_while(|index| {
        let length = unsafe { SendMessageW(control, CB_GETLBTEXTLEN, *index, 0) };
        let mut buffer = vec![0u16; length.max(0) as usize + 1];
        if length < 0 {
            return false;
        }
        unsafe { SendMessageW(control, CB_GETLBTEXT, *index, buffer.as_mut_ptr() as LPARAM) };
        from_wide(&buffer) != value
    });
    let position = values.count();
    let count = unsafe { SendMessageW(control, windows_sys::Win32::UI::WindowsAndMessaging::CB_GETCOUNT, 0, 0) };
    if (position as isize) < count {
        unsafe { SendMessageW(control, CB_SETCURSEL, position, 0) };
    } else {
        unsafe { SendMessageW(control, CB_SETCURSEL, usize::MAX, 0) };
        set_text(control, value);
    }
}

fn is_checked(control: HWND) -> bool {
    unsafe { SendMessageW(control, BM_GETCHECK, 0, 0) == 1 }
}

fn set_checked(control: HWND, checked: bool) {
    unsafe { SendMessageW(control, BM_SETCHECK, usize::from(checked), 0) };
}

fn enable(control: HWND, enabled: bool) {
    unsafe { EnableWindow(control, enabled.into()) };
}

fn show(control: HWND, shown: bool) {
    unsafe { ShowWindow(control, if shown { SW_SHOW } else { SW_HIDE }) };
}

struct Fonts {
    body: HFONT,
    title: HFONT,
    heading: HFONT,
    hint: HFONT,
}

impl Fonts {
    fn new(dpi: i32) -> Self {
        let font = |points: i32, weight: u32| unsafe {
            CreateFontW(
                -(points * dpi / 72),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                u32::from(DEFAULT_CHARSET),
                0,
                0,
                u32::from(CLEARTYPE_QUALITY),
                0,
                wide("Segoe UI").as_ptr(),
            )
        };
        Self { body: font(10, FW_NORMAL), title: font(18, FW_BOLD), heading: font(11, FW_BOLD), hint: font(9, FW_NORMAL) }
    }
}

struct Row {
    source: HWND,
    target: HWND,
    remove: HWND,
    add: HWND,
}

/// Places controls down a page, advancing `y`. Coordinates come in 96-DPI units and scale here.
struct Flow<'a> {
    ui: &'a Ui,
    x: i32,
    width: i32,
    y: i32,
}

impl Flow<'_> {
    fn gap(&mut self, units: i32) {
        self.y += self.ui.px(units);
    }

    fn label_width(&self) -> i32 {
        self.ui.px(190)
    }

    /// A label on the left and `control` filling the rest.
    fn field(&mut self, label: HWND, control: HWND, height: i32) {
        let (left, height) = (self.label_width(), self.ui.px(height));
        self.ui.place(label, self.x, self.y + self.ui.px(4), left - self.ui.px(8), self.ui.px(22));
        self.ui.place(control, self.x + left, self.y, self.width - left, height);
        self.y += height + self.ui.px(10);
    }

    /// Cells in a row starting at `indent`: fixed widths, the rest shared by the cells without one.
    fn row(&mut self, indent: i32, cells: &[(HWND, Option<i32>)], height: i32) {
        let (height, gap) = (self.ui.px(height), self.ui.px(8));
        let fixed: i32 = cells.iter().filter_map(|(_, width)| width.map(|width| self.ui.px(width))).sum();
        let flexible = cells.iter().filter(|(_, width)| width.is_none()).count() as i32;
        let spare = self.width - indent - fixed - gap * (cells.len() as i32 - 1);
        let mut x = self.x + indent;
        for (control, width) in cells {
            let width = width.map_or(spare / flexible.max(1), |width| self.ui.px(width));
            if !control.is_null() {
                self.ui.place(*control, x, self.y, width, height);
            }
            x += width + gap;
        }
        self.y += height + self.ui.px(10);
    }

    fn line(&mut self, control: HWND, height: i32) {
        self.row(0, &[(control, None)], height);
    }

    /// Wrapped text as tall as it needs, starting at `indent`.
    fn text(&mut self, control: HWND, indent: i32, after: i32) {
        let width = self.width - indent;
        let height = self.ui.text_height(control, width);
        self.ui.place(control, self.x + indent, self.y, width, height);
        self.y += height + self.ui.px(after);
    }
}

pub struct Ui {
    editor: RefCell<Editor>,
    main: HWND,
    scale: f32,
    fonts: Fonts,
    loading: Cell<bool>,
    hints: RefCell<Vec<HWND>>,
    combos: RefCell<Vec<HWND>>,
    bound_text: RefCell<Vec<(HWND, TextField, bool)>>,
    bound_flags: RefCell<Vec<(HWND, FlagField)>>,
    tabs: Cell<HWND>,
    pages: RefCell<Vec<HWND>>,
    scrolled: RefCell<[i32; 5]>,
    title: Cell<HWND>,
    subtitle: Cell<HWND>,
    status: Cell<HWND>,
    reload: Cell<HWND>,
    close: Cell<HWND>,
    save: Cell<HWND>,
    general: RefCell<Vec<HWND>>,
    post: RefCell<Vec<HWND>>,
    dictionary: RefCell<Vec<HWND>>,
    rules: RefCell<Vec<HWND>>,
    server: RefCell<Vec<HWND>>,
    rows: RefCell<Vec<Row>>,
}

thread_local! {
    static UI: Cell<Option<&'static Ui>> = const { Cell::new(None) };
}

fn ui() -> Option<&'static Ui> {
    UI.with(Cell::get)
}

// Indices into each page's control list, in the order they are created.
mod general {
    pub const HOTKEY_LABEL: usize = 0;
    pub const HOTKEY: usize = 1;
    pub const HOTKEY_HINT: usize = 2;
    pub const ENGINE_LABEL: usize = 3;
    pub const ENGINE: usize = 4;
    pub const OVERLAY_LABEL: usize = 5;
    pub const OVERLAY: usize = 6;
    pub const SOUNDS: usize = 7;
    pub const SPACE: usize = 8;
    pub const LINE: usize = 9;
    pub const ADVANCED: usize = 10;
    pub const TAP_LABEL: usize = 11;
    pub const TAP: usize = 12;
    pub const FOLDER_LABEL: usize = 13;
    pub const FOLDER: usize = 14;
    pub const CHOOSE: usize = 15;
}

mod post {
    pub const ENABLED: usize = 0;
    pub const ENABLED_HINT: usize = 1;
    pub const PROVIDER_LABEL: usize = 2;
    pub const PROVIDER: usize = 3;
    pub const KEY_LABEL: usize = 4;
    pub const KEY: usize = 5;
    pub const SHOW_KEY: usize = 6;
    pub const KEY_HINT: usize = 7;
    pub const MODEL_LABEL: usize = 8;
    pub const MODEL: usize = 9;
    pub const THINKING: usize = 10;
    pub const EFFORT_LABEL: usize = 11;
    pub const EFFORT: usize = 12;
    pub const TIMEOUT_LABEL: usize = 13;
    pub const TIMEOUT: usize = 14;
    pub const EXTRA_LABEL: usize = 15;
    pub const EXTRA: usize = 16;
    pub const EXTRA_HINT: usize = 17;
    pub const ENDPOINT_LABEL: usize = 18;
    pub const ENDPOINT: usize = 19;
    pub const TEST: usize = 20;
    pub const TEST_STATUS: usize = 21;
}

mod dictionary {
    pub const HINT: usize = 0;
    pub const WORDS_HEADING: usize = 1;
    pub const WORDS: usize = 2;
    pub const WORDS_HINT: usize = 3;
    pub const ROWS_HEADING: usize = 4;
    pub const ORIGINAL: usize = 5;
    pub const REPLACE_WITH: usize = 6;
    pub const ROWS_HINT: usize = 7;
    pub const EXAMPLE: usize = 8;
    pub const TRY_LABEL: usize = 9;
    pub const SAMPLE: usize = 10;
    pub const PREVIEW: usize = 11;
}

mod rules {
    pub const HINT: usize = 0;
    pub const LIST: usize = 1;
    pub const ADD: usize = 2;
    pub const REMOVE: usize = 3;
    pub const UP: usize = 4;
    pub const DOWN: usize = 5;
    pub const NAME_LABEL: usize = 6;
    pub const NAME: usize = 7;
    pub const ENABLED: usize = 8;
    pub const INSTRUCTIONS_LABEL: usize = 9;
    pub const INSTRUCTIONS: usize = 10;
    pub const APPS_LABEL: usize = 11;
    pub const APPS: usize = 12;
    pub const REMOVE_APP: usize = 13;
    pub const ADD_LABEL: usize = 14;
    pub const APP_NAME: usize = 15;
    pub const ADD_APP: usize = 16;
    pub const BROWSE: usize = 17;
    pub const NOTE: usize = 18;
}

mod server {
    pub const ENABLED: usize = 0;
    pub const HINT: usize = 1;
    pub const PORT_LABEL: usize = 2;
    pub const PORT: usize = 3;
    pub const BIND_LABEL: usize = 4;
    pub const BIND: usize = 5;
    pub const NOTE: usize = 6;
}

impl Ui {
    fn px(&self, units: i32) -> i32 {
        (units as f32 * self.scale).round() as i32
    }

    fn place(&self, control: HWND, x: i32, y: i32, width: i32, height: i32) {
        // A combo's height includes its open list; the closed part sizes itself to the font.
        let height = if self.combos.borrow().contains(&control) { self.px(260) } else { height };
        unsafe { SetWindowPos(control, std::ptr::null_mut(), x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    fn create(&self, class: &str, text: &str, style: u32, extra: u32, parent: HWND, control: u16, font: HFONT) -> HWND {
        let (class, text) = (wide(class), wide(text));
        unsafe {
            let hwnd = CreateWindowExW(
                extra,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                parent,
                control as usize as HMENU,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            SendMessageW(hwnd, WM_SETFONT, font as usize, 0);
            hwnd
        }
    }

    fn label(&self, parent: HWND, text: &str) -> HWND {
        self.create("STATIC", text, SS_NOPREFIX, 0, parent, 0, self.fonts.body)
    }

    fn hint(&self, parent: HWND, text: &str) -> HWND {
        let hint = self.create("STATIC", text, SS_NOPREFIX, 0, parent, 0, self.fonts.hint);
        self.hints.borrow_mut().push(hint);
        hint
    }

    fn heading(&self, parent: HWND, text: &str) -> HWND {
        self.create("STATIC", text, SS_NOPREFIX, 0, parent, 0, self.fonts.heading)
    }

    fn button(&self, parent: HWND, control: u16, text: &str) -> HWND {
        self.create("BUTTON", text, WS_TABSTOP | (BS_PUSHBUTTON | BS_NOTIFY) as u32, 0, parent, control, self.fonts.body)
    }

    fn check(&self, parent: HWND, control: u16, text: &str) -> HWND {
        self.create("BUTTON", text, WS_TABSTOP | (BS_AUTOCHECKBOX | BS_NOTIFY) as u32, 0, parent, control, self.fonts.body)
    }

    fn edit(&self, parent: HWND, control: u16, style: i32) -> HWND {
        let style = WS_TABSTOP | (ES_AUTOHSCROLL | style) as u32;
        self.create("EDIT", "", style, WS_EX_CLIENTEDGE, parent, control, self.fonts.body)
    }

    fn multiline(&self, parent: HWND, control: u16) -> HWND {
        let style = WS_TABSTOP | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
        self.create("EDIT", "", style, WS_EX_CLIENTEDGE, parent, control, self.fonts.body)
    }

    fn combo(&self, parent: HWND, control: u16, editable: bool, values: &[String]) -> HWND {
        let kind = if editable { CBS_DROPDOWN | CBS_AUTOHSCROLL } else { CBS_DROPDOWNLIST };
        let combo = self.create("COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | kind as u32, 0, parent, control, self.fonts.body);
        set_combo_values(combo, values);
        self.combos.borrow_mut().push(combo);
        combo
    }

    fn list(&self, parent: HWND, control: u16) -> HWND {
        let style = WS_TABSTOP | WS_VSCROLL | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32;
        self.create("LISTBOX", "", style, WS_EX_CLIENTEDGE, parent, control, self.fonts.body)
    }

    fn bound_edit(&self, parent: HWND, field: TextField) -> HWND {
        let next = self.next_bound();
        let edit = self.edit(parent, next, 0);
        self.bound_text.borrow_mut().push((edit, field, false));
        edit
    }

    fn bound_multiline(&self, parent: HWND, field: TextField) -> HWND {
        let next = self.next_bound();
        let edit = self.multiline(parent, next);
        self.bound_text.borrow_mut().push((edit, field, true));
        edit
    }

    fn bound_combo(&self, parent: HWND, field: TextField, editable: bool, values: &[String]) -> HWND {
        let next = self.next_bound();
        let combo = self.combo(parent, next, editable, values);
        self.bound_text.borrow_mut().push((combo, field, false));
        combo
    }

    fn bound_check(&self, parent: HWND, field: FlagField, text: &str) -> HWND {
        let next = self.next_bound();
        let check = self.check(parent, next, text);
        self.bound_flags.borrow_mut().push((check, field));
        check
    }

    fn next_bound(&self) -> u16 {
        id::BOUND + (self.bound_text.borrow().len() + self.bound_flags.borrow().len()) as u16
    }

    fn build_general(&self, page: HWND) {
        let hotkeys = self.editor.borrow().hotkey_labels();
        let engines: Vec<String> = ENGINES.iter().map(|(label, _)| (*label).to_owned()).collect();
        let overlays: Vec<String> = OVERLAYS.iter().map(|label| (*label).to_owned()).collect();
        let hotkey_label = self.label(page, "Dictation hotkey");
        let hotkey = self.bound_combo(page, |f| &mut f.hotkey, false, &hotkeys);
        let hotkey_hint = self.hint(page, "Hold to talk, or tap for hands-free recording. Esc cancels. On some keyboard layouts, Right Alt is AltGr.");
        let engine_label = self.label(page, "Speech recognition");
        let engine = self.bound_combo(page, |f| &mut f.engine, false, &engines);
        let overlay_label = self.label(page, "Overlay");
        let overlay = self.bound_combo(page, |f| &mut f.overlay, false, &overlays);
        let sounds = self.bound_check(page, |f| &mut f.sounds, "Play start, stop, cancel, and error sounds");
        let space = self.bound_check(page, |f| &mut f.append_space, "Add a space after each dictation");
        let line = self.create("STATIC", "", SS_ETCHEDHORZ, 0, page, 0, self.fonts.body);
        let advanced = self.heading(page, "Advanced");
        let tap_label = self.label(page, "Tap threshold (ms)");
        let tap = self.bound_edit(page, |f| &mut f.tap_threshold_ms);
        let folder_label = self.label(page, "Model folder");
        let folder = self.bound_edit(page, |f| &mut f.model_dir);
        let choose = self.button(page, id::CHOOSE_FOLDER, "Choose folder…");
        *self.general.borrow_mut() = vec![
            hotkey_label, hotkey, hotkey_hint, engine_label, engine, overlay_label, overlay, sounds, space, line, advanced, tap_label, tap,
            folder_label, folder, choose,
        ];
    }

    fn build_post(&self, page: HWND) {
        let providers: Vec<String> = PROVIDERS.iter().map(|(label, _)| (*label).to_owned()).collect();
        let enabled = self.bound_check(page, |f| &mut f.postprocess_enabled, "Post-process dictations");
        let enabled_hint = self.hint(
            page,
            "Clean up fillers, repetitions, and punctuation before pasting. Transcript text goes to the chosen provider; audio stays on this PC.",
        );
        let provider_label = self.label(page, "Provider");
        let provider = self.combo(page, id::POLISH_PROVIDER, false, &providers);
        let key_label = self.label(page, "Z.ai API key");
        let key = self.edit(page, id::KEY, ES_PASSWORD);
        unsafe { SendMessageW(key, EM_SETPASSWORDCHAR, BULLET, 0) };
        let show_key = self.check(page, id::SHOW_KEY, "Show");
        let key_hint = self.hint(page, "");
        let model_label = self.label(page, "Model");
        let model = self.bound_combo(page, |f| &mut f.polish_model, true, &[]);
        let thinking = self.check(page, id::THINKING, "Thinking before replying (Claude)");
        let effort_label = self.label(page, "Effort");
        let effort = self.bound_combo(page, |f| &mut f.effort, false, &[]);
        let timeout_label = self.label(page, "Timeout (seconds)");
        let timeout = self.bound_edit(page, |f| &mut f.timeout_s);
        let extra_label = self.label(page, "Extra instructions");
        let extra = self.bound_multiline(page, |f| &mut f.extra_instructions);
        let extra_hint = self.hint(page, "Names and terms to spell your way.");
        let endpoint_label = self.label(page, "API endpoint");
        let endpoint = self.bound_edit(page, |f| &mut f.endpoint);
        let test = self.button(page, id::TEST, "Test provider");
        let test_status = self.hint(page, "");
        *self.post.borrow_mut() = vec![
            enabled, enabled_hint, provider_label, provider, key_label, key, show_key, key_hint, model_label, model, thinking, effort_label, effort,
            timeout_label, timeout, extra_label, extra, extra_hint, endpoint_label, endpoint, test, test_status,
        ];
    }

    fn build_dictionary(&self, page: HWND) {
        let hint = self.hint(
            page,
            "Preferred spellings guide AI cleanup and preserve spelling before pasting. Replacements run locally even when cleanup is off, skipped, or fails.",
        );
        let words_heading = self.heading(page, "Preferred spellings");
        let words = self.bound_multiline(page, |f| &mut f.words);
        let words_hint = self.hint(page, "One word or phrase per line, such as PI or harness.");
        let rows_heading = self.heading(page, "Replacements");
        let original = self.label(page, "Original");
        let replace_with = self.label(page, "Replace with");
        let rows_hint = self.hint(
            page,
            "Enter the original and its replacement. Use + to add a row and − to remove it. Matches cover whole words or phrases, ignoring case; inserted text is not replaced again.",
        );
        let example = self.button(page, id::EXAMPLE, "Add PI / harness example");
        let try_label = self.label(page, "Try it");
        let sample = self.bound_edit(page, |f| &mut f.sample);
        let preview = self.label(page, "");
        *self.dictionary.borrow_mut() = vec![
            hint, words_heading, words, words_hint, rows_heading, original, replace_with, rows_hint, example, try_label, sample, preview,
        ];
    }

    fn build_rules(&self, page: HWND) {
        let hint = self.hint(page, "Applied from top to bottom.");
        let list = self.list(page, id::RULE_LIST);
        let add = self.button(page, id::ADD_RULE, "Add rule");
        let remove = self.button(page, id::REMOVE_RULE, "Remove");
        let up = self.button(page, id::MOVE_UP, "Move up");
        let down = self.button(page, id::MOVE_DOWN, "Move down");
        let name_label = self.label(page, "Rule name");
        let name = self.edit(page, id::RULE_NAME, 0);
        let enabled = self.check(page, id::RULE_ENABLED, "Enable this rule");
        let instructions_label = self.label(page, "Instructions");
        let instructions = self.multiline(page, id::INSTRUCTIONS);
        let apps_label = self.label(page, "Apps");
        let apps = self.list(page, id::APPS);
        let remove_app = self.button(page, id::REMOVE_APP, "Remove selected app");
        let add_label = self.label(page, "Add a running app or enter its executable name");
        let app_name = self.combo(page, id::APP_NAME, true, &[]);
        let add_app = self.button(page, id::ADD_APP, "Add app");
        let browse = self.button(page, id::BROWSE, "Browse…");
        let note = self.hint(page, "Rules use the app in front when recording ends. An active rule applies even with post-processing off.");
        *self.rules.borrow_mut() = vec![
            hint, list, add, remove, up, down, name_label, name, enabled, instructions_label, instructions, apps_label, apps, remove_app, add_label,
            app_name, add_app, browse, note,
        ];
    }

    fn build_server(&self, page: HWND) {
        let enabled = self.bound_check(page, |f| &mut f.server_enabled, "Allow a Mac to dictate through this PC");
        let hint = self.hint(
            page,
            "The PC transcribes the audio and uses its own post-processing provider. On the Mac, paste the URL and token from the tray's Copy Mac connection info.",
        );
        let port_label = self.label(page, "Port");
        let port = self.bound_edit(page, |f| &mut f.server_port);
        let bind_label = self.label(page, "Listen address");
        let bind = self.bound_edit(page, |f| &mut f.server_bind);
        let note = self.hint(
            page,
            "0.0.0.0 listens on every network adapter. Keep the connection token private. If you change the port, update the Windows Firewall rule for Parakeet Dictate too.",
        );
        *self.server.borrow_mut() = vec![enabled, hint, port_label, port, bind_label, bind, note];
    }

    fn build(&self) {
        let main = self.main;
        self.title.set(self.create("STATIC", "Settings", SS_NOPREFIX, 0, main, 0, self.fonts.title));
        let subtitle = self.hint(main, "Dictation, post-processing, and the apps you use.");
        self.subtitle.set(subtitle);
        let tabs = unsafe {
            let class = from_wide(std::slice::from_raw_parts(WC_TABCONTROLW, 64));
            self.create(&class, "", WS_TABSTOP | WS_CLIPSIBLINGS, 0, main, 0, self.fonts.body)
        };
        self.tabs.set(tabs);
        let mut pages = Vec::new();
        for (index, title) in ["General", "Post-processing", "Dictionary", "App rules", "Server for Mac"].into_iter().enumerate() {
            let mut text = wide(title);
            let mut item: TCITEMW = unsafe { std::mem::zeroed() };
            item.mask = TCIF_TEXT;
            item.pszText = text.as_mut_ptr();
            unsafe { SendMessageW(tabs, TCM_INSERTITEMW, index, (&raw const item) as LPARAM) };
            let style = WS_VSCROLL | WS_CLIPCHILDREN;
            let page = self.create(PAGE_CLASS, "", style, WS_EX_CONTROLPARENT, main, 0, self.fonts.body);
            show(page, index == 0);
            pages.push(page);
        }
        self.build_general(pages[0]);
        self.build_post(pages[1]);
        self.build_dictionary(pages[2]);
        self.build_rules(pages[3]);
        self.build_server(pages[4]);
        // Each new child goes below the ones before it, which leaves the pages under the tab control, and it
        // paints its whole client area over them. Below the pages, WS_CLIPSIBLINGS keeps it off them.
        unsafe { SetWindowPos(tabs, pages[4], 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
        *self.pages.borrow_mut() = pages;
        let status = self.label(main, "");
        self.status.set(status);
        self.reload.set(self.button(main, id::RELOAD, "Reload"));
        self.close.set(self.button(main, id::CLOSE, "Close"));
        self.save.set(self.button(main, id::SAVE, "Save and apply"));
    }

    fn text_height(&self, control: HWND, width: i32) -> i32 {
        let text = wide(&text_of(control));
        let font = unsafe { SendMessageW(control, windows_sys::Win32::UI::WindowsAndMessaging::WM_GETFONT, 0, 0) };
        let mut rect = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
        unsafe {
            let dc: HDC = GetDC(control);
            let previous = SelectObject(dc, font as _);
            DrawTextW(dc, text.as_ptr(), -1, &raw mut rect, DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX);
            SelectObject(dc, previous);
            ReleaseDC(control, dc);
        }
        rect.bottom.max(self.px(18)) + self.px(2)
    }

    fn layout_main(&self) {
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(self.main, &raw mut client) };
        let (width, height) = (client.right, client.bottom);
        let margin = self.px(18);
        let inner = width - margin * 2;
        self.place(self.title.get(), margin, margin, inner, self.px(36));
        self.place(self.subtitle.get(), margin, margin + self.px(36), inner, self.px(20));
        let top = margin + self.px(68);
        let button_height = self.px(30);
        let buttons_y = height - margin - button_height;
        let status_height = self.px(44);
        let status_y = buttons_y - status_height - self.px(4);
        self.place(self.status.get(), margin, status_y, inner, status_height);
        self.place(self.reload.get(), margin, buttons_y, self.px(90), button_height);
        let save_width = self.px(130);
        self.place(self.save.get(), width - margin - save_width, buttons_y, save_width, button_height);
        self.place(self.close.get(), width - margin - save_width - self.px(98), buttons_y, self.px(90), button_height);
        let tabs_height = status_y - top - self.px(8);
        self.place(self.tabs.get(), margin, top, inner, tabs_height);
        let mut area = RECT { left: margin, top, right: margin + inner, bottom: top + tabs_height };
        unsafe { SendMessageW(self.tabs.get(), TCM_ADJUSTRECT, 0, (&raw mut area) as LPARAM) };
        let pages = self.pages.borrow().clone();
        for page in pages {
            self.place(page, area.left, area.top, area.right - area.left, area.bottom - area.top);
        }
    }

    fn page_index(&self, page: HWND) -> Option<usize> {
        self.pages.borrow().iter().position(|known| *known == page)
    }

    fn layout_page(&self, page: HWND) {
        let Some(index) = self.page_index(page) else {
            return;
        };
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(page, &raw mut client) };
        let padding = self.px(20);
        let scrolled = self.scrolled.borrow()[index];
        let mut flow = Flow { ui: self, x: padding, width: client.right - padding * 2, y: padding - scrolled };
        match index {
            0 => self.flow_general(&mut flow),
            1 => self.flow_post(&mut flow),
            2 => self.flow_dictionary(&mut flow),
            3 => self.flow_rules(&mut flow, client.bottom - padding * 2),
            _ => self.flow_server(&mut flow),
        }
        let content = flow.y + scrolled + padding;
        let mut info: SCROLLINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SCROLLINFO>() as u32;
        info.fMask = SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL;
        info.nMax = content - 1;
        info.nPage = client.bottom.max(0) as u32;
        let limit = (content - client.bottom).max(0);
        info.nPos = scrolled.min(limit);
        unsafe { SetScrollInfo(page, SB_VERT, &raw const info, 1) };
        if info.nPos != scrolled {
            self.scroll_to(page, info.nPos);
        }
    }

    fn scroll_to(&self, page: HWND, position: i32) {
        let Some(index) = self.page_index(page) else {
            return;
        };
        let mut info: SCROLLINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SCROLLINFO>() as u32;
        info.fMask = SIF_ALL;
        unsafe { GetScrollInfo(page, SB_VERT, &raw mut info) };
        let limit = (info.nMax + 1 - info.nPage as i32).max(0);
        let position = position.clamp(0, limit);
        let old = std::mem::replace(&mut self.scrolled.borrow_mut()[index], position);
        unsafe {
            SetScrollPos(page, SB_VERT, position, 1);
            ScrollWindowEx(
                page,
                0,
                old - position,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                SW_SCROLLCHILDREN | SW_INVALIDATE | SW_ERASE,
            );
            UpdateWindow(page);
        }
    }

    /// Scroll so a control that took the focus shows.
    fn reveal(&self, page: HWND, control: HWND) {
        let Some(index) = self.page_index(page) else {
            return;
        };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut client = rect;
        unsafe {
            GetWindowRect(control, &raw mut rect);
            MapWindowPoints(std::ptr::null_mut(), page, (&raw mut rect).cast::<POINT>(), 2);
            GetClientRect(page, &raw mut client);
        }
        let start = self.scrolled.borrow()[index];
        if rect.top < self.px(32) {
            self.scroll_to(page, start + rect.top - self.px(32));
        } else if rect.bottom > client.bottom {
            self.scroll_to(page, start + rect.bottom - client.bottom + self.px(12));
        }
    }

    fn flow_general(&self, flow: &mut Flow<'_>) {
        use general::*;
        let c = self.general.borrow();
        flow.field(c[HOTKEY_LABEL], c[HOTKEY], 30);
        flow.text(c[HOTKEY_HINT], 0, 14);
        flow.field(c[ENGINE_LABEL], c[ENGINE], 30);
        flow.field(c[OVERLAY_LABEL], c[OVERLAY], 30);
        flow.line(c[SOUNDS], 24);
        flow.line(c[SPACE], 24);
        flow.gap(6);
        flow.line(c[LINE], 2);
        flow.gap(6);
        flow.line(c[ADVANCED], 24);
        flow.field(c[TAP_LABEL], c[TAP], 28);
        flow.field(c[FOLDER_LABEL], c[FOLDER], 28);
        flow.row(flow.label_width(), &[(c[CHOOSE], Some(140))], 30);
    }

    fn flow_post(&self, flow: &mut Flow<'_>) {
        use post::*;
        let c = self.post.borrow();
        let left = flow.label_width();
        flow.line(c[ENABLED], 24);
        flow.text(c[ENABLED_HINT], 0, 14);
        flow.field(c[PROVIDER_LABEL], c[PROVIDER], 30);
        let y = flow.y;
        self.place(c[KEY_LABEL], flow.x, y + self.px(4), left - self.px(8), self.px(22));
        flow.row(left, &[(c[KEY], None), (c[SHOW_KEY], Some(70))], 28);
        flow.text(c[KEY_HINT], left, 12);
        flow.field(c[MODEL_LABEL], c[MODEL], 30);
        if self.editor.borrow().provider() == crate::config::PolishProvider::Anthropic {
            flow.line(c[THINKING], 24);
        }
        flow.field(c[EFFORT_LABEL], c[EFFORT], 30);
        flow.field(c[TIMEOUT_LABEL], c[TIMEOUT], 28);
        flow.field(c[EXTRA_LABEL], c[EXTRA], 72);
        flow.text(c[EXTRA_HINT], left, 12);
        flow.field(c[ENDPOINT_LABEL], c[ENDPOINT], 28);
        flow.row(left, &[(c[TEST], Some(130))], 30);
        flow.text(c[TEST_STATUS], left, 4);
    }

    fn flow_dictionary(&self, flow: &mut Flow<'_>) {
        use dictionary::*;
        let c = self.dictionary.borrow();
        flow.text(c[HINT], 0, 14);
        flow.line(c[WORDS_HEADING], 24);
        flow.line(c[WORDS], 96);
        flow.text(c[WORDS_HINT], 0, 14);
        flow.line(c[ROWS_HEADING], 24);
        flow.row(0, &[(c[ORIGINAL], None), (c[REPLACE_WITH], None), (std::ptr::null_mut(), Some(36)), (std::ptr::null_mut(), Some(36))], 20);
        let can_remove = self.editor.borrow().rows_can_be_removed();
        for row in self.rows.borrow().iter() {
            show(row.remove, can_remove);
            flow.row(0, &[(row.source, None), (row.target, None), (row.remove, Some(36)), (row.add, Some(36))], 28);
        }
        flow.text(c[ROWS_HINT], 0, 12);
        flow.row(0, &[(c[EXAMPLE], Some(220))], 30);
        flow.gap(6);
        flow.field(c[TRY_LABEL], c[SAMPLE], 28);
        flow.text(c[PREVIEW], 0, 8);
    }

    fn flow_rules(&self, flow: &mut Flow<'_>, height: i32) {
        use rules::*;
        let c = self.rules.borrow();
        let column = self.px(220);
        let (x, top, width) = (flow.x, flow.y, flow.width);
        let gap = self.px(8);
        self.place(c[HINT], x, top, column, self.px(20));
        let buttons = self.px(30);
        let list_top = top + self.px(26);
        let list_height = (height - self.px(26) - buttons * 2 - gap * 2).max(self.px(200));
        self.place(c[LIST], x, list_top, column, list_height);
        let half = (column - gap) / 2;
        let below = list_top + list_height + gap;
        self.place(c[ADD], x, below, half, buttons);
        self.place(c[REMOVE], x + half + gap, below, half, buttons);
        self.place(c[UP], x, below + buttons + gap, half, buttons);
        self.place(c[DOWN], x + half + gap, below + buttons + gap, half, buttons);
        let left_end = below + buttons * 2 + gap;

        flow.x = x + column + self.px(18);
        flow.width = width - column - self.px(18);
        flow.line(c[NAME_LABEL], 20);
        flow.line(c[NAME], 28);
        flow.line(c[ENABLED], 24);
        flow.gap(4);
        flow.line(c[INSTRUCTIONS_LABEL], 20);
        flow.line(c[INSTRUCTIONS], 92);
        flow.gap(4);
        flow.line(c[APPS_LABEL], 20);
        flow.line(c[APPS], 84);
        flow.row(0, &[(c[REMOVE_APP], Some(170))], 30);
        flow.line(c[ADD_LABEL], 20);
        flow.line(c[APP_NAME], 30);
        flow.row(0, &[(c[ADD_APP], Some(90)), (c[BROWSE], Some(90))], 30);
        flow.gap(6);
        flow.text(c[NOTE], 0, 4);
        flow.y = flow.y.max(left_end);
    }

    fn flow_server(&self, flow: &mut Flow<'_>) {
        use server::*;
        let c = self.server.borrow();
        flow.line(c[ENABLED], 24);
        flow.text(c[HINT], 0, 14);
        flow.field(c[PORT_LABEL], c[PORT], 28);
        flow.field(c[BIND_LABEL], c[BIND], 28);
        flow.text(c[NOTE], 0, 4);
    }

    fn current_page(&self) -> HWND {
        let index = unsafe { SendMessageW(self.tabs.get(), TCM_GETCURSEL, 0, 0) };
        self.pages.borrow()[index.max(0) as usize]
    }

    fn select_tab(&self) {
        let current = self.current_page();
        let pages = self.pages.borrow().clone();
        for page in pages {
            show(page, page == current);
        }
        self.layout_page(current);
    }

    fn relayout(&self) {
        let pages = self.pages.borrow().clone();
        for page in pages {
            self.layout_page(page);
        }
    }

    fn rebuild_rows(&self) {
        let page = self.pages.borrow()[2];
        for row in self.rows.borrow_mut().drain(..) {
            unsafe {
                DestroyWindow(row.source);
                DestroyWindow(row.target);
                DestroyWindow(row.remove);
                DestroyWindow(row.add);
            }
        }
        let values = self.editor.borrow().fields.replacements.clone();
        let mut rows = Vec::with_capacity(values.len());
        for (index, (source, target)) in values.iter().enumerate() {
            let base = ROW_IDS + index as u16 * IDS_PER_ROW;
            let row = Row {
                source: self.edit(page, base, 0),
                target: self.edit(page, base + 1, 0),
                remove: self.button(page, base + 2, "−"),
                add: self.button(page, base + 3, "+"),
            };
            set_text(row.source, source);
            set_text(row.target, target);
            rows.push(row);
        }
        *self.rows.borrow_mut() = rows;
    }

    fn show_rule(&self) {
        use rules::*;
        let c = self.rules.borrow().clone();
        let (rules_list, selected) = {
            let editor = self.editor.borrow();
            (editor.fields.rules.clone(), editor.fields.selected_rule)
        };
        unsafe { SendMessageW(c[LIST], LB_RESETCONTENT, 0, 0) };
        for rule in &rules_list {
            unsafe { SendMessageW(c[LIST], LB_ADDSTRING, 0, wide(&rule_title(rule)).as_ptr() as LPARAM) };
        }
        unsafe { SendMessageW(c[LIST], LB_SETCURSEL, selected.unwrap_or(usize::MAX), 0) };
        let rule = selected.and_then(|index| rules_list.get(index));
        set_text(c[NAME], rule.map_or("", |rule| &rule.name));
        set_checked(c[ENABLED], rule.is_some_and(|rule| rule.enabled));
        set_text(c[INSTRUCTIONS], &to_control(rule.map_or("", |rule| &rule.instructions)));
        unsafe { SendMessageW(c[APPS], LB_RESETCONTENT, 0, 0) };
        for app in rule.map_or(&[][..], |rule| &rule.apps) {
            unsafe { SendMessageW(c[APPS], LB_ADDSTRING, 0, wide(app).as_ptr() as LPARAM) };
        }
        set_text(c[APP_NAME], "");
        let any = rule.is_some();
        for control in [NAME, ENABLED, INSTRUCTIONS, APPS, APP_NAME, ADD_APP, BROWSE, REMOVE_APP, REMOVE] {
            enable(c[control], any);
        }
        enable(c[UP], selected.is_some_and(|index| index > 0));
        enable(c[DOWN], selected.is_some_and(|index| index + 1 < rules_list.len()));
    }

    fn show_provider(&self) {
        use post::*;
        let c = self.post.borrow().clone();
        let (label, key, hint, managed, models, efforts, effort, thinking, anthropic, model) = {
            let editor = self.editor.borrow();
            (
                editor.key_label(),
                editor.fields.keys.get(editor.api_key_name()).cloned().unwrap_or_default(),
                editor.key_hint(),
                editor.key_is_managed(),
                editor.model_choices().iter().map(|model| (*model).to_owned()).collect::<Vec<_>>(),
                editor.allowed_efforts().iter().map(|effort| (*effort).to_owned()).collect::<Vec<_>>(),
                editor.fields.effort.clone(),
                editor.fields.thinking,
                editor.provider() == crate::config::PolishProvider::Anthropic,
                editor.fields.polish_model.clone(),
            )
        };
        set_text(c[KEY_LABEL], label);
        set_text(c[KEY], &key);
        unsafe {
            SendMessageW(c[KEY], EM_SETREADONLY, usize::from(managed), 0);
            SendMessageW(c[KEY], EM_SETPASSWORDCHAR, BULLET, 0);
        }
        set_checked(c[SHOW_KEY], false);
        set_text(c[KEY_HINT], &hint);
        set_combo_values(c[MODEL], &models);
        set_combo(c[MODEL], &model);
        show(c[THINKING], anthropic);
        set_checked(c[THINKING], thinking);
        set_combo_values(c[EFFORT], &efforts);
        set_combo(c[EFFORT], &effort);
    }

    /// Copy the whole form into the controls, without those writes counting as edits.
    fn show_form(&self) {
        self.loading.set(true);
        let fields = self.editor.borrow().fields.clone();
        let mut copy = fields;
        let text = self.bound_text.borrow().clone();
        for (control, field, lines) in text {
            let value = field(&mut copy).clone();
            if self.combos.borrow().contains(&control) {
                set_combo(control, &value);
            } else {
                set_text(control, &if lines { to_control(&value) } else { value });
            }
        }
        let flags = self.bound_flags.borrow().clone();
        for (control, field) in flags {
            set_checked(control, *field(&mut copy));
        }
        let provider = self.post.borrow()[post::PROVIDER];
        set_combo(provider, &copy.polish_provider);
        self.show_provider();
        self.rebuild_rows();
        self.show_rule();
        self.loading.set(false);
        self.refresh();
        self.relayout();
    }

    /// The parts of the window that follow from the form: Save, the status, the preview, the test.
    fn refresh(&self) {
        let (dirty, status, preview, test_status, testing) = {
            let editor = self.editor.borrow();
            (editor.dirty, editor.status.clone(), editor.dictionary_preview(), editor.test_status.clone(), editor.testing())
        };
        enable(self.save.get(), dirty);
        if text_of(self.status.get()) != status {
            set_text(self.status.get(), &status);
        }
        unsafe { InvalidateRect(self.status.get(), std::ptr::null(), 1) };
        let preview_label = self.dictionary.borrow()[dictionary::PREVIEW];
        if text_of(preview_label) != preview {
            set_text(preview_label, &preview);
            self.layout_page(self.pages.borrow()[2]);
        }
        let test_label = self.post.borrow()[post::TEST_STATUS];
        if text_of(test_label) != test_status {
            set_text(test_label, &test_status);
            let page = self.pages.borrow()[1];
            self.layout_page(page);
            if !test_status.is_empty() {
                self.reveal(page, test_label);
            }
        }
        enable(self.post.borrow()[post::TEST], !testing);
    }

    fn confirm(&self, title: &str, question: &str) -> bool {
        let (title, question) = (wide(title), wide(question));
        unsafe { MessageBoxW(self.main, question.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
    }

    fn save_settings(&self) {
        self.editor.borrow_mut().save();
        self.refresh();
    }

    fn reload_settings(&self) {
        if self.editor.borrow().dirty && !self.confirm("Reload settings", "Discard your unsaved changes and reload?") {
            return;
        }
        self.editor.borrow_mut().reload();
        self.show_form();
    }

    fn close_window(&self) {
        if self.editor.borrow().dirty && !self.confirm("Unsaved changes", "Discard your unsaved changes and close?") {
            return;
        }
        unsafe { DestroyWindow(self.main) };
    }

    fn edited(&self) {
        self.editor.borrow_mut().changed();
        self.refresh();
    }

    fn bound_changed(&self, control: HWND) -> bool {
        let found = self.bound_text.borrow().iter().find(|(known, ..)| *known == control).copied();
        if let Some((_, field, _)) = found {
            let value = if self.combos.borrow().contains(&control) { combo_value(control) } else { from_control(&text_of(control)) };
            *field(&mut self.editor.borrow_mut().fields) = value;
            self.edited();
            return true;
        }
        let found = self.bound_flags.borrow().iter().find(|(known, _)| *known == control).copied();
        if let Some((_, field)) = found {
            *field(&mut self.editor.borrow_mut().fields) = is_checked(control);
            self.edited();
            return true;
        }
        false
    }

    fn row_command(&self, identifier: u16, code: u32, control: HWND) {
        let offset = identifier - ROW_IDS;
        let (index, part) = (usize::from(offset / IDS_PER_ROW), offset % IDS_PER_ROW);
        match (part, code) {
            (0, EN_CHANGE) => {
                if let Some(row) = self.editor.borrow_mut().fields.replacements.get_mut(index) {
                    row.0 = text_of(control);
                }
                self.edited();
                self.layout_page(self.pages.borrow()[2]);
            }
            (1, EN_CHANGE) => {
                if let Some(row) = self.editor.borrow_mut().fields.replacements.get_mut(index) {
                    row.1 = text_of(control);
                }
                self.edited();
                self.layout_page(self.pages.borrow()[2]);
            }
            (2, BN_CLICKED) => {
                self.editor.borrow_mut().remove_row(index);
                self.rows_changed();
            }
            (3, BN_CLICKED) => {
                self.editor.borrow_mut().add_row(index);
                self.rows_changed();
            }
            _ => {}
        }
    }

    fn rows_changed(&self) {
        self.loading.set(true);
        self.rebuild_rows();
        self.loading.set(false);
        self.refresh();
        self.layout_page(self.pages.borrow()[2]);
    }

    fn rule_changed(&self) {
        self.loading.set(true);
        self.show_rule();
        self.loading.set(false);
        self.refresh();
    }

    fn update_rule(&self, change: impl FnOnce(&mut crate::app_rules::AppRule)) {
        let list = self.rules.borrow()[rules::LIST];
        let title = {
            let mut editor = self.editor.borrow_mut();
            let Some(rule) = editor.selected_rule() else {
                return;
            };
            change(rule);
            let title = rule_title(rule);
            editor.changed();
            title
        };
        let selected = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) };
        if selected >= 0 {
            self.loading.set(true);
            unsafe {
                SendMessageW(list, windows_sys::Win32::UI::WindowsAndMessaging::LB_DELETESTRING, selected as usize, 0);
                SendMessageW(list, windows_sys::Win32::UI::WindowsAndMessaging::LB_INSERTSTRING, selected as usize, wide(&title).as_ptr() as LPARAM);
                SendMessageW(list, LB_SETCURSEL, selected as usize, 0);
            }
            self.loading.set(false);
        }
        self.refresh();
    }

    fn add_rule_menu(&self, button: HWND) {
        let chosen = unsafe {
            let menu = CreatePopupMenu();
            for (index, (name, _)) in PRESETS.iter().enumerate() {
                AppendMenuW(menu, MF_STRING, index + 1, wide(name).as_ptr());
            }
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(button, &raw mut rect);
            let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, rect.left, rect.bottom, 0, self.main, std::ptr::null());
            DestroyMenu(menu);
            chosen
        };
        if chosen > 0 {
            let (name, instructions) = PRESETS[chosen as usize - 1];
            self.editor.borrow_mut().add_rule(name, instructions);
            self.rule_changed();
            unsafe { SetFocus(self.rules.borrow()[rules::NAME]) };
        }
    }

    fn add_typed_app(&self, name: &str) {
        let added = self.editor.borrow_mut().add_app(name);
        if added {
            self.rule_changed();
        } else {
            self.refresh();
        }
    }

    fn choose_folder(&self) {
        let title = wide("Choose the Parakeet model folder");
        let mut info: BROWSEINFOW = unsafe { std::mem::zeroed() };
        info.hwndOwner = self.main;
        info.lpszTitle = title.as_ptr();
        info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        let mut path = [0u16; 32_768];
        let chosen = unsafe {
            let list = SHBrowseForFolderW(&raw const info);
            if list.is_null() {
                return;
            }
            let found = SHGetPathFromIDListW(list, path.as_mut_ptr()) != 0;
            CoTaskMemFree(list as *const _);
            found
        };
        if chosen {
            set_text(self.general.borrow()[general::FOLDER], &from_wide(&path));
        }
    }

    fn browse_apps(&self) {
        let filter: Vec<u16> = "Windows apps\0*.exe\0\0".encode_utf16().collect();
        let title = wide("Choose apps");
        let mut buffer = vec![0u16; 65_536];
        let mut request: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        request.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        request.hwndOwner = self.main;
        request.lpstrFilter = filter.as_ptr();
        request.lpstrFile = buffer.as_mut_ptr();
        request.nMaxFile = buffer.len() as u32;
        request.lpstrTitle = title.as_ptr();
        request.Flags = OFN_EXPLORER | OFN_ALLOWMULTISELECT | OFN_FILEMUSTEXIST;
        if unsafe { GetOpenFileNameW(&raw mut request) } == 0 {
            return;
        }
        for name in chosen_files(&buffer) {
            self.add_typed_app(&name);
        }
    }

    fn page_command(&self, page: HWND, identifier: u16, code: u32, control: HWND) {
        if matches!(code, EN_SETFOCUS | BN_SETFOCUS) || code == CBN_SETFOCUS || code == LBN_SETFOCUS {
            self.reveal(page, control);
        }
        if !self.loading.get() {
            self.command(identifier, code, control);
        }
    }

    fn command(&self, identifier: u16, code: u32, control: HWND) {
        if identifier >= ROW_IDS {
            self.row_command(identifier, code, control);
            return;
        }
        if identifier >= id::BOUND && matches!(code, EN_CHANGE | BN_CLICKED) {
            self.bound_changed(control);
            return;
        }
        if identifier >= id::BOUND && (code == CBN_SELCHANGE || code == CBN_EDITCHANGE) {
            self.bound_changed(control);
            return;
        }
        match (identifier, code) {
            (id::CHOOSE_FOLDER, BN_CLICKED) => self.choose_folder(),
            (id::POLISH_PROVIDER, CBN_SELCHANGE) => {
                self.editor.borrow_mut().change_provider(&combo_value(control));
                self.loading.set(true);
                self.show_provider();
                let endpoint = self.post.borrow()[post::ENDPOINT];
                set_text(endpoint, &self.editor.borrow().fields.endpoint);
                self.loading.set(false);
                self.refresh();
                self.layout_page(self.pages.borrow()[1]);
            }
            (id::KEY, EN_CHANGE) => {
                let name = self.editor.borrow().api_key_name().to_owned();
                self.editor.borrow_mut().fields.keys.insert(name, text_of(control));
                self.edited();
            }
            (id::SHOW_KEY, BN_CLICKED) => {
                let key = self.post.borrow()[post::KEY];
                unsafe {
                    SendMessageW(key, EM_SETPASSWORDCHAR, if is_checked(control) { 0 } else { BULLET }, 0);
                    InvalidateRect(key, std::ptr::null(), 1);
                }
            }
            (id::THINKING, BN_CLICKED) => {
                {
                    let mut editor = self.editor.borrow_mut();
                    editor.fields.thinking = is_checked(control);
                    editor.limit_effort();
                    editor.changed();
                }
                let (efforts, effort) = {
                    let editor = self.editor.borrow();
                    (editor.allowed_efforts().iter().map(|effort| (*effort).to_owned()).collect::<Vec<_>>(), editor.fields.effort.clone())
                };
                let combo = self.post.borrow()[post::EFFORT];
                self.loading.set(true);
                set_combo_values(combo, &efforts);
                set_combo(combo, &effort);
                self.loading.set(false);
                self.refresh();
            }
            (id::TEST, BN_CLICKED) => {
                if self.editor.borrow_mut().test_provider() {
                    unsafe { SetTimer(self.main, TEST_TIMER, 100, None) };
                }
                self.refresh();
            }
            (id::EXAMPLE, BN_CLICKED) => {
                self.editor.borrow_mut().dictionary_example();
                self.loading.set(true);
                let words = self.dictionary.borrow()[dictionary::WORDS];
                set_text(words, &to_control(&self.editor.borrow().fields.words));
                self.loading.set(false);
                self.rows_changed();
            }
            (id::RULE_LIST, LBN_SELCHANGE) => {
                let index = unsafe { SendMessageW(control, LB_GETCURSEL, 0, 0) };
                self.editor.borrow_mut().fields.selected_rule = usize::try_from(index).ok();
                self.rule_changed();
            }
            (id::ADD_RULE, BN_CLICKED) => self.add_rule_menu(control),
            (id::REMOVE_RULE, BN_CLICKED) => {
                self.editor.borrow_mut().remove_rule();
                self.rule_changed();
            }
            (id::MOVE_UP, BN_CLICKED) => {
                self.editor.borrow_mut().move_rule(-1);
                self.rule_changed();
            }
            (id::MOVE_DOWN, BN_CLICKED) => {
                self.editor.borrow_mut().move_rule(1);
                self.rule_changed();
            }
            (id::RULE_NAME, EN_CHANGE) => {
                let value = text_of(control);
                self.update_rule(|rule| rule.name = value);
            }
            (id::RULE_ENABLED, BN_CLICKED) => {
                let value = is_checked(control);
                self.update_rule(|rule| rule.enabled = value);
            }
            (id::INSTRUCTIONS, EN_CHANGE) => {
                let value = from_control(&text_of(control));
                self.update_rule(|rule| rule.instructions = value);
            }
            (id::REMOVE_APP, BN_CLICKED) => {
                let apps = self.rules.borrow()[rules::APPS];
                let index = unsafe { SendMessageW(apps, LB_GETCURSEL, 0, 0) };
                if let Ok(index) = usize::try_from(index) {
                    self.editor.borrow_mut().remove_app(index);
                    self.rule_changed();
                }
            }
            (id::APP_NAME, CBN_DROPDOWN) => set_combo_values(control, &running_executables()),
            (id::ADD_APP, BN_CLICKED) => {
                let combo = self.rules.borrow()[rules::APP_NAME];
                self.add_typed_app(&text_of(combo));
            }
            (id::BROWSE, BN_CLICKED) => self.browse_apps(),
            (id::RELOAD, BN_CLICKED) => self.reload_settings(),
            (id::CLOSE, BN_CLICKED) => self.close_window(),
            (id::SAVE, BN_CLICKED) => self.save_settings(),
            _ => {}
        }
    }

    fn poll_test(&self) {
        if !self.editor.borrow_mut().poll_provider_test() {
            unsafe { KillTimer(self.main, TEST_TIMER) };
        }
        self.refresh();
    }

    fn color(&self, dc: HDC, control: HWND) -> LRESULT {
        let tone = self.editor.try_borrow().map_or(Tone::Neutral, |editor| editor.tone);
        let color = if control == self.status.get() {
            match tone {
                Tone::Neutral => HINT_COLOR,
                Tone::Success => SUCCESS_COLOR,
                Tone::Failure => FAILURE_COLOR,
            }
        } else if self.hints.borrow().contains(&control) {
            HINT_COLOR
        } else {
            unsafe { GetSysColor(windows_sys::Win32::Graphics::Gdi::COLOR_WINDOWTEXT) }
        };
        let managed = control == self.post.borrow()[post::KEY];
        let background = if managed { COLOR_BTNFACE } else { COLOR_WINDOW };
        unsafe {
            SetTextColor(dc, color);
            SetBkColor(dc, GetSysColor(background));
            GetSysColorBrush(background) as LRESULT
        }
    }

    fn any_combo_open(&self) -> bool {
        self.combos.borrow().iter().any(|combo| unsafe { SendMessageW(*combo, CB_GETDROPPEDSTATE, 0, 0) != 0 })
    }
}

/// The names of the files GetOpenFileName chose: one full path, or a folder followed by names.
fn chosen_files(buffer: &[u16]) -> Vec<String> {
    let end = buffer.windows(2).position(|pair| pair == [0, 0]).unwrap_or(buffer.len());
    let parts: Vec<String> = buffer[..end].split(|unit| *unit == 0).map(String::from_utf16_lossy).collect();
    if parts.len() > 1 {
        return parts[1..].to_vec();
    }
    let name = |part: &String| Path::new(part).file_name().map(|name| name.to_string_lossy().into_owned());
    parts.iter().filter_map(name).collect()
}

unsafe extern "system" fn main_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(ui) = ui() else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    match message {
        WM_COMMAND => {
            let (identifier, code) = ((wparam & 0xffff) as u16, ((wparam >> 16) & 0xffff) as u32);
            if lparam != 0 && !ui.loading.get() {
                ui.command(identifier, code, lparam as HWND);
            }
            0
        }
        WM_NOTIFY => {
            let header = lparam as *const NMHDR;
            if unsafe { (*header).code } == TCN_SELCHANGE {
                ui.select_tab();
            }
            0
        }
        WM_SIZE => {
            ui.layout_main();
            ui.relayout();
            0
        }
        WM_GETMINMAXINFO => {
            let limits = lparam as *mut MINMAXINFO;
            unsafe {
                (*limits).ptMinTrackSize = POINT { x: ui.px(700), y: ui.px(500) };
            }
            0
        }
        WM_TIMER => {
            ui.poll_test();
            0
        }
        WM_CTLCOLORSTATIC => ui.color(wparam as HDC, lparam as HWND),
        WM_CLOSE => {
            ui.close_window();
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(ui) = ui() else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    match message {
        WM_COMMAND => {
            let (identifier, code) = ((wparam & 0xffff) as u16, ((wparam >> 16) & 0xffff) as u32);
            if lparam != 0 {
                ui.page_command(hwnd, identifier, code, lparam as HWND);
            }
            0
        }
        WM_CTLCOLORSTATIC => ui.color(wparam as HDC, lparam as HWND),
        WM_SIZE => {
            ui.layout_page(hwnd);
            0
        }
        WM_VSCROLL => {
            let mut info: SCROLLINFO = unsafe { std::mem::zeroed() };
            info.cbSize = size_of::<SCROLLINFO>() as u32;
            info.fMask = SIF_ALL;
            unsafe { GetScrollInfo(hwnd, SB_VERT, &raw mut info) };
            let line = ui.px(30);
            let target = match (wparam & 0xffff) as i32 {
                SB_LINEUP => info.nPos - line,
                SB_LINEDOWN => info.nPos + line,
                SB_PAGEUP => info.nPos - info.nPage as i32,
                SB_PAGEDOWN => info.nPos + info.nPage as i32,
                SB_THUMBTRACK => info.nTrackPos,
                SB_TOP => 0,
                SB_BOTTOM => info.nMax,
                _ => info.nPos,
            };
            ui.scroll_to(hwnd, target);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wparam >> 16) as u16 as i16);
            let current = ui.page_index(hwnd).map_or(0, |index| ui.scrolled.borrow()[index]);
            ui.scroll_to(hwnd, current - delta / 120 * ui.px(60));
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register(class: &str, procedure: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, icon: HICON) -> Result<(), String> {
    let name = wide(class);
    unsafe {
        let mut description: WNDCLASSEXW = std::mem::zeroed();
        description.cbSize = size_of::<WNDCLASSEXW>() as u32;
        description.lpfnWndProc = Some(procedure);
        description.hInstance = GetModuleHandleW(std::ptr::null());
        description.lpszClassName = name.as_ptr();
        description.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
        description.hIcon = icon;
        description.hCursor = windows_sys::Win32::UI::WindowsAndMessaging::LoadCursorW(
            std::ptr::null_mut(),
            windows_sys::Win32::UI::WindowsAndMessaging::IDC_ARROW,
        );
        if RegisterClassExW(&raw const description) == 0 {
            return Err(crate::win::last_error("RegisterClassExW"));
        }
    }
    Ok(())
}

fn app_icon() -> HICON {
    // Resource 1 is the icon build.rs embeds.
    unsafe { LoadImageW(GetModuleHandleW(std::ptr::null()), 1 as _, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE) as HICON }
}

fn activate(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}

/// Run the window until it closes. `show` brings it forward when another `--settings` asks.
pub fn run(show_event: &Handle, on_apply: Box<dyn Fn() -> std::io::Result<String>>) -> Result<(), String> {
    let editor = Editor::open(SettingsStore::default(), on_apply).map_err(|e| e.to_string())?;
    let classes = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES | ICC_TAB_CLASSES };
    unsafe {
        InitCommonControlsEx(&raw const classes);
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
    let icon = app_icon();
    register(MAIN_CLASS, main_proc, icon)?;
    register(PAGE_CLASS, page_proc, std::ptr::null_mut())?;

    let dpi = unsafe { GetDpiForSystem() } as i32;
    let scale = dpi as f32 / 96.0;
    let (screen_width, screen_height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let width = ((820.0 * scale) as i32).min(screen_width - 80);
    let height = ((720.0 * scale) as i32).min(screen_height - 100);
    let (class, title) = (wide(MAIN_CLASS), wide(TITLE));
    let main = unsafe {
        CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            if screen_width > width { (screen_width - width) / 2 } else { CW_USEDEFAULT },
            ((screen_height - height) / 2).max(0),
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if main.is_null() {
        return Err(crate::win::last_error("CreateWindowExW"));
    }
    unsafe {
        SendMessageW(main, WM_SETICON, ICON_BIG as usize, icon as LPARAM);
        SendMessageW(main, WM_SETICON, ICON_SMALL as usize, icon as LPARAM);
    }
    let ui: &'static Ui = Box::leak(Box::new(Ui {
        editor: RefCell::new(editor),
        main,
        scale,
        fonts: Fonts::new(dpi),
        loading: Cell::new(false),
        hints: RefCell::default(),
        combos: RefCell::default(),
        bound_text: RefCell::default(),
        bound_flags: RefCell::default(),
        tabs: Cell::new(std::ptr::null_mut()),
        pages: RefCell::default(),
        scrolled: RefCell::new([0; 5]),
        title: Cell::new(std::ptr::null_mut()),
        subtitle: Cell::new(std::ptr::null_mut()),
        status: Cell::new(std::ptr::null_mut()),
        reload: Cell::new(std::ptr::null_mut()),
        close: Cell::new(std::ptr::null_mut()),
        save: Cell::new(std::ptr::null_mut()),
        general: RefCell::default(),
        post: RefCell::default(),
        dictionary: RefCell::default(),
        rules: RefCell::default(),
        server: RefCell::default(),
        rows: RefCell::default(),
    }));
    ui.build();
    UI.with(|slot| slot.set(Some(ui)));
    ui.layout_main();
    ui.show_form();
    activate(main);
    pump(ui, show_event);
    Ok(())
}

fn pump(ui: &Ui, show_event: &Handle) {
    let handles = [show_event.0];
    loop {
        let woke = unsafe { MsgWaitForMultipleObjects(1, handles.as_ptr(), 0, u32::MAX, QS_ALLINPUT) };
        if woke == WAIT_OBJECT_0 {
            activate(ui.main);
        }
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&raw mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    return;
                }
                if message.message == WM_KEYDOWN {
                    if message.wParam == usize::from(b'S') && GetKeyState(i32::from(VK_CONTROL)) < 0 {
                        ui.save_settings();
                        continue;
                    }
                    if message.wParam == usize::from(VK_ESCAPE) && !ui.any_combo_open() {
                        ui.close_window();
                        continue;
                    }
                }
                if IsDialogMessageW(ui.main, &raw const message) != 0 {
                    continue;
                }
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}
