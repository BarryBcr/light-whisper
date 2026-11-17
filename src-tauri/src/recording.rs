//! Cycle d'une dictée : appui → enregistrement → appui → transcription → collage.
//!
//! Ce que cette version change par rapport à l'origine, et pourquoi (analyse du 17/11/2026) :
//! - l'overlay passe en « Transcription… » dès l'appui d'arrêt et reste visible jusqu'au
//!   collage : avant, il disparaissait, l'utilisateur croyait que rien ne se passait et
//!   ré-appuyait ;
//! - un appui pendant la transcription est ignoré (`transcription_en_cours`) ;
//! - l'overlay ne prend plus le focus au démarrage : l'app cible le garde ;
//! - avant de coller, on vérifie que l'app cible a bien repris le focus. Sinon on ne
//!   colle pas à l'aveugle : la dictée reste copiée et l'overlay le dit ;
//! - texte vide, annulation par Échap et passage illisible sont affichés au lieu d'être muets ;
//! - chaque dictée est sauvée dans `derniere-dictee.txt` et journalisée (`journal.rs`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::audio;
use crate::config;
use crate::decoupe::{self, TranscripteurContinu, Transcrire};
use crate::journal::{self, LigneDictee};
use crate::paste::{self, Restauration};
use crate::state::AppState;

/// Délai maximal pour que l'app cible reprenne le focus avant le collage.
const ATTENTE_FOCUS_MAX: Duration = Duration::from_millis(1000);
/// Quand le focus a dû être repris, l'app cible a besoin d'un temps pour activer sa
/// fenêtre avant de recevoir la frappe.
const STABILISATION_FOCUS: Duration = Duration::from_millis(200);
/// Durée d'affichage d'un message de fin sur l'overlay.
const DUREE_MESSAGE: Duration = Duration::from_millis(1600);

fn emit_error(app: &AppHandle, msg: &str) {
    eprintln!("ERROR: {}", msg);
    let _ = app.emit("app-error", msg.to_string());
    // Show settings window so the user actually sees the toast
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// États affichés par l'overlay (`recorder.js`).
fn afficher_etat(app: &AppHandle, etat: &str, texte: &str) {
    let _ = app.emit(
        "etat-overlay",
        serde_json::json!({ "etat": etat, "texte": texte }),
    );
}

/// Affiche un message puis cache l'overlay, sauf si une nouvelle dictée a démarré
/// entre-temps (sinon on cacherait l'overlay de la dictée suivante).
fn terminer_avec_message(app: &AppHandle, etat: &str, texte: &str) {
    let state = app.state::<AppState>();
    let generation = state.generation_overlay.load(Ordering::SeqCst);
    montrer_overlay(app);
    afficher_etat(app, etat, texte);
    std::thread::sleep(DUREE_MESSAGE);
    if state.generation_overlay.load(Ordering::SeqCst) == generation {
        cacher_overlay(app);
    }
}

fn montrer_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("recorder") {
        // Pas de set_focus : l'app cible doit garder le focus pendant toute la dictée.
        let _ = window.center();
        let _ = window.show();
    }
}

fn cacher_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("recorder") {
        let _ = window.hide();
    }
}

fn register_escape(app: &AppHandle) {
    let _ = app.global_shortcut().register("Escape");
}

fn unregister_escape(app: &AppHandle) {
    let _ = app.global_shortcut().unregister("Escape");
}

/// Remet `transcription_en_cours` à faux quoi qu'il arrive (retour anticipé, panique).
struct GardeTranscription<'a>(&'a AppState);

impl Drop for GardeTranscription<'_> {
    fn drop(&mut self) {
        self.0.transcription_en_cours.store(false, Ordering::SeqCst);
    }
}

/// Transcription par le moteur partagé de l'app, sous verrou et protégée des paniques.
fn transcripteur(app: &AppHandle) -> Transcrire {
    let app = app.clone();
    Arc::new(move |audio_16k: &[f32]| {
        let state = app.state::<AppState>();
        let langue = state.config.lock().unwrap().language.clone();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut engine = state.engine.lock().unwrap();
            if !engine.is_loaded() {
                return Err("STT engine not loaded — download a model in Settings".to_string());
            }
            engine.transcribe(audio_16k, &langue)
        }))
        .unwrap_or_else(|_| {
            Err("Transcription crashed — try a different STT engine or model".to_string())
        })
    })
}

/// Stop recording and hide overlay without transcribing (ESC cancel).
pub fn cancel_recording(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.recorder.lock().unwrap().is_recording() {
        return;
    }

    if let Some(continu) = state.continu.lock().unwrap().take() {
        continu.abandonner();
    }
    let _ = state.recorder.lock().unwrap().stop();
    let _ = app.emit("recording-stopped", ());
    unregister_escape(app);

    let pid = state.previous_app_pid.load(Ordering::SeqCst);
    if pid > 0 {
        paste::activate_pid(pid);
    }
    journal::ecrire(&LigneDictee { issue: "annulee", app_cible_pid: pid, ..Default::default() });
    terminer_avec_message(app, "info", "Annulé");
}

pub fn do_toggle_recording(app: &AppHandle) {
    let state = app.state::<AppState>();

    if state.transcription_en_cours.load(Ordering::SeqCst) {
        afficher_etat(app, "occupe", "Transcription en cours…");
        return;
    }

    if state.recorder.lock().unwrap().is_recording() {
        arreter_et_coller(app, &state);
    } else {
        demarrer(app, &state);
    }
}

fn demarrer(app: &AppHandle, state: &AppState) {
    state.generation_overlay.fetch_add(1, Ordering::SeqCst);
    // Capturer l'app au premier plan AVANT de montrer l'overlay.
    let pid = paste::get_frontmost_pid();
    state.previous_app_pid.store(pid, Ordering::SeqCst);

    let config = state.config.lock().unwrap().clone();
    montrer_overlay(app);
    afficher_etat(app, "enregistrement", "");

    let demarrage = state.recorder.lock().unwrap().start(&config.audio_device, app.clone());
    if let Err(e) = demarrage {
        let _ = app.emit("recording-stopped", ());
        cacher_overlay(app);
        emit_error(app, &format!("Cannot start recording: {}", e));
        return;
    }
    register_escape(app);

    if config.transcription_par_morceaux {
        let (echantillons, frequence) = state.recorder.lock().unwrap().tampons_partages();
        *state.continu.lock().unwrap() = Some(TranscripteurContinu::demarrer(
            transcripteur(app),
            echantillons,
            frequence,
        ));
    }
}

fn arreter_et_coller(app: &AppHandle, state: &AppState) {
    state.transcription_en_cours.store(true, Ordering::SeqCst);
    let _garde = GardeTranscription(state);
    let arret = Instant::now();
    // Retour visuel immédiat, avant les attentes (morceau en cours, arrêt du micro).
    afficher_etat(app, "transcription", "Transcription…");
    unregister_escape(app);

    // Le transcripteur continu s'arrête en premier : il termine le morceau en cours et
    // rend la position jusqu'où l'audio est déjà transcrit.
    let continu = state.continu.lock().unwrap().take().map(|c| c.arreter());

    let result = state.recorder.lock().unwrap().stop();
    let _ = app.emit("recording-stopped", ());

    let (samples, sample_rate) = match result {
        Ok(r) => r,
        Err(e) => {
            cacher_overlay(app);
            emit_error(app, &format!("Recording failed: {}", e));
            return;
        }
    };

    if audio::est_silence_numerique(&samples) {
        cacher_overlay(app);
        let device = state.config.lock().unwrap().audio_device.clone();
        emit_error(app, &format!(
            "No audio detected (device: \"{}\"). Check that the device is connected, or grant microphone access in System Settings > Privacy & Security > Microphone",
            device
        ));
        return;
    }

    let mut ligne = LigneDictee {
        audio_s: samples.len() as f64 / sample_rate.max(1) as f64,
        app_cible_pid: state.previous_app_pid.load(Ordering::SeqCst),
        ..Default::default()
    };

    let debut_transcription = Instant::now();
    let resultat = match decoupe::finaliser(continu, &samples, sample_rate, &transcripteur(app)) {
        Ok(r) => r,
        Err(e) => {
            cacher_overlay(app);
            emit_error(app, &format!("Transcription failed: {}", e));
            return;
        }
    };
    let text = resultat.texte;
    ligne.transcription_s = debut_transcription.elapsed().as_secs_f64();
    ligne.caracteres = text.chars().count();
    ligne.morceaux = resultat.morceaux;
    ligne.morceaux_vides = resultat.morceaux_vides;

    if text.is_empty() {
        ligne.issue = "rien_entendu";
        ligne.attente_apres_arret_s = arret.elapsed().as_secs_f64();
        journal::ecrire(&ligne);
        terminer_avec_message(app, "info", "Rien entendu");
        return;
    }

    // Filet de sécurité : la dictée n'est jamais perdue, même si le collage échoue.
    let _ = std::fs::write(config::chemin_derniere_dictee(), &text);

    let pid = ligne.app_cible_pid;
    let focus_deja_la = pid > 0 && paste::get_frontmost_pid() == pid;
    if pid > 0 && !focus_deja_la {
        paste::activate_pid(pid);
    }
    ligne.focus_confirme =
        pid > 0 && paste::attendre_focus(pid, ATTENTE_FOCUS_MAX, paste::get_frontmost_pid);

    if !ligne.focus_confirme {
        // Coller à l'aveugle enverrait le texte dans la mauvaise fenêtre.
        let _ = paste::copier_seulement(&text);
        ligne.issue = "copiee_sans_coller";
        ligne.attente_apres_arret_s = arret.elapsed().as_secs_f64();
        journal::ecrire(&ligne);
        terminer_avec_message(app, "info", "Copié : faites Cmd+V");
        return;
    }
    if !focus_deja_la {
        std::thread::sleep(STABILISATION_FOCUS);
    }

    // L'overlay disparaît avant le Cmd+V pour ne rien intercepter.
    cacher_overlay(app);
    let config = state.config.lock().unwrap().clone();
    let restauration = if config.restaurer_presse_papier {
        Restauration::ApresDelai(Duration::from_millis(config.delai_restauration_ms))
    } else {
        Restauration::Jamais
    };
    ligne.attente_apres_arret_s = arret.elapsed().as_secs_f64();
    match paste::paste_text(&text, restauration) {
        Ok(rapport) => {
            ligne.issue = "collee";
            ligne.cmd_v_envoye = rapport.cmd_v_envoye;
            ligne.ancien_restaure = rapport.ancien_restaure;
            journal::ecrire(&ligne);
            if ligne.morceaux_vides > 0 {
                // Un passage parlé n'a pas pu être transcrit : le dire plutôt que de
                // laisser croire que le texte est complet.
                terminer_avec_message(app, "info", "Collé · passage illisible");
            }
        }
        Err(e) => {
            ligne.issue = "echec_collage";
            journal::ecrire(&ligne);
            emit_error(app, &format!(
                "Paste failed: {}. On macOS, enable Accessibility in System Settings > Privacy & Security > Accessibility",
                e
            ));
        }
    }
}
