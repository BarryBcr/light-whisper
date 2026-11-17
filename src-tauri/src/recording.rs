//! Cycle d'une dictée : appui → enregistrement → appui → transcription → collage.
//!
//! Ce que cette version change par rapport à l'origine, et pourquoi (analyse du 17/11/2026) :
//! - l'overlay reste visible en « Transcription… » jusqu'au collage : avant, il disparaissait
//!   à l'arrêt, l'utilisateur croyait que rien ne se passait et ré-appuyait ;
//! - un appui pendant la transcription est ignoré (`transcription_en_cours`) ;
//! - l'overlay ne prend plus le focus au démarrage : l'app cible le garde ;
//! - avant de coller, on vérifie que l'app cible a bien repris le focus. Sinon on ne
//!   colle pas à l'aveugle : la dictée reste copiée et l'overlay le dit ;
//! - texte vide et annulation par Échap sont affichés au lieu d'être muets ;
//! - chaque dictée est sauvée dans `derniere-dictee.txt` et journalisée (`journal.rs`).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::audio;
use crate::config;
use crate::decoupe::{self, TranscripteurContinu};
use crate::journal::{self, LigneDictee};
use crate::paste::{self, Restauration};
use crate::state::AppState;

/// Délai maximal pour que l'app cible reprenne le focus avant le collage.
const ATTENTE_FOCUS_MAX: Duration = Duration::from_millis(1000);
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

/// Affiche un message puis cache l'overlay.
fn terminer_avec_message(app: &AppHandle, etat: &str, texte: &str) {
    afficher_etat(app, etat, texte);
    std::thread::sleep(DUREE_MESSAGE);
    cacher_overlay(app);
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
        *state.continu.lock().unwrap() =
            Some(TranscripteurContinu::demarrer(app.clone(), echantillons, frequence));
    }
}

fn arreter_et_coller(app: &AppHandle, state: &AppState) {
    state.transcription_en_cours.store(true, Ordering::SeqCst);
    let _garde = GardeTranscription(state);
    let arret = Instant::now();

    // Le transcripteur continu s'arrête en premier : il termine le morceau en cours et
    // rend la position jusqu'où l'audio est déjà transcrit.
    let continu = state.continu.lock().unwrap().take().map(|c| c.arreter());

    let result = state.recorder.lock().unwrap().stop();
    let _ = app.emit("recording-stopped", ());
    unregister_escape(app);
    afficher_etat(app, "transcription", "Transcription…");

    let (samples, sample_rate) = match result {
        Ok(r) => r,
        Err(e) => {
            cacher_overlay(app);
            emit_error(app, &format!("Recording failed: {}", e));
            return;
        }
    };
    let audio_s = samples.len() as f64 / sample_rate.max(1) as f64;

    if audio::est_silence_numerique(&samples) {
        cacher_overlay(app);
        let device = state.config.lock().unwrap().audio_device.clone();
        emit_error(app, &format!(
            "No audio detected (device: \"{}\"). Check that the device is connected, or grant microphone access in System Settings > Privacy & Security > Microphone",
            device
        ));
        return;
    }

    let config = state.config.lock().unwrap().clone();
    let debut_transcription = Instant::now();

    // Seule la partie pas encore transcrite par le continu reste à traiter.
    let (deja_transcrit, debut_reste) = match continu {
        Some(bilan) => (bilan.textes, bilan.position.min(samples.len())),
        None => (Vec::new(), 0),
    };
    let reste_16k = audio::resample(&samples[debut_reste..], sample_rate, 16000);

    let transcription = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut engine = state.engine.lock().unwrap();
        if !engine.is_loaded() {
            return Err("STT engine not loaded — download a model in Settings".to_string());
        }
        if reste_16k.len() < decoupe::ECHANTILLONS_MIN_16K {
            return Ok(String::new());
        }
        engine.transcribe(&reste_16k, &config.language)
    }));

    let mut ligne = LigneDictee {
        audio_s,
        morceaux: deja_transcrit.len() + 1,
        app_cible_pid: state.previous_app_pid.load(Ordering::SeqCst),
        ..Default::default()
    };

    let text = match transcription {
        Ok(Ok(fin)) => {
            let mut morceaux = deja_transcrit;
            morceaux.push(fin);
            decoupe::assembler(&morceaux)
        }
        Ok(Err(e)) => {
            cacher_overlay(app);
            emit_error(app, &format!("Transcription failed: {}", e));
            return;
        }
        Err(_) => {
            cacher_overlay(app);
            emit_error(app, "Transcription crashed — try a different STT engine or model");
            return;
        }
    };
    ligne.transcription_s = debut_transcription.elapsed().as_secs_f64();
    ligne.caracteres = text.chars().count();

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
    if pid > 0 {
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

    // L'overlay disparaît avant le Cmd+V pour ne rien intercepter.
    cacher_overlay(app);
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
