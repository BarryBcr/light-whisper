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

use crate::audio;
pub use crate::coupure::{assembler, trouver_coupure, ECHANTILLONS_MIN_16K};

const SONDAGE: Duration = Duration::from_millis(400);
/// Au-dessus de ce RMS, un morceau contient de la parole : s'il revient vide après
/// tous les réessais, on le compte comme passage perdu (journal + overlay).
const RMS_PAROLE: f32 = 0.01;

/// Transcription d'un audio 16 kHz mono. Injectée pour que le même code tourne dans
/// l'app (moteur partagé sous verrou) et dans l'autotest (`autotest.rs`).
pub type Transcrire = Arc<dyn Fn(&[f32]) -> Result<String, String> + Send + Sync>;

/// Ce que le transcripteur rend à l'arrêt : les textes déjà prêts, jusqu'où (en
/// échantillons à la fréquence native) l'audio a été transcrit, et les passages perdus.
#[derive(Default)]
pub struct Bilan {
    pub textes: Vec<String>,
    pub position: usize,
    pub morceaux_vides: usize,
}

/// Texte final d'une dictée.
pub struct Resultat {
    pub texte: String,
    pub morceaux: usize,
    pub morceaux_vides: usize,
}

fn est_parole(audio: &[f32]) -> bool {
    !audio.is_empty()
        && (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt() > RMS_PAROLE
}

/// Transcrit ce qui reste après le dernier morceau et assemble le texte final.
/// Seul point d'entrée de fin de dictée, partagé par l'app et l'autotest.
pub fn finaliser(
    bilan: Option<Bilan>,
    samples: &[f32],
    sample_rate: u32,
    transcrire: &Transcrire,
) -> Result<Resultat, String> {
    let bilan = bilan.unwrap_or_default();
    let debut_reste = bilan.position.min(samples.len());
    let reste_16k = audio::resample(&samples[debut_reste..], sample_rate, 16000);
    let mut morceaux_vides = bilan.morceaux_vides;
    let mut textes = bilan.textes;
    if reste_16k.len() >= ECHANTILLONS_MIN_16K {
        let fin = transcrire(&reste_16k)?;
        if fin.trim().is_empty() && est_parole(&reste_16k) {
            morceaux_vides += 1;
        }
        textes.push(fin);
    }
    Ok(Resultat { texte: assembler(&textes), morceaux: textes.len(), morceaux_vides })
}

pub struct TranscripteurContinu {
    arret: Arc<AtomicBool>,
    fil: JoinHandle<Bilan>,
}

impl TranscripteurContinu {
    pub fn demarrer(
        transcrire: Transcrire,
        echantillons: Arc<Mutex<Vec<f32>>>,
        frequence: Arc<Mutex<u32>>,
    ) -> Self {
        let arret = Arc::new(AtomicBool::new(false));
        let arret_fil = Arc::clone(&arret);
        let fil = std::thread::spawn(move || {
            let mut bilan = Bilan::default();
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
                match transcrire(&morceau_16k) {
                    Ok(t) => {
                        if t.trim().is_empty() && est_parole(&morceau_16k) {
                            bilan.morceaux_vides += 1;
                        }
                        bilan.textes.push(t);
                        bilan.position += coupure;
                    }
                    // En cas d'échec, on n'avance pas : ce morceau sera transcrit à l'arrêt
                    // avec le reste, comme dans la version d'origine.
                    Err(_) => break,
                }
            }
            bilan
        });
        Self { arret, fil }
    }

    /// Arrête le fil (après le morceau en cours) et rend le bilan.
    pub fn arreter(self) -> Bilan {
        self.arret.store(true, Ordering::SeqCst);
        self.fil.join().unwrap_or_default()
    }

    /// Annulation : on arrête sans garder le texte.
    pub fn abandonner(self) {
        let _ = self.arreter();
    }
}
