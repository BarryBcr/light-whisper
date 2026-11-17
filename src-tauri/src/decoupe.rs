//! Transcription au fil de l'eau : on transcrit pendant que l'utilisateur parle, morceau
//! par morceau, en coupant aux pauses.
//!
//! Pourquoi : la version d'origine attendait l'arrêt pour transcrire tout l'audio d'un
//! bloc. L'attente grandissait avec la longueur de la dictée, or les dictées font souvent
//! plus d'une minute. Ici, à l'arrêt, il ne reste que le dernier morceau (quelques
//! secondes) à traiter : l'attente devient presque constante.
//!
//! Le choix des points de coupure est dans `coupure.rs` (pur, partagé avec le banc).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::audio;
pub use crate::coupure::{assembler, trouver_coupure, ECHANTILLONS_MIN_16K};
use crate::state::AppState;

const SONDAGE: Duration = Duration::from_millis(400);

/// Ce que le transcripteur rend à l'arrêt : les textes déjà prêts, et jusqu'où
/// (en échantillons à la fréquence native) l'audio a été transcrit.
pub struct Bilan {
    pub textes: Vec<String>,
    pub position: usize,
}

pub struct TranscripteurContinu {
    arret: Arc<AtomicBool>,
    fil: JoinHandle<Bilan>,
}

impl TranscripteurContinu {
    pub fn demarrer(
        app: AppHandle,
        echantillons: Arc<Mutex<Vec<f32>>>,
        frequence: Arc<Mutex<u32>>,
    ) -> Self {
        let arret = Arc::new(AtomicBool::new(false));
        let arret_fil = Arc::clone(&arret);
        let fil = std::thread::spawn(move || {
            let mut bilan = Bilan { textes: Vec::new(), position: 0 };
            let langue = app.state::<AppState>().config.lock().unwrap().language.clone();
            while !arret_fil.load(Ordering::SeqCst) {
                std::thread::sleep(SONDAGE);
                let sr = *frequence.lock().unwrap();
                if sr == 0 {
                    continue;
                }
                let en_attente: Vec<f32> = {
                    let tampon = echantillons.lock().unwrap();
                    if tampon.len() <= bilan.position {
                        continue;
                    }
                    tampon[bilan.position..].to_vec()
                };
                let Some(coupure) = trouver_coupure(&en_attente, sr) else {
                    continue;
                };
                let morceau_16k = audio::resample(&en_attente[..coupure], sr, 16000);
                let texte = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let state = app.state::<AppState>();
                    let mut engine = state.engine.lock().unwrap();
                    engine.transcribe(&morceau_16k, &langue)
                }));
                match texte {
                    Ok(Ok(t)) => {
                        bilan.textes.push(t);
                        bilan.position += coupure;
                    }
                    // En cas d'échec, on n'avance pas : ce morceau sera transcrit à l'arrêt
                    // avec le reste, comme dans la version d'origine.
                    _ => break,
                }
            }
            bilan
        });
        Self { arret, fil }
    }

    /// Arrête le fil (après le morceau en cours) et rend le bilan.
    pub fn arreter(self) -> Bilan {
        self.arret.store(true, Ordering::SeqCst);
        self.fil.join().unwrap_or(Bilan { textes: Vec::new(), position: 0 })
    }

    /// Annulation : on arrête sans garder le texte.
    pub fn abandonner(self) {
        let _ = self.arreter();
    }
}

