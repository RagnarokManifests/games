// SAM-style (Steam Achievement Manager) native helper — low-level rewrite.
//
// The previous version of this helper used the FLAT steam_api(64).dll C API
// (SteamAPI_Init + named exports), loading the *target game's own* DLL. That
// approach was found to intermittently trigger Steam to actually LAUNCH the
// real game as a side effect of SteamAPI_Init() re-validating ownership for
// games unlocked via Ragnarok's ownership-flag method (not real Steamworks
// ownership) — disruptive, and still unreliable after several rounds of
// tuning (timeouts, retries, serialization).
//
// This rewrite follows exactly what github.com/gibbed/SteamAchievementManager
// (SAM) does, which has never been reported to auto-launch games in years of
// widespread use: load steamclient(64).dll — Steam's OWN client library,
// always present in the Steam install folder itself, completely independent
// of any specific game's bundled DLL — and talk to the ALREADY-RUNNING Steam
// client directly through its low-level C++ interfaces (ISteamClient,
// ISteamUser, ISteamUserStats), bypassing the high-level SteamAPI_Init()
// gateway (and whatever ownership-relaunch logic lives inside it) entirely.
//
// This requires calling raw C++ virtual methods via their vtable slot index
// — there is no named/versioned export safety net here like the flat API
// had. Slot indices and interface version strings below are copied directly
// from SAM's own (community-verified, MIT-licensed, actively maintained)
// source — see SAM.API/Interfaces/*.cs and SAM.API/Wrappers/*.cs.
//
// Usage:
//   ach_helper.exe <steam_path> <app_id> set    <achievement_name>
//   ach_helper.exe <steam_path> <app_id> clear  <achievement_name>
//   ach_helper.exe <steam_path> <app_id> get    <name1,name2,...>
//   ach_helper.exe <steam_path> <app_id> ticket <ignored>
//
// `ticket` extracts a real Encrypted App Ticket (ETicket) for <app_id> from
// the currently logged-in Steam account — see OpenSteamTool's own
// setAppTicket/setETicket docs (github.com/OpenSteam001/OpenSteamTool#usage).
// Only succeeds if that account genuinely owns the game; Steam returns
// EResult 15 (AccessDenied) otherwise. Uses ISteamUser021, not the
// ISteamUser012 the achievement actions below use — RequestEncryptedAppTicket
// / GetEncryptedAppTicket don't exist on 012 (confirmed against the actual
// versioned interface headers, not guessed). Slot indices and the
// EncryptedAppTicketResponse_t callback id (k_iSteamUserCallbacks(100) + 54)
// are from the real Steamworks SDK headers, not SAM (which only wraps
// achievement-related interfaces).

use std::env;
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int};
use std::process::exit;
use std::thread::sleep;
use std::time::{Duration, Instant};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const c_char) -> *mut c_void;
    fn FreeLibrary(hModule: *mut c_void) -> c_int;
    fn GetLastError() -> u32;
    fn SetDllDirectoryW(lpPathName: *const u16) -> c_int;
}

// Flat (cdecl) exports on steamclient(64).dll itself — these are plain C
// functions, not C++ methods, so no vtable/thiscall involved.
type FnCreateInterface = unsafe extern "C" fn(*const c_char, *mut i32) -> *mut c_void;
type FnGetCallback = unsafe extern "C" fn(i32, *mut CallbackMsg, *mut i32) -> u8;
type FnFreeLastCallback = unsafe extern "C" fn(i32) -> u8;

// Matches SAM.API/Types/CallbackMessage.cs: [StructLayout(Sequential, Pack=1)]
// { int User; int Id; IntPtr ParamPointer; int ParamSize; } — must be
// byte-for-byte identical since steamclient(64).dll writes into it directly.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct CallbackMsg {
    user: i32,
    id: i32,
    param: usize,
    param_size: i32,
}

const CALLBACK_USER_STATS_RECEIVED: i32 = 1101;
const CALLBACK_ENCRYPTED_APP_TICKET_RESPONSE: i32 = 154; // k_iSteamUserCallbacks(100) + 54
const K_ERESULT_OK: i32 = 1;

// C++ virtual (thiscall) methods, reached via vtable slot lookup below.
// `this` is passed as the first argument on the Rust side (mirroring the
// hidden `this` a real C++ call passes in ECX on x86). Rust's "thiscall" ABI
// keyword is only valid on x86 (a hard compile error elsewhere, unlike e.g.
// "cdecl" which just silently degrades) — on x86_64 Windows there is only
// one calling convention for everything, `this` included, so plain "C" is
// the correct (and only legal) choice there.
macro_rules! member_fn {
    ($name:ident, fn($($arg:ty),*) -> $ret:ty) => {
        #[cfg(target_arch = "x86")]
        type $name = unsafe extern "thiscall" fn($($arg),*) -> $ret;
        #[cfg(target_arch = "x86_64")]
        type $name = unsafe extern "C" fn($($arg),*) -> $ret;
    };
}

member_fn!(FnCreateSteamPipe, fn(*mut c_void) -> i32);
member_fn!(FnReleaseSteamPipe, fn(*mut c_void, i32) -> u8);
member_fn!(FnConnectToGlobalUser, fn(*mut c_void, i32) -> i32);
member_fn!(FnReleaseUser, fn(*mut c_void, i32, i32) -> ());
member_fn!(FnGetInterfaceFromClient, fn(*mut c_void, i32, i32, *const c_char) -> *mut c_void);
member_fn!(FnGetSteamId, fn(*mut c_void, *mut u64) -> ());
member_fn!(FnGetAchievement, fn(*mut c_void, *const c_char, *mut u8) -> u8);
member_fn!(FnSetOrClearAchievement, fn(*mut c_void, *const c_char) -> u8);
member_fn!(FnGetAchievementAndUnlockTime, fn(*mut c_void, *const c_char, *mut u8, *mut u32) -> u8);
member_fn!(FnStoreStats, fn(*mut c_void) -> u8);
member_fn!(FnRequestUserStats, fn(*mut c_void, u64) -> u64);
member_fn!(FnRequestEncryptedAppTicket, fn(*mut c_void, *const c_void, i32) -> u64);
member_fn!(FnGetEncryptedAppTicket, fn(*mut c_void, *mut u8, i32, *mut u32) -> u8);

// ISteamClient018 vtable slot indices (SAM.API/Interfaces/ISteamClient018.cs)
const SLOT_CREATE_STEAM_PIPE: usize = 0;
const SLOT_RELEASE_STEAM_PIPE: usize = 1;
const SLOT_CONNECT_TO_GLOBAL_USER: usize = 2;
const SLOT_RELEASE_USER: usize = 4;
const SLOT_GET_ISTEAM_USER: usize = 5;
const SLOT_GET_ISTEAM_USER_STATS: usize = 13;

// ISteamUser012 (SAM.API/Interfaces/ISteamUser012.cs)
const SLOT_GET_STEAM_ID: usize = 2;

// ISteamUser021 — verified against github.com/GloriousEggroll/lsteamclient's
// cppISteamUser_SteamUser021.h (a real Steamworks SDK header mirror), not
// SAM (whose achievement-focused wrapper doesn't cover these methods at all).
const SLOT_REQUEST_ENCRYPTED_APP_TICKET: usize = 20;
const SLOT_GET_ENCRYPTED_APP_TICKET: usize = 21;

// ISteamUserStats013 (SAM.API/Interfaces/ISteamUserStats013.cs)
const SLOT_GET_ACHIEVEMENT: usize = 5;
const SLOT_SET_ACHIEVEMENT: usize = 6;
const SLOT_CLEAR_ACHIEVEMENT: usize = 7;
const SLOT_GET_ACHIEVEMENT_AND_UNLOCK_TIME: usize = 8;
const SLOT_STORE_STATS: usize = 9;
const SLOT_REQUEST_USER_STATS: usize = 15;

unsafe fn vtable_fn(obj: *mut c_void, index: usize) -> *mut c_void {
    let vtable_ptr = *(obj as *const usize) as *const usize;
    *(vtable_ptr.add(index)) as *mut c_void
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_proc(module: *mut c_void, name: &str) -> Option<*mut c_void> {
    let cname = CString::new(name).ok()?;
    let addr = unsafe { GetProcAddress(module, cname.as_ptr()) };
    if addr.is_null() {
        None
    } else {
        Some(addr)
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("ERROR: {}", msg);
    exit(1);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 5 {
        eprintln!("USAGE: ach_helper <steam_path> <app_id> <set|clear|get|ticket> <payload>");
        exit(2);
    }
    let steam_path = &args[1];
    let app_id = &args[2];
    // Parsed once for the callback filter further down. A non-numeric AppID
    // yields 0, which matches nothing — the safe direction.
    let app_id_num: u32 = app_id.parse().unwrap_or(0);
    let action = args[3].as_str();
    let payload = &args[4];
    let debug = env::var("ACH_HELPER_DEBUG").is_ok();

    env::set_var("SteamAppId", app_id);
    env::set_var("SteamGameId", app_id);

    #[cfg(target_arch = "x86_64")]
    let dll_name = "steamclient64.dll";
    #[cfg(target_arch = "x86")]
    let dll_name = "steamclient.dll";

    let dll_path = format!("{}\\{}", steam_path.trim_end_matches('\\'), dll_name);

    // steamclient(64).dll pulls in sibling DLLs from the same folder — match
    // SAM's Steam.Load(), which prepends the Steam install dir (and its
    // "bin" subfolder) to the DLL search path before loading it.
    let search_dir = to_wide(steam_path);
    unsafe { SetDllDirectoryW(search_dir.as_ptr()) };

    let module = unsafe { LoadLibraryW(to_wide(&dll_path).as_ptr()) };
    if module.is_null() {
        fail(&format!(
            "no se pudo cargar {} (código de Windows {})",
            dll_path,
            unsafe { GetLastError() }
        ));
    }

    let create_interface_addr = match get_proc(module, "CreateInterface") {
        Some(a) => a,
        None => fail("CreateInterface no encontrado en steamclient(64).dll"),
    };
    let create_interface: FnCreateInterface = unsafe { std::mem::transmute(create_interface_addr) };

    let get_callback_addr = get_proc(module, "Steam_BGetCallback");
    let free_callback_addr = get_proc(module, "Steam_FreeLastCallback");

    macro_rules! create_iface {
        ($version:expr) => {{
            let cver = CString::new($version).unwrap();
            let mut ret_code: i32 = 0;
            let ptr = unsafe { create_interface(cver.as_ptr(), &mut ret_code) };
            if debug {
                eprintln!("DEBUG: CreateInterface('{}') -> {:?} (ret_code={})", $version, ptr, ret_code);
            }
            ptr
        }};
    }

    let client = create_iface!("SteamClient018");
    if client.is_null() {
        fail("CreateInterface('SteamClient018') devolvió null — ¿Steam no está abierto?");
    }

    let pipe = unsafe {
        let f: FnCreateSteamPipe = std::mem::transmute(vtable_fn(client, SLOT_CREATE_STEAM_PIPE));
        f(client)
    };
    if debug {
        eprintln!("DEBUG: CreateSteamPipe() -> {}", pipe);
    }
    if pipe == 0 {
        fail("CreateSteamPipe() falló.");
    }

    let user = unsafe {
        let f: FnConnectToGlobalUser = std::mem::transmute(vtable_fn(client, SLOT_CONNECT_TO_GLOBAL_USER));
        f(client, pipe)
    };
    if debug {
        eprintln!("DEBUG: ConnectToGlobalUser({}) -> {}", pipe, user);
    }
    if user == 0 {
        fail("ConnectToGlobalUser() falló (¿no hay una sesión de Steam activa?).");
    }

    let cleanup = |success: bool| -> ! {
        unsafe {
            let f: FnReleaseUser = std::mem::transmute(vtable_fn(client, SLOT_RELEASE_USER));
            f(client, pipe, user);
            let f2: FnReleaseSteamPipe = std::mem::transmute(vtable_fn(client, SLOT_RELEASE_STEAM_PIPE));
            f2(client, pipe);
            FreeLibrary(module);
        }
        exit(if success { 0 } else { 1 });
    };

    if action == "ticket" {
        let get_iface_from_client: FnGetInterfaceFromClient =
            unsafe { std::mem::transmute(vtable_fn(client, SLOT_GET_ISTEAM_USER)) };
        let cver_user021 = CString::new("SteamUser021").unwrap();
        let steam_user021 = unsafe { get_iface_from_client(client, user, pipe, cver_user021.as_ptr()) };
        if debug {
            eprintln!("DEBUG: GetISteamUser('SteamUser021') -> {:?}", steam_user021);
        }
        if steam_user021.is_null() {
            eprintln!("ERROR: No se pudo obtener ISteamUser021 (¿versión de Steam muy antigua?).");
            cleanup(false);
        }

        let call_handle = unsafe {
            let f: FnRequestEncryptedAppTicket =
                std::mem::transmute(vtable_fn(steam_user021, SLOT_REQUEST_ENCRYPTED_APP_TICKET));
            f(steam_user021, std::ptr::null(), 0)
        };
        if debug {
            eprintln!("DEBUG: RequestEncryptedAppTicket() -> {}", call_handle);
        }
        if call_handle == 0 {
            eprintln!("ERROR: RequestEncryptedAppTicket() falló inmediatamente.");
            cleanup(false);
        }

        let mut eresult: i32 = -1;
        if let (Some(get_cb_addr), Some(free_cb_addr)) = (get_callback_addr, free_callback_addr) {
            let get_cb: FnGetCallback = unsafe { std::mem::transmute(get_cb_addr) };
            let free_cb: FnFreeLastCallback = unsafe { std::mem::transmute(free_cb_addr) };
            let deadline = Instant::now() + Duration::from_secs(45);
            let mut received = false;
            while Instant::now() < deadline && !received {
                let mut msg = CallbackMsg { user: 0, id: 0, param: 0, param_size: 0 };
                let mut call: i32 = 0;
                let got = unsafe { get_cb(pipe, &mut msg, &mut call) };
                if got != 0 {
                    let msg_id = msg.id;
                    let msg_param = msg.param;
                    let msg_param_size = msg.param_size;
                    if debug {
                        eprintln!("DEBUG: callback id={} param_size={} received", msg_id, msg_param_size);
                    }
                    if msg_id == CALLBACK_ENCRYPTED_APP_TICKET_RESPONSE && msg_param != 0 {
                        // EncryptedAppTicketResponse_t { EResult m_eResult; } — a single i32.
                        eresult = unsafe { *(msg_param as *const i32) };
                        received = true;
                    }
                    unsafe { free_cb(pipe) };
                } else {
                    sleep(Duration::from_millis(50));
                }
            }
            if debug {
                eprintln!("DEBUG: EncryptedAppTicketResponse observed = {} (eresult={})", received, eresult);
            }
            if !received {
                eprintln!("ERROR: Steam no respondió a tiempo (RequestEncryptedAppTicket).");
                cleanup(false);
            }
        } else {
            eprintln!("ERROR: Steam_BGetCallback/Steam_FreeLastCallback no encontrados en steamclient(64).dll.");
            cleanup(false);
        }

        if eresult != K_ERESULT_OK {
            eprintln!(
                "ERROR: Steam denegó el ticket (EResult={}) — la cuenta activa probablemente NO es dueña real de este AppID.",
                eresult
            );
            cleanup(false);
        }

        let mut buf = vec![0u8; 8192];
        let mut actual_size: u32 = 0;
        let ok = unsafe {
            let f: FnGetEncryptedAppTicket =
                std::mem::transmute(vtable_fn(steam_user021, SLOT_GET_ENCRYPTED_APP_TICKET));
            f(steam_user021, buf.as_mut_ptr(), buf.len() as i32, &mut actual_size)
        };
        if debug {
            eprintln!("DEBUG: GetEncryptedAppTicket() -> ok={} size={}", ok, actual_size);
        }
        if ok == 0 || actual_size == 0 {
            eprintln!("ERROR: GetEncryptedAppTicket() falló tras una respuesta OK — inesperado.");
            cleanup(false);
        }

        let hex: String = buf[..actual_size as usize].iter().map(|b| format!("{:02x}", b)).collect();
        println!("TICKET={}", hex);
        cleanup(true);
    }

    let get_iface_from_client: FnGetInterfaceFromClient =
        unsafe { std::mem::transmute(vtable_fn(client, SLOT_GET_ISTEAM_USER)) };
    let cver_user = CString::new("SteamUser012").unwrap();
    let steam_user = unsafe { get_iface_from_client(client, user, pipe, cver_user.as_ptr()) };
    if debug {
        eprintln!("DEBUG: GetISteamUser -> {:?}", steam_user);
    }
    if steam_user.is_null() {
        fail("No se pudo obtener ISteamUser012.");
    }

    let steam_id: u64 = unsafe {
        let f: FnGetSteamId = std::mem::transmute(vtable_fn(steam_user, SLOT_GET_STEAM_ID));
        let mut id: u64 = 0;
        f(steam_user, &mut id);
        id
    };
    if debug {
        eprintln!("DEBUG: GetSteamID() -> {}", steam_id);
    }

    let get_iface_from_client2: FnGetInterfaceFromClient =
        unsafe { std::mem::transmute(vtable_fn(client, SLOT_GET_ISTEAM_USER_STATS)) };
    let cver_stats = CString::new("STEAMUSERSTATS_INTERFACE_VERSION013").unwrap();
    let stats = unsafe { get_iface_from_client2(client, user, pipe, cver_stats.as_ptr()) };
    if debug {
        eprintln!("DEBUG: GetISteamUserStats -> {:?}", stats);
    }
    if stats.is_null() {
        fail("No se pudo obtener ISteamUserStats013.");
    }

    // RequestUserStats() is async: it queues the request and later delivers
    // a UserStatsReceived_t callback (id 1101) once the data has actually
    // arrived. Poll the raw callback queue for it instead of guessing a
    // fixed sleep — this is what was unreliable in the flat-API version for
    // games with huge stat blobs (PAYDAY 2: 1328 achievements).
    let call_handle = unsafe {
        let f: FnRequestUserStats = std::mem::transmute(vtable_fn(stats, SLOT_REQUEST_USER_STATS));
        f(stats, steam_id)
    };
    if debug {
        eprintln!("DEBUG: RequestUserStats({}) -> {}", steam_id, call_handle);
    }
    if call_handle == 0 {
        fail("RequestUserStats() falló inmediatamente.");
    }

    if let (Some(get_cb_addr), Some(free_cb_addr)) = (get_callback_addr, free_callback_addr) {
        let get_cb: FnGetCallback = unsafe { std::mem::transmute(get_cb_addr) };
        let free_cb: FnFreeLastCallback = unsafe { std::mem::transmute(free_cb_addr) };
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut received = false;
        let mut stats_result: i32 = -1;
        while Instant::now() < deadline {
            let mut msg = CallbackMsg { user: 0, id: 0, param: 0, param_size: 0 };
            let mut call: i32 = 0;
            let got = unsafe { get_cb(pipe, &mut msg, &mut call) };
            if got != 0 {
                let msg_id = msg.id;
                if debug {
                    eprintln!("DEBUG: callback id={} received", msg_id);
                }
                if msg_id == CALLBACK_USER_STATS_RECEIVED {
                    // The queue belongs to the whole Steam client, not to this
                    // request, so the first UserStatsReceived_t to arrive may
                    // well be another app's — likely, since the watcher polls
                    // this while a game is running. Taking it as ours meant
                    // leaving before our own stats loaded and then reporting
                    // every achievement as locked, which in turn made the
                    // emulator bridge fire a full batch write nobody asked
                    // for. m_nGameID sits at offset 0.
                    let mut mine = true;
                    if msg.param != 0 && msg.param_size >= 8 {
                        let cb_app = unsafe {
                            std::ptr::read_unaligned(msg.param as *const u64)
                        };
                        // The low 32 bits carry the AppID for a plain game.
                        mine = (cb_app as u32) == app_id_num;
                        if !mine && debug {
                            eprintln!("DEBUG: callback de otro app ({}), ignorado", cb_app as u32);
                        }
                    }
                    if !mine {
                        unsafe { free_cb(pipe) };
                        sleep(Duration::from_millis(10));
                        continue;
                    }
                    received = true;
                    // UserStatsReceived_t is { uint64 m_nGameID; EResult
                    // m_eResult; CSteamID m_steamIDUser; }. Only the arrival
                    // of this callback was ever checked, never its result —
                    // so stats that Steam declined to load looked exactly
                    // like stats that arrived fine, and the refusal only
                    // surfaced later as SetAchievement returning false with
                    // nothing to explain it.
                    if msg.param != 0 && msg.param_size >= 12 {
                        stats_result = unsafe {
                            std::ptr::read_unaligned((msg.param as *const u8).add(8) as *const i32)
                        };
                    }
                }
                unsafe { free_cb(pipe) };
                if received {
                    break;
                }
                // Yield even when callbacks keep coming. The sleep used to be
                // in the `else` alone, so a busy Steam kept this spinning a
                // core flat out while printing a line per callback — thousands
                // of them, each forwarded to the UI as its own Tauri event.
                sleep(Duration::from_millis(5));
            } else {
                sleep(Duration::from_millis(50));
            }
        }
        if debug {
            // 1 is k_EResultOK. Anything else means Steam has no usable stats
            // for this account and app, and every write will be refused.
            let meaning = match stats_result {
                -1 => "sin leer",
                1 => "OK",
                2 => "Fail",
                3 => "NoConnection",
                8 => "InvalidParam",
                9 => "FileNotFound",
                15 => "AccessDenied",
                20 => "ServiceUnavailable",
                _ => "otro",
            };
            eprintln!(
                "DEBUG: UserStatsReceived callback observed = {} (m_eResult={} {})",
                received, stats_result, meaning
            );
        }
    } else {
        // Exports missing (shouldn't happen on any real Steam install) —
        // fall back to a fixed wait rather than failing outright.
        sleep(Duration::from_secs(3));
    }

    if action == "get" {
        let get_fn: FnGetAchievementAndUnlockTime =
            unsafe { std::mem::transmute(vtable_fn(stats, SLOT_GET_ACHIEVEMENT_AND_UNLOCK_TIME)) };
        for name in payload.split(',').filter(|s| !s.is_empty()) {
            let cname = match CString::new(name) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let mut achieved: u8 = 0;
            let mut unlock_time: u32 = 0;
            let ok = unsafe { get_fn(stats, cname.as_ptr(), &mut achieved, &mut unlock_time) };
            if debug {
                eprintln!("DEBUG: GetAchievementAndUnlockTime('{}') -> ok={} achieved={} unlock_time={}", name, ok, achieved, unlock_time);
            }
            if ok != 0 {
                println!("{}={}={}", name, achieved, unlock_time);
            }
        }
        cleanup(true);
    }

    // Comma-separated payload lets the caller set/clear many achievements in
    // ONE Steam connection (StoreStats called once at the end) instead of
    // spawning this whole helper — and paying its connect/RequestUserStats/
    // callback-wait cost — once per achievement, which is what an "unlock
    // all" button would otherwise need to do. A single name (no comma) is
    // unaffected: split(',') on it just yields that one element.
    let names: Vec<&str> = payload.split(',').filter(|s| !s.is_empty()).collect();
    if names.is_empty() {
        fail("No se especificó ningún logro.");
    }

    // An unrecognised action must not fall through to deleting.
    //
    // This was `if action == "set" { SET } else { CLEAR }`, so a typo on the
    // command line — `sett` instead of `set` — silently CLEARED the
    // achievement, printed OK and exited 0. Destructive by default is the
    // wrong way round for a binary whose own USAGE line documents an argument
    // order that has already been gotten wrong once.
    let slot = match &action[..] {
        "set" => SLOT_SET_ACHIEVEMENT,
        "clear" => SLOT_CLEAR_ACHIEVEMENT,
        other => {
            eprintln!("ERROR: acción desconocida '{}'. Se esperaba set, clear, get o ticket.", other);
            cleanup(false);
            unreachable!()
        }
    };
    let set_or_clear: FnSetOrClearAchievement = unsafe { std::mem::transmute(vtable_fn(stats, slot)) };

    // Counted, not just "did any work".
    //
    // `any_ok` went true on a single success, so 1 achievement out of 500
    // meant StoreStats ran, "OK" was printed, and the launcher told the user
    // "500 logro(s) sincronizado(s) con Steam". Nothing downstream could tell
    // the difference, because the helper never reported a count.
    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    for name in &names {
        let cname = match CString::new(*name) {
            Ok(c) => c,
            Err(_) => {
                eprintln!("ERROR: el nombre de logro '{}' contiene un byte nulo inválido.", name);
                continue;
            }
        };
        let r = unsafe { set_or_clear(stats, cname.as_ptr()) != 0 };
        if debug {
            eprintln!("DEBUG: {}Achievement('{}') -> {}", if action == "set" { "Set" } else { "Clear" }, name, r);
        }
        if r {
            ok_count += 1;
        } else {
            fail_count += 1;
            eprintln!(
                "ERROR: {} devolvió false para '{}' (¿nombre de logro incorrecto, o cuenta sin progreso de stats para este juego?).",
                action, name
            );
        }
    }
    // The launcher parses this line to report a truthful number.
    println!("COUNT ok={} fail={}", ok_count, fail_count);
    if ok_count == 0 {
        cleanup(false);
    }

    let store_ok = unsafe {
        let f: FnStoreStats = std::mem::transmute(vtable_fn(stats, SLOT_STORE_STATS));
        let r = f(stats) != 0;
        if debug {
            eprintln!("DEBUG: StoreStats() -> {}", r);
        }
        r
    };
    if !store_ok {
        eprintln!("ERROR: StoreStats falló al guardar el cambio.");
        cleanup(false);
    }

    println!("OK");
    cleanup(true);
}
