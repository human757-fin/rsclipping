//! Global hotkeys via RegisterHotKey + a hidden window message pump.
//! Registers exactly three hotkeys and forwards events over an mpsc channel.

use crate::config::{Action, Hotkey};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, UnregisterClassW, WNDCLASSW, WM_DESTROY,
    WS_OVERLAPPEDWINDOW,
};
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

const WM_HOTKEY: u32 = 0x0312;

static CHANNEL: OnceLock<Mutex<Option<Sender<Action>>>> = OnceLock::new();

fn channel_ref() -> &'static Mutex<Option<Sender<Action>>> {
    CHANNEL.get_or_init(|| Mutex::new(None))
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_HOTKEY {
        let action = match wparam.0 as u32 {
            1 => Some(Action::Clip),
            2 => Some(Action::RecordStart),
            3 => Some(Action::RecordStop),
            _ => None,
        };
        if let Some(action) = action {
            if let Ok(guard) = channel_ref().lock() {
                if let Some(tx) = guard.as_ref() {
                    let _ = tx.send(action);
                }
            }
        }
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Run the hotkey message loop until `stop` is set or the window is destroyed. Blocking.
pub fn run(
    stop: Arc<AtomicBool>,
    tx: Sender<Action>,
    clip: Hotkey,
    record_start: Hotkey,
    record_stop: Hotkey,
) -> Result<()> {
    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();

    let class_name = w!("rsclipping_hotkey_window");
    let wc = WNDCLASSW {
        style: windows::Win32::UI::WindowsAndMessaging::WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: Default::default(),
        hCursor: Default::default(),
        hbrBackground: Default::default(),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: class_name,
    };

    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        anyhow::bail!(
            "RegisterClassW failed (is the class already registered?)"
        );
    }

    let hwnd = unsafe {
        CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            class_name,
            w!("rsclipping_hotkey"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
HWND(std::ptr::null_mut()),
            HMENU(std::ptr::null_mut()),
            hinstance,
            None,
        )
    }?;
    if hwnd.0.is_null() {
        anyhow::bail!("CreateWindowExW returned null HWND");
    }

    *channel_ref().lock().unwrap() = Some(tx);

    let regs = [
        (1i32, clip),
        (2i32, record_start),
        (3i32, record_stop),
    ];
    let mut registered = Vec::new();
    for (id, key) in regs {
        let mods = HOT_KEY_MODIFIERS(key.mods_code() | MOD_NOREPEAT.0);
        let ok = unsafe { RegisterHotKey(hwnd, id, mods, key.vk_code()) };
        if ok.is_err() {
            log::warn!("RegisterHotKey failed for {} (already in use?)", key.as_str());
        } else {
            registered.push(id);
        }
    }
    if registered.is_empty() {
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = unsafe { UnregisterClassW(class_name, hinstance) };
        anyhow::bail!("no hotkeys could be registered");
    }
    log::info!(
        "hotkeys: clip={} record={} stop={}",
        clip.as_str(),
        record_start.as_str(),
        record_stop.as_str()
    );

    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0) };
        if ret.0 == 0 {
            break;
        }
        let _ = unsafe { TranslateMessage(&msg) };
        unsafe { DispatchMessageW(&msg) };
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }

    for id in registered {
        let _ = unsafe { UnregisterHotKey(hwnd, id) };
    }
    let _ = unsafe { DestroyWindow(hwnd) };
    let _ = unsafe { UnregisterClassW(class_name, hinstance) };
    Ok(())
}