//! Collage de la dictée dans l'app qui avait le focus.
//!
//! Le collage passe par le presse-papier puis un Cmd+V simulé, car c'est la seule voie
//! qui marche dans toutes les apps (terminaux, Electron, navigateurs).
//!
//! Piège déjà payé (analyse du 17/11/2026) : la version d'origine remettait l'ancien
//! presse-papier 100 ms après le Cmd+V. Les apps lentes à lire le presse-papier
//! (Electron : Claude desktop, VS Code, Cursor ; Chrome sous charge) le lisaient après
//! la remise, et collaient donc l'ANCIEN contenu au lieu de la dictée.
//! D'où deux règles ici :
//! 1. par défaut, on ne remet pas l'ancien contenu (la dictée reste copiée) ;
//! 2. si la remise est activée, elle est tardive ET protégée par `changeCount` :
//!    on ne remet que si personne n'a écrit dans le presse-papier entre-temps.

use arboard::Clipboard;
use std::time::{Duration, Instant};

// ── macOS : focus et compteur du presse-papier via le runtime ObjC (aucune dépendance) ──

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;

    extern "C" {
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    type EnvoiObjet = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;

    unsafe fn envoi_objet() -> EnvoiObjet {
        std::mem::transmute(objc_msgSend as *const ())
    }

    /// PID de l'app au premier plan, -1 si aucune.
    pub fn get_frontmost_pid() -> i32 {
        unsafe {
            type EnvoiI32 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
            let send_i32: EnvoiI32 = std::mem::transmute(objc_msgSend as *const ());
            let send_obj = envoi_objet();

            let cls = objc_getClass(b"NSWorkspace\0".as_ptr());
            let workspace = send_obj(cls, sel_registerName(b"sharedWorkspace\0".as_ptr()));
            let app = send_obj(workspace, sel_registerName(b"frontmostApplication\0".as_ptr()));
            if app.is_null() {
                return -1;
            }
            send_i32(app, sel_registerName(b"processIdentifier\0".as_ptr()))
        }
    }

    /// Ramène une app au premier plan par son PID.
    pub fn activate_pid(pid: i32) {
        unsafe {
            type EnvoiAvecI32 = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> *mut c_void;
            type EnvoiAvecUsize = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> bool;
            let send_with_i32: EnvoiAvecI32 = std::mem::transmute(objc_msgSend as *const ());
            let send_with_usize: EnvoiAvecUsize = std::mem::transmute(objc_msgSend as *const ());

            let cls = objc_getClass(b"NSRunningApplication\0".as_ptr());
            let app = send_with_i32(
                cls,
                sel_registerName(b"runningApplicationWithProcessIdentifier:\0".as_ptr()),
                pid,
            );
            if !app.is_null() {
                // NSApplicationActivateIgnoringOtherApps = 2
                send_with_usize(app, sel_registerName(b"activateWithOptions:\0".as_ptr()), 2);
            }
        }
    }

    /// Compteur de modifications du presse-papier général. Il augmente à chaque écriture,
    /// par n'importe quelle app : c'est ce qui permet de savoir si l'utilisateur a copié
    /// autre chose après notre collage.
    pub fn compteur_presse_papier() -> isize {
        unsafe {
            type EnvoiIsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
            let send_isize: EnvoiIsize = std::mem::transmute(objc_msgSend as *const ());
            let send_obj = envoi_objet();

            let cls = objc_getClass(b"NSPasteboard\0".as_ptr());
            let pasteboard = send_obj(cls, sel_registerName(b"generalPasteboard\0".as_ptr()));
            if pasteboard.is_null() {
                return -1;
            }
            send_isize(pasteboard, sel_registerName(b"changeCount\0".as_ptr()))
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::{activate_pid, get_frontmost_pid};

#[cfg(target_os = "macos")]
fn compteur_presse_papier() -> isize {
    macos::compteur_presse_papier()
}

#[cfg(not(target_os = "macos"))]
pub fn get_frontmost_pid() -> i32 {
    -1
}

#[cfg(not(target_os = "macos"))]
pub fn activate_pid(_pid: i32) {}

/// Hors macOS, pas de compteur : -1 désactive la remise protégée (on ne restaure jamais).
#[cfg(not(target_os = "macos"))]
fn compteur_presse_papier() -> isize {
    -1
}

/// Vrai si macOS autorise l'app à simuler des frappes (réglage Accessibilité).
///
/// Piège déjà payé (recette du 17/11/2026) : sans cette autorisation, macOS jette le
/// Cmd+V simulé SANS renvoyer d'erreur. L'app croyait avoir collé et rien n'apparaissait.
/// Après activation dans les Réglages, il faut relancer l'app.
#[cfg(target_os = "macos")]
pub fn accessibilite_accordee() -> bool {
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibilite_accordee() -> bool {
    true
}

// ── Décisions pures (testées plus bas) ──

/// Que faire de la dictée une fois transcrite.
#[derive(Debug, PartialEq, Eq)]
pub enum DecisionCollage {
    Coller,
    /// Copier seulement : un Cmd+V serait jeté par macOS.
    CopierAccessibiliteManquante,
    /// Copier seulement : le Cmd+V partirait dans une autre fenêtre.
    CopierFocusPerdu,
}

/// L'Accessibilité passe en premier : sans elle, même avec le focus, rien ne serait collé.
pub fn decider_collage(accessibilite: bool, focus_confirme: bool) -> DecisionCollage {
    if !accessibilite {
        DecisionCollage::CopierAccessibiliteManquante
    } else if !focus_confirme {
        DecisionCollage::CopierFocusPerdu
    } else {
        DecisionCollage::Coller
    }
}

/// Que faire de l'ancien contenu du presse-papier après le collage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restauration {
    /// La dictée reste dans le presse-papier. Défaut : zéro risque de coller l'ancien.
    Jamais,
    /// Remettre l'ancien contenu après ce délai, si personne n'a écrit entre-temps.
    ApresDelai(Duration),
}

/// Remet-on l'ancien contenu ? Seulement si on en avait un, et si le compteur n'a pas
/// bougé depuis notre écriture (sinon l'utilisateur a copié autre chose, on n'y touche
/// pas). Un compteur à -1 signifie « inconnu » : dans le doute, on ne restaure pas.
pub fn doit_restaurer(compteur_apres_ecriture: isize, compteur_actuel: isize, avait_un_precedent: bool) -> bool {
    avait_un_precedent
        && compteur_apres_ecriture >= 0
        && compteur_actuel == compteur_apres_ecriture
}

/// Attend que `pid` soit au premier plan, en sondant `lire_pid` toutes les 20 ms,
/// jusqu'à `delai_max`. Renvoie true si le focus est confirmé.
/// La source du PID est injectée pour pouvoir tester sans bureau.
pub fn attendre_focus(pid: i32, delai_max: Duration, mut lire_pid: impl FnMut() -> i32) -> bool {
    let debut = Instant::now();
    loop {
        if lire_pid() == pid {
            return true;
        }
        if debut.elapsed() >= delai_max {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ── Effets ──

/// Ce qui s'est passé pendant un collage, pour le journal.
#[derive(Debug, Default)]
pub struct RapportCollage {
    pub cmd_v_envoye: bool,
    pub ancien_restaure: bool,
}

/// Met la dictée dans le presse-papier sans coller. Sert quand le focus n'a pas pu être
/// confirmé : coller à l'aveugle enverrait le texte dans la mauvaise fenêtre.
pub fn copier_seulement(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init error: {}", e))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set error: {}", e))
}

pub fn paste_text(text: &str, restauration: Restauration) -> Result<RapportCollage, String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init error: {}", e))?;
    let precedent = match restauration {
        Restauration::Jamais => None,
        Restauration::ApresDelai(_) => clipboard.get_text().ok(),
    };

    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set error: {}", e))?;
    let compteur_apres_ecriture = compteur_presse_papier();

    // set_text est synchrone ; cette pause laisse seulement l'app cible finir d'activer
    // sa fenêtre avant de recevoir la frappe.
    std::thread::sleep(Duration::from_millis(30));
    simulate_paste()?;
    let mut rapport = RapportCollage { cmd_v_envoye: true, ..Default::default() };

    if let Restauration::ApresDelai(delai) = restauration {
        std::thread::sleep(delai);
        if doit_restaurer(compteur_apres_ecriture, compteur_presse_papier(), precedent.is_some()) {
            if let Some(prev) = precedent {
                rapport.ancien_restaure = clipboard.set_text(&prev).is_ok();
            }
        }
    }

    Ok(rapport)
}

/// Check (and optionally prompt for) macOS Accessibility permission.
/// Returns true if the app is already trusted.
#[cfg(target_os = "macos")]
pub fn ensure_accessibility_permission() -> bool {
    use std::ffi::c_void;

    #[repr(C)]
    struct CFDictionaryKeyCallBacks {
        _opaque: [u8; 0],
    }
    #[repr(C)]
    struct CFDictionaryValueCallBacks {
        _opaque: [u8; 0],
    }

    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;

        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const CFDictionaryKeyCallBacks,
            value_callbacks: *const CFDictionaryValueCallBacks,
        ) -> *const c_void;
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            c_str: *const i8,
            encoding: u32,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);

        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

    unsafe {
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            b"AXTrustedCheckOptionPrompt\0".as_ptr() as *const i8,
            K_CF_STRING_ENCODING_UTF8,
        );

        let keys = [key];
        let values = [kCFBooleanTrue];

        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr() as *const *const c_void,
            values.as_ptr() as *const *const c_void,
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        let trusted = AXIsProcessTrustedWithOptions(dict);

        CFRelease(dict);
        CFRelease(key);

        trusted
    }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_accessibility_permission() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn simulate_paste() -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGKeyCode};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // Key code 9 = 'v' on macOS
    const KEY_V: CGKeyCode = 9;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource")?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_V, true)
        .map_err(|_| "Failed to create key down event")?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);

    let key_up = CGEvent::new_keyboard_event(source, KEY_V, false)
        .map_err(|_| "Failed to create key up event")?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    key_down.post(core_graphics::event::CGEventTapLocation::HID);
    // Une frappe réelle dure quelques ms ; certaines apps ignorent un appui/relâché simultané.
    std::thread::sleep(Duration::from_millis(10));
    key_up.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn simulate_paste() -> Result<(), String> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init error: {}", e))?;

    enigo
        .key(Key::Control, Direction::Press)
        .map_err(|e| format!("Key press error: {}", e))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key click error: {}", e))?;
    enigo
        .key(Key::Control, Direction::Release)
        .map_err(|e| format!("Key release error: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restaure_si_personne_n_a_ecrit() {
        assert!(doit_restaurer(42, 42, true));
    }

    #[test]
    fn ne_restaure_pas_si_l_utilisateur_a_copie_autre_chose() {
        assert!(!doit_restaurer(42, 43, true));
    }

    #[test]
    fn ne_restaure_pas_sans_contenu_precedent() {
        assert!(!doit_restaurer(42, 42, false));
    }

    #[test]
    fn ne_restaure_pas_si_compteur_inconnu() {
        assert!(!doit_restaurer(-1, -1, true));
    }

    #[test]
    fn sans_accessibilite_on_copie_meme_avec_le_focus() {
        assert_eq!(decider_collage(false, true), DecisionCollage::CopierAccessibiliteManquante);
        assert_eq!(decider_collage(false, false), DecisionCollage::CopierAccessibiliteManquante);
    }

    #[test]
    fn sans_focus_on_copie() {
        assert_eq!(decider_collage(true, false), DecisionCollage::CopierFocusPerdu);
    }

    #[test]
    fn avec_accessibilite_et_focus_on_colle() {
        assert_eq!(decider_collage(true, true), DecisionCollage::Coller);
    }

    #[test]
    fn focus_confirme_des_que_le_pid_arrive() {
        let mut appels = 0;
        let ok = attendre_focus(7, Duration::from_millis(500), || {
            appels += 1;
            if appels >= 3 { 7 } else { 1 }
        });
        assert!(ok);
        assert_eq!(appels, 3);
    }

    #[test]
    fn focus_refuse_apres_le_delai_max() {
        let debut = Instant::now();
        let ok = attendre_focus(7, Duration::from_millis(100), || 1);
        assert!(!ok);
        assert!(debut.elapsed() >= Duration::from_millis(100));
    }
}
