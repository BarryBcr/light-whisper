//! Autotest sans bureau : rejoue un WAV au rythme réel à travers la VRAIE chaîne de
//! l'app (TranscripteurContinu puis decoupe::finaliser, même moteur, même rééchantillonnage
//! depuis 48 kHz comme un micro), sans overlay, sans presse-papier, sans Cmd+V.
//!
//! Pourquoi : le banc (`bench/`) mesure le modèle, pas l'app. Ce mode prouve que le
//! code livré produit le bon texte et mesure l'attente réelle après l'arrêt, morceau en
//! cours compris.
//!
//! Usage : LIGHTWHISPER_AUTOTEST=<fichier.wav 16 kHz mono> LIGHTWHISPER_AUTOTEST_DUREE=150 \
//!         LIGHTWHISPER_AUTOTEST_SORTIE=texte.txt "Light Whisper Dev.app/Contents/MacOS/light-whisper"

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio;
use crate::config;
use crate::decoupe::{self, TranscripteurContinu, Transcrire};
use crate::state;
use crate::stt::SttEngine;

const FREQUENCE_MICRO: u32 = 48_000;
const BLOC: Duration = Duration::from_millis(10);

fn lire_wav(chemin: &str) -> Result<Vec<f32>, String> {
    let mut lecteur = hound::WavReader::open(chemin).map_err(|e| e.to_string())?;
    let spec = lecteur.spec();
    if spec.sample_rate != 16_000 || spec.channels != 1 {
        return Err("WAV attendu : 16 kHz mono".into());
    }
    lecteur
        .samples::<i16>()
        .map(|s| s.map(|v| v as f32 / 32_768.0).map_err(|e| e.to_string()))
        .collect()
}

pub fn executer(chemin_wav: &str) -> Result<(), String> {
    let duree_s: f32 = std::env::var("LIGHTWHISPER_AUTOTEST_DUREE")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(60.0);

    let cfg = config::load_config();
    let mut engine = SttEngine::from_engine_name(&cfg.engine);
    engine.load_model(&state::get_model_path_for_config(&cfg))?;
    let engine = Arc::new(Mutex::new(engine));
    let langue = cfg.language.clone();
    let moteur = Arc::clone(&engine);
    let transcrire: Transcrire = Arc::new(move |a: &[f32]| moteur.lock().unwrap().transcribe(a, &langue));

    let wav = lire_wav(chemin_wav)?;
    let n = ((duree_s * 16_000.0) as usize).min(wav.len());
    // Comme un micro réel : 48 kHz, rééchantillonné par l'app.
    let micro = audio::resample(&wav[..n], 16_000, FREQUENCE_MICRO);

    let tampon = Arc::new(Mutex::new(Vec::<f32>::new()));
    let frequence = Arc::new(Mutex::new(FREQUENCE_MICRO));
    let continu = TranscripteurContinu::demarrer(Arc::clone(&transcrire), Arc::clone(&tampon), frequence);

    let par_bloc = (FREQUENCE_MICRO as f32 * BLOC.as_secs_f32()) as usize;
    let debut = Instant::now();
    for (i, bloc) in micro.chunks(par_bloc).enumerate() {
        tampon.lock().unwrap().extend_from_slice(bloc);
        // Rythme réel, sans dérive cumulée.
        let cible = BLOC * (i as u32 + 1);
        if let Some(reste) = cible.checked_sub(debut.elapsed()) {
            std::thread::sleep(reste);
        }
    }

    let arret = Instant::now();
    let bilan = continu.arreter();
    let tout = std::mem::take(&mut *tampon.lock().unwrap());
    let resultat = decoupe::finaliser(Some(bilan), &tout, FREQUENCE_MICRO, &transcrire)?;
    let attente = arret.elapsed().as_secs_f64();

    println!(
        "AUTOTEST audio={:.1}s attente_apres_arret={:.2}s morceaux={} vides={} mots={}",
        n as f32 / 16_000.0,
        attente,
        resultat.morceaux,
        resultat.morceaux_vides,
        resultat.texte.split_whitespace().count()
    );
    if let Ok(sortie) = std::env::var("LIGHTWHISPER_AUTOTEST_SORTIE") {
        std::fs::write(sortie, &resultat.texte).map_err(|e| e.to_string())?;
    }
    Ok(())
}
