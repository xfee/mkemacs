#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ── 配置（直接改这里就行）────────────────────────────────────
const HYPER_KEY: u32 = 0x78; // F9（注册表 CapsLock → F9）
const VERSION: &str = "v0.1.0";
const GITHUB_URL: &str = "https://github.com/xfee/mkemacs";
const RELEASES_URL: &str = "https://github.com/xfee/mkemacs/releases";
const SHARPKEYS_URL: &str = "https://sharpkeys.net/";
// ────────────────────────────────────────────────────────────

// 快捷键说明（lookup() 和托盘菜单共用）
const MAPPINGS: &[(&str, &str)] = &[
    ("F9 + A", "Home  (行首)"),
    ("F9 + E", "End   (行尾)"),
    ("F9 + B", "Left  (左移)"),
    ("F9 + F", "Right (右移)"),
    ("F9 + P", "Up    (上移)"),
    ("F9 + N", "Down  (下移)"),
    ("F9 + D", "Delete (删除后)"),
    ("F9 + H", "Backspace (删除前)"),
    ("F9 + K", "Shift+End, Delete (删至行尾)"),
];

// 防止递归触发 Hook 的魔数
const MAGIC_EXTRA: usize = 0x454D4143534B4559; // "EMACSKEY"

// 窗口消息 ID
const WM_TRAYICON: u32 = WM_USER + 1;
const IDM_ENABLE: usize = 1001;
const IDM_UPDATE: usize = 1002;
const IDM_HOMEPAGE: usize = 1003;
const IDM_SHARPKEYS: usize = 1004;
const IDM_EXIT: usize = 1005;

// 全局状态
static HYPER_DOWN: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(true);
static CONSUMED_VK: AtomicU16 = AtomicU16::new(0);
static mut HOOK: *mut c_void = std::ptr::null_mut();
static mut TRAY_HWND: HWND = HWND(std::ptr::null_mut());
static SENDER: Mutex<Option<Sender<Vec<INPUT>>>> = Mutex::new(None);

// ── 键位映射表 ──────────────────────────────────────────────

fn lookup(vk: u32) -> Option<Vec<INPUT>> {
    match vk {
        0x41 => Some(single_key(VK_HOME)),     // F9+A → Home
        0x45 => Some(single_key(VK_END)),      // F9+E → End
        0x42 => Some(single_key(VK_LEFT)),     // F9+B → Left
        0x46 => Some(single_key(VK_RIGHT)),    // F9+F → Right
        0x50 => Some(single_key(VK_UP)),       // F9+P → Up
        0x4E => Some(single_key(VK_DOWN)),     // F9+N → Down
        0x44 => Some(single_key(VK_DELETE)),   // F9+D → Delete
        0x48 => Some(single_key(VK_BACK)),     // F9+H → Backspace
        0x4B => Some(kill_line()),             // F9+K → Shift+End, Delete
        _ => None,
    }
}

fn call_next_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { CallNextHookEx(HHOOK(HOOK), code, wparam, lparam) }
}

// ── INPUT 构造器 ────────────────────────────────────────────

fn single_key(vk: VIRTUAL_KEY) -> Vec<INPUT> {
    vec![key_down(vk), key_up(vk)]
}

fn kill_line() -> Vec<INPUT> {
    vec![
        key_down(VK_SHIFT),
        key_down(VK_END),
        key_up(VK_END),
        key_up(VK_SHIFT),
        key_down(VK_DELETE),
        key_up(VK_DELETE),
    ]
}

fn key_down(vk: VIRTUAL_KEY) -> INPUT {
    make_input(vk, KEYBD_EVENT_FLAGS(0))
}

fn key_up(vk: VIRTUAL_KEY) -> INPUT {
    make_input(vk, KEYEVENTF_KEYUP)
}

fn make_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC_EXTRA,
            },
        },
    }
}

// ── 键盘钩子回调 ─────────────────────────────────────────────

extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return call_next_hook(code, wparam, lparam);
    }

    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };

    // 自己发出的模拟按键，直接放行
    if info.dwExtraInfo == MAGIC_EXTRA {
        return call_next_hook(code, wparam, lparam);
    }

    // 功能被禁用时，全部放行
    if !ENABLED.load(Ordering::Relaxed) {
        return call_next_hook(code, wparam, lparam);
    }

    let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
    let vk = info.vkCode;

    // ── Hyper 键按下/抬起 ──
    if vk == HYPER_KEY {
        HYPER_DOWN.store(is_down, Ordering::SeqCst);
        if !is_down {
            CONSUMED_VK.store(0, Ordering::SeqCst);
        }
        return LRESULT(1); // 吃掉 F9
    }

    // ── 键抬起 ──
    if !is_down {
        // 检查是不是之前被消费的键的抬起事件
        let consumed = CONSUMED_VK.swap(0, Ordering::SeqCst);
        if consumed != 0 && consumed as u32 == vk {
            return LRESULT(1);
        }
        // Hyper 状态下，其他键的抬起也吃掉防止泄漏
        if HYPER_DOWN.load(Ordering::SeqCst) {
            return LRESULT(1);
        }
        return call_next_hook(code, wparam, lparam);
    }

    // ── 键按下 & Hyper 按下 ──
    if HYPER_DOWN.load(Ordering::SeqCst) {
        if let Some(actions) = lookup(vk) {
            CONSUMED_VK.store(vk as u16, Ordering::SeqCst);
            if let Ok(s) = SENDER.lock() {
                if let Some(ref tx) = *s {
                    let _ = tx.send(actions);
                }
            }
            return LRESULT(1);
        }
    }

    call_next_hook(code, wparam, lparam)
}

// ── SendInput 发送线程 ───────────────────────────────────────

fn spawn_sender() -> Sender<Vec<INPUT>> {
    let (tx, rx) = mpsc::channel::<Vec<INPUT>>();
    thread::spawn(move || {
        for actions in rx {
            let mods: [(VIRTUAL_KEY, KEYBD_EVENT_FLAGS); 8] = [
                (VK_LCONTROL, KEYBD_EVENT_FLAGS(0)),
                (VK_RCONTROL, KEYEVENTF_EXTENDEDKEY),
                (VK_LSHIFT, KEYBD_EVENT_FLAGS(0)),
                (VK_RSHIFT, KEYBD_EVENT_FLAGS(0)),
                (VK_LMENU, KEYBD_EVENT_FLAGS(0)),
                (VK_RMENU, KEYEVENTF_EXTENDEDKEY),
                (VK_LWIN, KEYBD_EVENT_FLAGS(0)),
                (VK_RWIN, KEYEVENTF_EXTENDEDKEY),
            ];
            let mut all: Vec<INPUT> = Vec::with_capacity(mods.len() + actions.len());
            for (vk, flag) in &mods {
                if unsafe { GetAsyncKeyState(vk.0 as i32) } < 0 {
                    all.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: *vk,
                                wScan: 0,
                                dwFlags: KEYEVENTF_KEYUP | *flag,
                                time: 0,
                                dwExtraInfo: MAGIC_EXTRA,
                            },
                        },
                    });
                }
            }
            all.extend(actions);
            unsafe { SendInput(&all, mem::size_of::<INPUT>() as i32) };
        }
    });
    tx
}

// ── 系统托盘 ──────────────────────────────────────────────────

fn setup_tray(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() };
    let tip: Vec<u16> = "mkemacs\0".encode_utf16().collect();
    let len = tip.len().min(128);
    nid.szTip[..len].copy_from_slice(&tip[..len]);

    let _ = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
}

fn show_balloon(title: &str, msg: &str) {
    let hwnd = unsafe { TRAY_HWND };
    if hwnd.0.is_null() {
        return;
    }
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;

    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let msg_wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let tlen = title_wide.len().min(64);
    let mlen = msg_wide.len().min(256);
    nid.szInfoTitle[..tlen].copy_from_slice(&title_wide[..tlen]);
    nid.szInfo[..mlen].copy_from_slice(&msg_wide[..mlen]);

    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu().unwrap_or_default();

        // 标题
        let title: Vec<u16> = "mkemacs\0".encode_utf16().collect();
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(title.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // "使用说明" 二级菜单
        let help_menu = CreatePopupMenu().unwrap_or_default();
        let hint: Vec<u16> = "需要 SharpKeys 将 CapsLock 映射为 F9\0"
            .encode_utf16()
            .collect();
        let _ = AppendMenuW(help_menu, MF_STRING | MF_GRAYED, 0, PCWSTR(hint.as_ptr()));
        let _ = AppendMenuW(help_menu, MF_SEPARATOR, 0, PCWSTR::null());
        for (key, desc) in MAPPINGS {
            let item: Vec<u16> = format!("{key}  →  {desc}\0").encode_utf16().collect();
            let _ = AppendMenuW(help_menu, MF_STRING | MF_GRAYED, 0, PCWSTR(item.as_ptr()));
        }
        let help_label: Vec<u16> = "使用说明\0".encode_utf16().collect();
        let _ = AppendMenuW(
            menu,
            MF_POPUP | MF_STRING,
            help_menu.0 as usize,
            PCWSTR(help_label.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // 禁用 / 开启
        let label: Vec<u16> = if ENABLED.load(Ordering::Relaxed) {
            "禁用\0".encode_utf16().collect()
        } else {
            "开启\0".encode_utf16().collect()
        };
        let _ = AppendMenuW(menu, MF_STRING, IDM_ENABLE, PCWSTR(label.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // 检查更新（版本号用 tab 右对齐）
        let update_label: Vec<u16> = format!("检查更新\t{VERSION}\0")
            .encode_utf16()
            .collect();
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            IDM_UPDATE,
            PCWSTR(update_label.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // 项目主页
        let homepage: Vec<u16> = "项目主页\0".encode_utf16().collect();
        let _ = AppendMenuW(menu, MF_STRING, IDM_HOMEPAGE, PCWSTR(homepage.as_ptr()));

        // SharpKeys 主页
        let sharpkeys: Vec<u16> = "SharpKeys 主页\0".encode_utf16().collect();
        let _ = AppendMenuW(menu, MF_STRING, IDM_SHARPKEYS, PCWSTR(sharpkeys.as_ptr()));

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // 退出
        let exit_text: Vec<u16> = "退出\0".encode_utf16().collect();
        let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, PCWSTR(exit_text.as_ptr()));

        let _ = SetForegroundWindow(hwnd);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = TrackPopupMenu(menu, TPM_BOTTOMALIGN | TPM_LEFTALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

// ── 启用/禁用切换 ────────────────────────────────────────────

fn toggle_enabled() {
    let was = ENABLED.fetch_xor(true, Ordering::SeqCst);
    let now = !was;
    let hwnd = unsafe { TRAY_HWND };

    // 更新托盘提示文字
    let tip_str = if now { "mkemacs\0" } else { "mkemacs (Disabled)\0" };
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_TIP;
    let tip: Vec<u16> = tip_str.encode_utf16().collect();
    let len = tip.len().min(128);
    nid.szTip[..len].copy_from_slice(&tip[..len]);
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };

    // 通知
    if now {
        show_balloon("mkemacs", "mkemacs 快捷键已启用");
    } else {
        show_balloon("mkemacs", "mkemacs 快捷键已停用");
    }
}

// ── 隐藏窗口过程 ──────────────────────────────────────────────

fn open_url(url: &str) {
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = windows::Win32::UI::Shell::ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SHOW_WINDOW_CMD(1), // SW_SHOWNORMAL
        );
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            if lparam.0 == WM_RBUTTONUP as isize {
                show_tray_menu(hwnd);
            } else if lparam.0 == WM_LBUTTONUP as isize {
                toggle_enabled();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if wparam.0 == IDM_ENABLE {
                toggle_enabled();
            } else if wparam.0 == IDM_UPDATE {
                open_url(RELEASES_URL);
            } else if wparam.0 == IDM_HOMEPAGE {
                open_url(GITHUB_URL);
            } else if wparam.0 == IDM_SHARPKEYS {
                open_url(SHARPKEYS_URL);
            } else if wparam.0 == IDM_EXIT {
                cleanup_tray(hwnd);
                unsafe { PostQuitMessage(0) };
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn cleanup_tray(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

// ── 入口 ─────────────────────────────────────────────────────

fn main() {
    let h_instance = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap() };
    let h_instance: HINSTANCE = HINSTANCE(h_instance.0);

    // 注册窗口类
    let class_name = HSTRING::from("mkemacsTrayClass");
    let wc = WNDCLASSEXW {
        cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: h_instance,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..unsafe { mem::zeroed() }
    };
    unsafe { RegisterClassExW(&wc) };

    // 创建隐藏消息窗口
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            h_instance,
            None,
        )
    }
    .unwrap_or_else(|e| panic!("Failed to create window: {e:?}"));

    // 托盘图标
    setup_tray(hwnd);
    unsafe { TRAY_HWND = hwnd };

    // 启动 SendInput 线程
    *SENDER.lock().unwrap() = Some(spawn_sender());

    // 安装键盘钩子
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), h_instance, 0) }
        .unwrap_or_else(|e| panic!("Failed to set hook: {e:?}"));
    unsafe { HOOK = hook.0 };

    // 启动通知
    show_balloon("mkemacs", "mkemacs 快捷键已启用");

    // 消息循环
    let mut msg: MSG = unsafe { mem::zeroed() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // 清理
    unsafe {
        if !HOOK.is_null() {
            let _ = UnhookWindowsHookEx(HHOOK(HOOK));
            HOOK = std::ptr::null_mut();
        }
    }
}
