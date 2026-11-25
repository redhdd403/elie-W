//! Demo pedagogique de fenetres Win32 : une pop-up, puis cinq enfants a
//! chaque fermeture. Profondeur bornee par defaut ; voir README.md.

#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DrawTextW, EndPaint, GetStockObject, SelectObject, SetBkMode,
    SetTextColor, UpdateWindow, DEFAULT_GUI_FONT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::Media::Multimedia::mciSendStringW;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, LoadCursorW, PostQuitMessage,
    RegisterClassW, SendMessageW, SetProcessDPIAware, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, BS_PUSHBUTTON, GWLP_USERDATA, IDC_ARROW, IDOK, MSG, SM_CXSCREEN,
    SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_PAINT, WM_SETFONT, WNDCLASSW,
    WS_CAPTION, WS_CHILD, WS_SYSMENU, WS_VISIBLE,
};

/// Enfants ouverts a chaque fermeture.
const CHILDREN: usize = 5;

/// Mode `--infinite` : pop-ups ouvertes periodiquement, sans fermeture. Elles
/// sont independantes des enfants ouverts a chaque fermeture.
const SPAWN_PER_TICK: usize = 2;
const SPAWN_INTERVAL: Duration = Duration::from_secs(1);

/// Profondeur par defaut : generations 0 a 2, soit `(5^3 - 1) / 4 = 31` pop-ups.
const DEFAULT_DEPTH: usize = 3;

/// Taille de la pop-up en zone **cliente** (le cadre est ajoute par
/// `AdjustWindowRectEx`).
const CLIENT_W: i32 = 210;
const CLIENT_H: i32 = 150;

/// Hauteur reservee au bouton OK en bas de la zone cliente.
const BUTTON_ZONE: i32 = 46;

/// Marge (px) entre deux pop-ups d'une meme vague.
const GAP: i32 = 16;

const POPUP_STYLE: u32 = WS_CAPTION | WS_SYSMENU;

/// Nom de classe (statique, terminee par NUL) : `RegisterClassW` conserve le
/// pointeur, il doit donc rester valide pour toute la duree du processus.
const CLASS_NAME: &[u16] = &[
    b'r' as u16,
    b'a' as u16,
    b'a' as u16,
    b'a' as u16,
    b'a' as u16,
    b'a' as u16,
    b'h' as u16,
    b'-' as u16,
    b'w' as u16,
    b'e' as u16,
    b'e' as u16,
    b'e' as u16,
    b'e' as u16,
    0,
];

const BODY_TEXTS: &[&str] = &[
    "raaaaah weeee",
    "RAAAAAH WEEEE",
    "raaaaah weeee !",
    "raaah. weee. raaah.",
];

const TITLE_TEXTS: &[&str] = &["raaaaah weeee", "raaah", "weeee", "raaaaah"];

/// Pop-ups ouvertes **plus** celles dont le thread est en cours de lancement.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Nombre de generations affichees (borne par defaut, cf. `DEFAULT_DEPTH`).
static DEPTH_LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_DEPTH);
/// Mode non borne (`--infinite`) : spawn periodique et aucune limite de
/// profondeur. A reserver a une machine virtuelle jetable.
static INFINITE: AtomicBool = AtomicBool::new(false);

static SEED: AtomicU64 = AtomicU64::new(0);
static SEED_INIT: Once = Once::new();
static REGISTER_CLASS: Once = Once::new();

// ---------------------------------------------------------------------------
// Alea maison (xorshift64*, pas de crate rand pour si peu)
// ---------------------------------------------------------------------------

fn rand_u64() -> u64 {
    SEED_INIT.call_once(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        // Horloge + adresse d'une locale : deux processus lances au meme
        // instant n'auront pas la meme suite.
        let local = &nanos as *const u64 as u64;
        SEED.store((nanos ^ local.rotate_left(17)) | 1, Ordering::Relaxed);
    });

    // Boucle de CAS : deux threads ne peuvent pas tirer la meme valeur.
    let mut current = SEED.load(Ordering::Relaxed);
    loop {
        let mut next = current;
        next ^= next << 13;
        next ^= next >> 7;
        next ^= next << 17;
        match SEED.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

/// Entier aleatoire dans `[lo, hi]` (inclus).
fn rand_range(lo: i32, hi: i32) -> i32 {
    if hi <= lo {
        return lo;
    }
    let span = (hi - lo) as u64 + 1;
    lo + (rand_u64() % span) as i32
}

fn pick<'a>(choices: &[&'a str]) -> &'a str {
    choices[(rand_u64() % choices.len() as u64) as usize]
}

// ---------------------------------------------------------------------------
// Geometrie
// ---------------------------------------------------------------------------

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Taille reelle de la fenetre : zone cliente + cadre + barre de titre.
fn popup_window_size() -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: CLIENT_W,
        bottom: CLIENT_H,
    };
    unsafe { AdjustWindowRectEx(&mut rect, POPUP_STYLE, 0, 0) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Position aleatoire sur le bureau virtuel (tous ecrans), fenetre
/// entierement visible.
fn random_position() -> (i32, i32) {
    let (width, height) = popup_window_size();
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let max_x = vx + vw - width;
    let max_y = vy + vh - height;
    if max_x <= vx || max_y <= vy {
        return (vx.max(0), vy.max(0));
    }
    (rand_range(vx, max_x), rand_range(vy, max_y))
}

/// Position centree sur l'ecran principal (pop-up initiale).
fn centered_position() -> (i32, i32) {
    let (width, height) = popup_window_size();
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    (((sw - width) / 2).max(0), ((sh - height) / 2).max(0))
}

fn overlaps(a: (i32, i32), b: (i32, i32), width: i32, height: i32) -> bool {
    (a.0 - b.0).abs() < width + GAP && (a.1 - b.1).abs() < height + GAP
}

/// `count` positions aleatoires, sans recouvrement entre elles (nombre d'essais
/// borne pour ne pas boucler sur un bureau surcharge).
fn spread_positions(count: usize) -> Vec<(i32, i32)> {
    let (width, height) = popup_window_size();
    let mut positions = Vec::with_capacity(count);
    for _ in 0..count {
        let mut candidate = random_position();
        for _ in 0..64 {
            if positions
                .iter()
                .all(|p| !overlaps(candidate, *p, width, height))
            {
                break;
            }
            candidate = random_position();
        }
        positions.push(candidate);
    }
    positions
}

// ---------------------------------------------------------------------------
// Audio : un mp3 a chaque ouverture de fenetre (MCI / winmm)
// ---------------------------------------------------------------------------

/// Voix MCI ouvertes une seule fois puis reutilisees en boucle : jusqu'a
/// `VOICES` sons simultanes, sans jamais fuir de handle ni de thread.
const VOICES: usize = 8;

/// Prefixe des alias MCI (`raaaaah0`, `raaaaah1`, ...).
const ALIAS_PREFIX: &str = "raaaaah";

/// Fichier choisi par `--sound` ou trouve a cote de l'executable.
static SOUND_FILE: OnceLock<PathBuf> = OnceLock::new();
/// File des demandes de lecture. Le thread audio est le seul a toucher aux
/// alias MCI : MCI attend que les commandes viennent du thread qui a ouvert
/// le peripherique.
static SOUND_TX: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// Envoie une commande MCI. Le code de retour est ignore : sans mp3 valide,
/// MCI echoue proprement et l'application reste simplement silencieuse.
fn mci(cmd: &str) {
    let cmd = wide(cmd);
    unsafe { mciSendStringW(cmd.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) };
}

/// mp3 a jouer, par ordre de priorite :
/// 1. `--sound` (fichier explicite) ;
/// 2. `AAAAAHWEEEE.mp3` / `raaaaah.mp3` / `sound.mp3` / `popup.mp3` a cote de
///    l'executable, puis dans le repertoire courant ;
/// 3. rien : le programme reste muet.
fn sound_file() -> Option<PathBuf> {
    if let Some(path) = SOUND_FILE.get() {
        return Some(path.clone());
    }

    let mut dirs = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    {
        dirs.push(dir);
    }
    // `cargo run` lance l'exe depuis target/..., mais garde la racine du crate
    // comme repertoire courant : on y cherche aussi le mp3.
    if let Ok(cwd) = std::env::current_dir() {
        if !dirs.contains(&cwd) {
            dirs.push(cwd);
        }
    }

    for dir in dirs {
        for name in [
            "AAAAAHWEEEE.mp3",
            "raaaaah.mp3",
            "sound.mp3",
            "popup.mp3",
        ] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                let _ = SOUND_FILE.set(candidate.clone());
                return Some(candidate);
            }
        }
    }

    None
}

/// Thread audio : ouvre les alias une fois, puis joue chaque demande sur la
/// voix suivante (round-robin). `from 0` relance depuis le debut, ce qui donne
/// un nouveau son a chaque ouverture meme si les huit voix tournent deja.
fn audio_thread(rx: mpsc::Receiver<()>) {
    let Some(path) = sound_file() else {
        // Pas de mp3 : on vide la file pour que les envois ne s'accumulent pas.
        while rx.recv().is_ok() {}
        return;
    };

    let file = path.display();
    for voice in 0..VOICES {
        mci(&format!(
            "open \"{file}\" type mpegvideo alias {ALIAS_PREFIX}{voice}"
        ));
    }

    let mut voice = 0;
    while rx.recv().is_ok() {
        mci(&format!("play {ALIAS_PREFIX}{voice} from 0"));
        voice = (voice + 1) % VOICES;
    }
}

/// Joue le mp3 d'ouverture de pop-up. Non bloquant : la demande est seulement
/// deposee dans la file du thread audio.
fn play_open_sound() {
    let tx = SOUND_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || audio_thread(rx));
        tx
    });
    let _ = tx.send(());
}

// ---------------------------------------------------------------------------
// Fenetres
// ---------------------------------------------------------------------------

/// Donnees attachees a une fenetre (`GWLP_USERDATA`).
struct PopupState {
    generation: usize,
    title: Vec<u16>,
    body: Vec<u16>,
    /// Garantit que les enfants ne sont lances qu'une seule fois, quel que
    /// soit le chemin de fermeture (croix, bouton OK).
    claimed: AtomicBool,
}

/// Arguments passes a un thread de pop-up (clonables pour le repli en ligne).
#[derive(Clone)]
struct PopupArgs {
    generation: usize,
    /// `None` = centree sur l'ecran principal.
    position: Option<(i32, i32)>,
    title: Vec<u16>,
    body: Vec<u16>,
}

unsafe fn state_ref(hwnd: HWND) -> Option<&'static PopupState> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PopupState;
    // `null.as_ref()` est un comportement indefini, meme s'il renvoie None
    // en pratique : on teste le pointeur avant de le dereferencer.
    if ptr.is_null() {
        None
    } else {
        Some(&*ptr)
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let hinstance = GetModuleHandleW(std::ptr::null());
            let class = wide("BUTTON");
            let label = wide("OK");
            let button = CreateWindowExW(
                0,
                class.as_ptr(),
                label.as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32,
                (CLIENT_W - 84) / 2,
                CLIENT_H - 40,
                84,
                28,
                hwnd,
                IDOK as usize as *mut c_void,
                hinstance,
                std::ptr::null(),
            );
            if !button.is_null() {
                let font = GetStockObject(DEFAULT_GUI_FONT);
                SendMessageW(button, WM_SETFONT, font as usize, 1);
            }
            0
        }
        WM_COMMAND => {
            // Bouton OK : notification BN_CLICKED (code 0) sur IDOK.
            if (wparam & 0xffff) == IDOK as usize {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            rect.bottom -= BUTTON_ZONE;

            let font = GetStockObject(DEFAULT_GUI_FONT);
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, 0x0000_0000);
            if let Some(state) = state_ref(hwnd) {
                DrawTextW(
                    hdc,
                    state.body.as_ptr(),
                    state.body.len() as i32 - 1,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
            SelectObject(hdc, old);
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            finish_popup(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn register_class(hinstance: *mut c_void) {
    REGISTER_CLASS.call_once(|| {
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            // Fond blanc, volontairement jamais libere : la classe vit
            // jusqu'a la fin du processus.
            hbrBackground: CreateSolidBrush(0x00FF_FFFF),
            lpszMenuName: std::ptr::null(),
            lpszClassName: CLASS_NAME.as_ptr(),
        };
        RegisterClassW(&class);
    });
}

/// Cree la fenetre et fait tourner sa boucle de messages jusqu'a sa fermeture.
fn run_popup(args: PopupArgs) {
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    unsafe { register_class(hinstance) };

    let (x, y) = args.position.unwrap_or_else(centered_position);
    let (width, height) = popup_window_size();

    let state = Box::new(PopupState {
        generation: args.generation,
        title: args.title.clone(),
        body: args.body.clone(),
        claimed: AtomicBool::new(false),
    });

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            CLASS_NAME.as_ptr(),
            state.title.as_ptr(),
            POPUP_STYLE,
            x,
            y,
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };

    if hwnd.is_null() {
        // Pas de fenetre : on rend le jeton pour ne pas bloquer `main`.
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
        return;
    }

    // Un son a chaque ouverture effective de fenetre.
    play_open_sound();

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Lance une pop-up : dans son propre thread, ou en ligne si la limite de
/// threads de Windows est atteinte (pour ne pas perdre la pop-up).
fn launch_popup(generation: usize, position: Option<(i32, i32)>) {
    // Jeton pris avant `spawn` : sinon `main` pourrait voir 0 et rendre la
    // main avant le moindre affichage.
    ACTIVE.fetch_add(1, Ordering::SeqCst);

    let args = PopupArgs {
        generation,
        position,
        title: wide(pick(TITLE_TEXTS)),
        body: wide(pick(BODY_TEXTS)),
    };

    let child_args = args.clone();
    match thread::Builder::new().spawn(move || run_popup(child_args)) {
        Ok(_) => {}
        Err(_) => run_popup(args),
    }
}

/// Unique point de sortie d'une pop-up (`WM_DESTROY`).
unsafe fn finish_popup(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PopupState;
    if ptr.is_null() {
        return;
    }
    let state = &*ptr;
    if state.claimed.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = state.generation;

    // Enfants d'abord, jeton rendu ensuite (cf. README, compteur ACTIVE).
    // `DEPTH_LIMIT` compte les generations affichees : 3 => 1+5+25 = 31.
    if generation.saturating_add(1) < DEPTH_LIMIT.load(Ordering::SeqCst) {
        for position in spread_positions(CHILDREN) {
            launch_popup(generation + 1, Some(position));
        }
    }

    drop(Box::from_raw(ptr));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    ACTIVE.fetch_sub(1, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Desinstallation et point d'entree
// ---------------------------------------------------------------------------

/// `--uninstall` : retire l'exe d'une eventuelle installation anterieure dans
/// le dossier Demarrage de l'utilisateur. Le programme ne s'y installe plus.
fn remove_startup_copy() {
    let (Ok(exe), Ok(appdata)) = (std::env::current_exe(), std::env::var("APPDATA")) else {
        return;
    };
    let dir = std::path::Path::new(&appdata).join("Microsoft/Windows/Start Menu/Programs/Startup");
    if let Some(name) = exe.file_name() {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

fn main() {
    unsafe { SetProcessDPIAware() };

    let mut args = std::env::args().skip(1);
    let mut uninstall = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--uninstall" => uninstall = true,
            "--infinite" => {
                INFINITE.store(true, Ordering::SeqCst);
                DEPTH_LIMIT.store(usize::MAX, Ordering::SeqCst);
            }
            "--depth" => {
                if let Some(value) = args.next().and_then(|v| v.parse().ok()) {
                    DEPTH_LIMIT.store(value, Ordering::SeqCst);
                }
            }
            "--sound" => {
                if let Some(value) = args.next() {
                    let path = PathBuf::from(value);
                    let path = if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir().unwrap_or_default().join(path)
                    };
                    let _ = SOUND_FILE.set(path);
                }
            }
            other => {
                if let Ok(value) = other.parse::<usize>() {
                    DEPTH_LIMIT.store(value, Ordering::SeqCst);
                }
            }
        }
    }

    if uninstall {
        remove_startup_copy();
    }

    launch_popup(0, None);

    // Mode fini par defaut : le spawn periodique n'existe qu'avec `--infinite`,
    // sinon le processus ne s'arreterait jamais.
    let periodic = INFINITE.load(Ordering::SeqCst);

    let mut next_spawn = Instant::now() + SPAWN_INTERVAL;
    loop {
        // `--infinite` : deux pop-ups de plus chaque seconde.
        if periodic && Instant::now() >= next_spawn {
            for position in spread_positions(SPAWN_PER_TICK) {
                launch_popup(0, Some(position));
            }
            next_spawn = Instant::now() + SPAWN_INTERVAL;
        }

        // Mode fini (defaut) : on rend la main quand la derniere pop-up est fermee.
        if !periodic && ACTIVE.load(Ordering::SeqCst) == 0 {
            break;
        }

        thread::sleep(Duration::from_millis(20));
    }
}
