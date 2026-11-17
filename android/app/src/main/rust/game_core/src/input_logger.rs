//! Pure Rust Input & Control Diagnostics Logger for NFSU2 Android.
//!
//! Provides high-fidelity logging of all input subsystems:
//! 1. External Gamepad & Controller discovery, capabilities, and axis configurations.
//! 2. Multi-touch screen interaction (coordinates, pointer IDs, and mapped HUD touch controls).
//! 3. Analog stick deflection (Left Stick, Right Stick, Triggers).
//! 4. Digital button presses, D-pad events, and DirectInput scancode translations.
//! 5. Dynamic Camera Orbit / Look rotations driven by touch drag and right analog stick.
//!
//! Output destinations:
//! - `/sdcard/nfsu2/input_log.txt`
//! - `/sdcard/NFSU2/input_log.txt`
//! - App internal storage fallback

use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jfloat, jint};
use jni::JNIEnv;

const DEFAULT_SDCARD_PATHS: &[&str] = &[
    "/sdcard/nfsu2/input_log.txt",
    "/sdcard/NFSU2/input_log.txt",
    "/storage/emulated/0/nfsu2/input_log.txt",
    "/storage/emulated/0/NFSU2/input_log.txt",
];

const LOG_HEADER: &str = "\
================================================================================
NFSU2 Android Input & Control Diagnostics Log
Engine: Pure Rust Input Logger (game_core::input_logger)
================================================================================
";

pub struct InputLogger {
    file: Mutex<Option<File>>,
    custom_path: Mutex<Option<PathBuf>>,
    initialized: AtomicBool,
    last_axis_log_time: Mutex<Option<Instant>>,
    last_touch_log_time: Mutex<Option<Instant>>,
    last_camera_log_time: Mutex<Option<Instant>>,
    last_rx: Mutex<f32>,
    last_ry: Mutex<f32>,
    event_counter: AtomicU64,
}

impl InputLogger {
    pub const fn new() -> Self {
        Self {
            file: Mutex::new(None),
            custom_path: Mutex::new(None),
            initialized: AtomicBool::new(false),
            last_axis_log_time: Mutex::new(None),
            last_touch_log_time: Mutex::new(None),
            last_camera_log_time: Mutex::new(None),
            last_rx: Mutex::new(0.0),
            last_ry: Mutex::new(0.0),
            event_counter: AtomicU64::new(0),
        }
    }

    fn get_timestamp() -> String {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe {
            libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&ts.tv_sec, &mut tm);
            let millis = (ts.tv_nsec / 1_000_000) as u32;
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec,
                millis
            )
        }
    }

    /// Initializes logger with optional custom base path (e.g. app filesDir).
    pub fn init(&self, custom_base: Option<&str>) {
        let mut target_file = None;

        if let Some(base) = custom_base {
            let p = Path::new(base).join("input_log.txt");
            if let Ok(mut g) = self.custom_path.lock() {
                *g = Some(p.clone());
            }
            if let Ok(f) = OpenOptions::new().create(true).write(true).truncate(true).open(&p) {
                target_file = Some(f);
            }
        }

        if target_file.is_none() {
            for &candidate in DEFAULT_SDCARD_PATHS {
                let p = Path::new(candidate);
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(f) = OpenOptions::new().create(true).write(true).truncate(true).open(p) {
                    target_file = Some(f);
                    break;
                }
            }
        }

        if let Some(mut f) = target_file {
            let _ = f.write_all(LOG_HEADER.as_bytes());
            let init_msg = format!(
                "[{}] [INIT] Input logger initialized successfully (PID {})\n",
                Self::get_timestamp(),
                unsafe { libc::getpid() }
            );
            let _ = f.write_all(init_msg.as_bytes());
            let _ = f.flush();

            if let Ok(mut guard) = self.file.lock() {
                *guard = Some(f);
            }
            self.initialized.store(true, Ordering::Release);
        }
    }

    /// Writes a line to the log file.
    fn write_line(&self, line: &str) {
        if !self.initialized.load(Ordering::Acquire) {
            self.init(None);
        }

        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.write_all(line.as_bytes());
                let _ = f.write_all(b"\n");
                let count = self.event_counter.fetch_add(1, Ordering::Relaxed);
                if count % 10 == 0 {
                    let _ = f.flush();
                }
            }
        }
    }

    /// Logs an external input device (gamepad, keyboard, joystick).
    pub fn log_device(&self, id: i32, name: &str, sources: u32, is_external: bool, desc: &str) {
        let ts = Self::get_timestamp();
        let is_gamepad = (sources & 0x401) == 0x401; // SOURCE_GAMEPAD
        let is_joystick = (sources & 0x1000010) == 0x1000010; // SOURCE_JOYSTICK
        let is_keyboard = (sources & 0x101) == 0x101; // SOURCE_KEYBOARD

        let type_desc = match (is_gamepad, is_joystick, is_keyboard) {
            (true, true, _) => "Gamepad + Joystick",
            (true, false, _) => "Gamepad",
            (false, true, _) => "Joystick / Controller",
            (false, false, true) => "External Keyboard",
            _ => "Generic Input Device",
        };

        let line = format!(
            "[{}] [DEVICE_DETECTED] ID: {} | Name: \"{}\" | Type: {} | External: {} | Sources: 0x{:08X} | Details: {}",
            ts, id, name, type_desc, is_external, sources, desc
        );
        self.write_line(&line);
    }

    /// Logs a touchscreen interaction.
    pub fn log_touch(&self, pointer_id: i32, action: i32, x: f32, y: f32, control_name: &str) {
        let action_str = match action {
            0 => "ACTION_DOWN",
            1 => "ACTION_UP",
            2 => "ACTION_MOVE",
            3 => "ACTION_CANCEL",
            5 => "ACTION_POINTER_DOWN",
            6 => "ACTION_POINTER_UP",
            _ => "ACTION_OTHER",
        };

        // Rate limit continuous move events to 80ms unless it's a DOWN or UP event
        if action == 2 {
            let now = Instant::now();
            if let Ok(mut guard) = self.last_touch_log_time.lock() {
                if let Some(prev) = *guard {
                    if now.duration_since(prev).as_millis() < 80 {
                        return;
                    }
                }
                *guard = Some(now);
            }
        }

        let ts = Self::get_timestamp();
        let line = format!(
            "[{}] [TOUCH] Pointer #{} {} at ({:.3}, {:.3}) -> Control: \"{}\"",
            ts, pointer_id, action_str, x, y, control_name
        );
        self.write_line(&line);
    }

    /// Logs gamepad analog stick and trigger states.
    pub fn log_gamepad_axes(
        &self,
        id: i32,
        lx: f32,
        ly: f32,
        rx: f32,
        ry: f32,
        lt: f32,
        rt: f32,
    ) {
        let now = Instant::now();
        let mut changed_significantly = false;

        if let (Ok(mut l_rx), Ok(mut l_ry)) = (self.last_rx.lock(), self.last_ry.lock()) {
            if (rx - *l_rx).abs() > 0.15 || (ry - *l_ry).abs() > 0.15 {
                changed_significantly = true;
                *l_rx = rx;
                *l_ry = ry;
            }
        }

        if !changed_significantly {
            if let Ok(mut guard) = self.last_axis_log_time.lock() {
                if let Some(prev) = *guard {
                    if now.duration_since(prev).as_millis() < 120 {
                        return;
                    }
                }
                *guard = Some(now);
            }
        }

        let ts = Self::get_timestamp();
        let line = format!(
            "[{}] [GAMEPAD_AXIS] ID: {} | LeftStick: ({:+.3}, {:+.3}) | RightStick: ({:+.3}, {:+.3}) | Triggers: LT={:.2}, RT={:.2}",
            ts, id, lx, ly, rx, ry, lt, rt
        );
        self.write_line(&line);
    }

    /// Logs gamepad digital button actions.
    pub fn log_gamepad_button(&self, id: i32, button_name: &str, pressed: bool, scan: u32) {
        let ts = Self::get_timestamp();
        let state = if pressed { "PRESSED" } else { "RELEASED" };
        let line = format!(
            "[{}] [GAMEPAD_BTN] ID: {} | Button: {:<12} {:<8} -> Scancode: 0x{:02X}",
            ts, id, button_name, state, scan
        );
        self.write_line(&line);
    }

    /// Logs camera look / rotation actions.
    pub fn log_camera_look(&self, source: &str, x: f32, y: f32, yaw_rad: f32, is_rear: bool) {
        let now = Instant::now();
        if let Ok(mut guard) = self.last_camera_log_time.lock() {
            if let Some(prev) = *guard {
                if now.duration_since(prev).as_millis() < 100 {
                    return;
                }
            }
            *guard = Some(now);
        }

        let ts = Self::get_timestamp();
        let yaw_deg = yaw_rad * 180.0 / std::f32::consts::PI;
        let mode = if is_rear { "REAR_SNAP" } else { "ORBIT" };
        let line = format!(
            "[{}] [CAMERA_LOOK] Source: {:<10} | Input: ({:+.3}, {:+.3}) | TargetYaw: {:+6.1}° | Mode: {}",
            ts, source, x, y, yaw_deg, mode
        );
        self.write_line(&line);
    }

    /// Flushes log buffer to disk.
    pub fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.flush();
            }
        }
    }
}

pub static GLOBAL_INPUT_LOGGER: InputLogger = InputLogger::new();

// ---------------------------------------------------------------------------
// C ABI Export Functions
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn nfs_input_log_init(base_path: *const c_char) {
    let custom = if !base_path.is_null() {
        unsafe { CStr::from_ptr(base_path).to_str().ok() }
    } else {
        None
    };
    GLOBAL_INPUT_LOGGER.init(custom);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_device(
    id: i32,
    name: *const c_char,
    sources: u32,
    is_external: bool,
    desc: *const c_char,
) {
    let s_name = if !name.is_null() {
        unsafe { CStr::from_ptr(name).to_str().unwrap_or("Unknown") }
    } else {
        "Unknown"
    };
    let s_desc = if !desc.is_null() {
        unsafe { CStr::from_ptr(desc).to_str().unwrap_or("") }
    } else {
        ""
    };
    GLOBAL_INPUT_LOGGER.log_device(id, s_name, sources, is_external, s_desc);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_touch(
    pointer_id: i32,
    action: i32,
    x: f32,
    y: f32,
    control_name: *const c_char,
) {
    let cname = if !control_name.is_null() {
        unsafe { CStr::from_ptr(control_name).to_str().unwrap_or("Touch") }
    } else {
        "Touch"
    };
    GLOBAL_INPUT_LOGGER.log_touch(pointer_id, action, x, y, cname);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_gamepad_axes(
    id: i32,
    lx: f32,
    ly: f32,
    rx: f32,
    ry: f32,
    lt: f32,
    rt: f32,
) {
    GLOBAL_INPUT_LOGGER.log_gamepad_axes(id, lx, ly, rx, ry, lt, rt);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_gamepad_button(
    id: i32,
    button_name: *const c_char,
    pressed: bool,
    scan: u32,
) {
    let bname = if !button_name.is_null() {
        unsafe { CStr::from_ptr(button_name).to_str().unwrap_or("Button") }
    } else {
        "Button"
    };
    GLOBAL_INPUT_LOGGER.log_gamepad_button(id, bname, pressed, scan);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_camera(
    source: *const c_char,
    x: f32,
    y: f32,
    yaw: f32,
    is_rear: bool,
) {
    let src = if !source.is_null() {
        unsafe { CStr::from_ptr(source).to_str().unwrap_or("Unknown") }
    } else {
        "Unknown"
    };
    GLOBAL_INPUT_LOGGER.log_camera_look(src, x, y, yaw, is_rear);
}

#[no_mangle]
pub extern "C" fn nfs_input_log_flush() {
    GLOBAL_INPUT_LOGGER.flush();
}

// ---------------------------------------------------------------------------
// JNI Export Functions
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn Java_io_mininxd_nfsu2_GameActivity_nativeInputLogInit(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) {
    if let Ok(s) = env.get_string(&path) {
        let rust_str: String = s.into();
        GLOBAL_INPUT_LOGGER.init(Some(&rust_str));
    } else {
        GLOBAL_INPUT_LOGGER.init(None);
    }
}

#[no_mangle]
pub extern "system" fn Java_io_mininxd_nfsu2_GameActivity_nativeInputLogDevice(
    mut env: JNIEnv,
    _class: JClass,
    id: jint,
    name: JString,
    sources: jint,
    is_external: jboolean,
    desc: JString,
) {
    let name_str = env.get_string(&name).map(|s| s.into()).unwrap_or_else(|_| "Unknown".to_string());
    let desc_str = env.get_string(&desc).map(|s| s.into()).unwrap_or_else(|_| "".to_string());
    GLOBAL_INPUT_LOGGER.log_device(id, &name_str, sources as u32, is_external != 0, &desc_str);
}

#[no_mangle]
pub extern "system" fn Java_io_mininxd_nfsu2_GameActivity_nativeInputLogTouch(
    mut env: JNIEnv,
    _class: JClass,
    pointer_id: jint,
    action: jint,
    x: jfloat,
    y: jfloat,
    control_name: JString,
) {
    let cname = env.get_string(&control_name).map(|s| s.into()).unwrap_or_else(|_| "Touch".to_string());
    GLOBAL_INPUT_LOGGER.log_touch(pointer_id, action, x, y, &cname);
}
