//! Global hotkeys.
//!
//! **Windows**: RegisterHotKey + a hidden window message pump.
//! **Linux (X11)**: XGrabKey on the root window; consumes KeyPress events.

use crate::config::{Action, Hotkey};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostQuitMessage, RegisterClassW, TranslateMessage, UnregisterClassW, WNDCLASSW,
        WM_DESTROY, WS_OVERLAPPEDWINDOW,
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

    /// Run the hotkey message loop until `stop` is set or the window is
    /// destroyed. Blocking.
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
            anyhow::bail!("RegisterClassW failed (is the class already registered?)");
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
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use crate::config::HotkeyKey;
    use anyhow::Context;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, EventMask, GrabMode, ModMask};
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    const XK_F1: u32 = 0xffbe;
    const XK_PRINT: u32 = 0xff61;
    const XK_ALT_L: u32 = 0xffe9;
    const XK_ALT_R: u32 = 0xffea;
    const XK_SUPER_L: u32 = 0xffeb;
    const XK_SUPER_R: u32 = 0xffec;
    const XK_SPACE: u32 = 0x20;

    /// CapsLock (LockMask=0x02) and NumLock (Mod2=0x10) must not change a
    /// hotkey's identity; grabs are registered for both states.
    const STRIP_MASK: u16 = 0x02 | 0x10;

    /// XGrabKey-powered hotkeys. Blocking loop reading KeyPress events.
    pub fn run(
        stop: Arc<AtomicBool>,
        tx: Sender<Action>,
        clip: Hotkey,
        record_start: Hotkey,
        record_stop: Hotkey,
    ) -> Result<()> {
        let (conn, screen_num) = x11rb::connect(None)
            .context("cannot connect to X server (hotkeys disabled without an X display)")?;
        let root = conn.setup().roots[screen_num].root;

        let mut binds: Vec<(u8, u16, Action)> = Vec::new();
        for (key, action) in [
            (clip, Action::Clip),
            (record_start, Action::RecordStart),
            (record_stop, Action::RecordStop),
        ] {
            match grab_key(&conn, root, key) {
                Ok(Some((keycode, mask))) => {
                    log::info!("hotkey {} -> keycode {keycode} mask {mask:#x}", key.as_str());
                    binds.push((keycode, mask, action));
                }
                Ok(None) => log::warn!("could not map hotkey {} to a keycode", key.as_str()),
                Err(e) => log::warn!("failed to grab hotkey {}: {e:#}", key.as_str()),
            }
        }
        if binds.is_empty() {
            anyhow::bail!("could not register any hotkeys");
        }

        conn.change_window_attributes(
            root,
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::default()
                .event_mask(EventMask::KEY_PRESS),
        )
        .context("failed to select KeyPress on root")?;

        while !stop.load(Ordering::Relaxed) {
            while let Some(event) = conn.poll_for_event().context("poll_for_event failed")? {
                if let Event::KeyPress(kev) = event {
                    let state = u16::from(kev.state) & !STRIP_MASK;
                    for (keycode, mask, action) in &binds {
                        if kev.detail == *keycode && state == *mask {
                            let _ = tx.send(*action);
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        for (keycode, _, _) in &binds {
            let _ = conn.ungrab_key(*keycode, root, ModMask::ANY);
        }
        Ok(())
    }

    /// Resolve a `Hotkey` to its keycode / baseline modifier mask and grab it.
    fn grab_key(
        conn: &RustConnection,
        root: x11rb::protocol::xproto::Window,
        key: Hotkey,
    ) -> Result<Option<(u8, u16)>> {
        let sym = match key.key {
            HotkeyKey::F(n) if (1..=12).contains(&n) => XK_F1 + u32::from(n) - 1,
            HotkeyKey::PrintScreen => XK_PRINT,
            HotkeyKey::Letter(c) => u32::from(c),
            HotkeyKey::Digit(d) => 0x30 + u32::from(d),
            HotkeyKey::Space => XK_SPACE,
            _ => return Ok(None),
        };
        let keycode = match keycode_for_sym(conn, sym) {
            Some(kc) => kc,
            None => return Ok(None),
        };

        let mut mask = 0u16;
        if key.mods & crate::config::MOD_SHIFT != 0 {
            mask |= u16::from(ModMask::SHIFT);
        }
        if key.mods & crate::config::MOD_CTRL != 0 {
            mask |= u16::from(ModMask::CONTROL);
        }
        if key.mods & crate::config::MOD_ALT != 0 {
            mask |= modifier_mask_for(conn, &[XK_ALT_L, XK_ALT_R])?;
        }
        if key.mods & crate::config::MOD_WIN != 0 {
            mask |= modifier_mask_for(conn, &[XK_SUPER_L, XK_SUPER_R])?;
        }

        // Grab with/without CapsLock and NumLock so the hotkey fires
        // regardless of lock-key state.
        let mut grabbed = false;
        for extra in [0u16, 0x02, 0x10, 0x02 | 0x10] {
            if conn
                .grab_key(true, root, ModMask::from(mask | extra), keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .is_ok()
            {
                grabbed = true;
            }
        }
        if !grabbed {
            log::warn!("XGrabKey failed for {}", key.as_str());
        }
        Ok(Some((keycode, mask)))
    }

    /// Find the first keycode whose keysym row contains `sym`.
    fn keycode_for_sym(conn: &RustConnection, sym: u32) -> Option<u8> {
        let setup = conn.setup();
        let first = setup.min_keycode;
        let count = setup.max_keycode - setup.min_keycode + 1;
        let kb = conn.get_keyboard_mapping(first, count).ok()?.reply().ok()?;
        let per = kb.keysyms_per_keycode as usize;
        for (i, row) in kb.keysyms.chunks(per).enumerate() {
            if row.iter().any(|s| *s == sym) {
                return Some(first + i as u8);
            }
        }
        None
    }

    /// Find the modifier mask bit for the given modifier keysyms by scanning
    /// the keyboard + modifier maps (e.g. which ModN is Alt / Super).
    fn modifier_mask_for(conn: &RustConnection, syms: &[u32]) -> Result<u16> {
        let setup = conn.setup();
        let first = setup.min_keycode;
        let count = setup.max_keycode - setup.min_keycode + 1;
        let kb = conn
            .get_keyboard_mapping(first, count)
            .context("get_keyboard_mapping failed")?
            .reply()
            .context("get_keyboard_mapping reply failed")?;

        let mut keycodes = std::collections::HashSet::new();
        let per = kb.keysyms_per_keycode as usize;
        for (i, row) in kb.keysyms.chunks(per).enumerate() {
            if row.iter().any(|s| syms.contains(s)) {
                keycodes.insert(first + i as u8);
            }
        }
        if keycodes.is_empty() {
            return Ok(0);
        }

        let mm = conn
            .get_modifier_mapping()
            .context("get_modifier_mapping failed")?
            .reply()
            .context("get_modifier_mapping reply failed")?;
        let per_mod = if mm.keycodes.is_empty() {
            1
        } else {
            mm.keycodes.len() / 8
        }
        .max(1);
        let mut mask = 0u16;
        for (i, kc) in mm.keycodes.iter().enumerate() {
            if *kc != 0 && keycodes.contains(kc) {
                mask |= 1u16 << (i / per_mod);
            }
        }
        Ok(mask)
    }
}

pub use imp::run;