//! Où couper l'audio pour la transcription au fil de l'eau. Fonctions pures, sans
//! dépendance : le banc (`bench/`) inclut ce fichier tel quel, pour mesurer exactement
//! le découpage que l'app utilise.
//!
//! Choix de réglage :
//! - jamais de morceau sous 12 s : le modèle a besoin de contexte pour bien transcrire ;
//! - coupure forcée à 30 s si aucune pause nette, au point le plus calme. Au-delà, le
//!   temps de transcription d'un bloc grimpe vite (banc 17/11 : 60 s = 4 s, 180 s = 35 s) ;
//! - on ne coupe jamais dans la dernière demi-seconde reçue : la phrase peut continuer.

const MORCEAU_MIN_S: f32 = 12.0;
const MORCEAU_MAX_S: f32 = 30.0;
const MARGE_FIN_S: f32 = 0.5;
const TRAME_S: f32 = 0.02;
const FENETRE_PAUSE_TRAMES: usize = 15; // 300 ms
/// Une pause est une fenêtre dont l'énergie est sous ce ratio de l'énergie médiane.
const RATIO_PAUSE: f32 = 0.3;
/// Moins de 0,25 s d'audio à 16 kHz : rien d'utile à transcrire.
pub const ECHANTILLONS_MIN_16K: usize = 4000;

/// Cherche où couper l'audio en attente (qui commence au curseur).
/// Renvoie le décalage de coupure, ou None s'il faut attendre plus d'audio.
pub fn trouver_coupure(en_attente: &[f32], frequence: u32) -> Option<usize> {
    let f = frequence as f32;
    let min = (MORCEAU_MIN_S * f) as usize;
    let max = (MORCEAU_MAX_S * f) as usize;
    let marge = (MARGE_FIN_S * f) as usize;
    let trame = ((TRAME_S * f) as usize).max(1);

    if en_attente.len() < min + marge {
        return None;
    }
    let fin_zone = (en_attente.len() - marge).min(max);
    if fin_zone <= min {
        return None;
    }

    let energies: Vec<f32> = en_attente[min..fin_zone]
        .chunks(trame)
        .map(|t| t.iter().map(|s| s * s).sum::<f32>() / t.len() as f32)
        .collect();
    if energies.len() < FENETRE_PAUSE_TRAMES {
        return None;
    }

    let mut triees = energies.clone();
    triees.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mediane = triees[triees.len() / 2];

    let (meilleure, energie_min) = (0..=energies.len() - FENETRE_PAUSE_TRAMES)
        .map(|i| {
            let e = energies[i..i + FENETRE_PAUSE_TRAMES].iter().sum::<f32>()
                / FENETRE_PAUSE_TRAMES as f32;
            (i, e)
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let pause_nette = energie_min <= mediane * RATIO_PAUSE;
    let zone_pleine = en_attente.len() - marge >= max;
    if pause_nette || zone_pleine {
        // Couper au milieu de la fenêtre la plus calme.
        Some(min + (meilleure + FENETRE_PAUSE_TRAMES / 2) * trame)
    } else {
        None
    }
}

/// Entoure l'audio (16 kHz) de `marge` échantillons de silence de chaque côté.
pub fn avec_marges_silence(audio_16k: &[f32], marge: usize) -> Vec<f32> {
    let mut sortie = Vec::with_capacity(audio_16k.len() + 2 * marge);
    sortie.resize(marge, 0.0);
    sortie.extend_from_slice(audio_16k);
    sortie.resize(sortie.len() + marge, 0.0);
    sortie
}

/// Marges essayées tour à tour (0,5 s, 0, 1 s) tant que le texte revient vide.
const MARGES_ESSAIS: [usize; 3] = [8_000, 0, 16_000];

/// Transcrit en réessayant avec une autre marge de silence si le texte revient vide.
///
/// Piège déjà payé (banc du 17/11/2026) : Parakeet TDT renvoie parfois un texte VIDE
/// pour un morceau qui contient pourtant de la parole, de façon reproductible pour
/// une entrée exacte (ex. échantillons 1356160-1551040 de l'audio du banc : vide 3 fois
/// sur 3), et le même morceau légèrement plus long ou plus court passe. Sans ce
/// réessai, environ 1 morceau sur 10 disparaissait du texte final.
/// `transcrire` est injecté pour tester sans modèle.
pub fn transcrire_avec_reessais<E>(
    audio_16k: &[f32],
    mut transcrire: impl FnMut(Vec<f32>) -> Result<String, E>,
) -> Result<String, E> {
    let mut texte = String::new();
    for marge in MARGES_ESSAIS {
        texte = transcrire(avec_marges_silence(audio_16k, marge))?;
        if !texte.trim().is_empty() {
            break;
        }
    }
    Ok(texte)
}

/// Recolle les morceaux transcrits.
pub fn assembler(morceaux: &[String]) -> String {
    morceaux
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// Parole simulée : un signal non nul ; pause : du silence.
    fn parole(secondes: f32) -> Vec<f32> {
        (0..(secondes * SR as f32) as usize)
            .map(|i| 0.3 * ((i as f32) * 0.05).sin())
            .collect()
    }

    fn silence(secondes: f32) -> Vec<f32> {
        vec![0.0; (secondes * SR as f32) as usize]
    }

    #[test]
    fn attend_tant_que_le_morceau_est_trop_court() {
        let mut a = parole(5.0);
        a.extend(silence(1.0));
        a.extend(parole(2.0));
        assert_eq!(trouver_coupure(&a, SR), None);
    }

    #[test]
    fn coupe_dans_la_pause_apres_le_minimum() {
        let mut a = parole(14.0);
        a.extend(silence(0.6));
        a.extend(parole(3.0));
        let c = trouver_coupure(&a, SR).expect("une coupure");
        let s = c as f32 / SR as f32;
        assert!((14.0..=14.6).contains(&s), "coupure à {s} s");
    }

    #[test]
    fn force_une_coupure_au_maximum_sans_pause() {
        let a = parole(32.0);
        let c = trouver_coupure(&a, SR).expect("coupure forcée");
        assert!(c <= (MORCEAU_MAX_S * SR as f32) as usize);
    }

    #[test]
    fn ne_coupe_pas_sans_pause_avant_le_maximum() {
        let a = parole(20.0);
        assert_eq!(trouver_coupure(&a, SR), None);
    }

    #[test]
    fn marges_de_silence_autour() {
        let a = avec_marges_silence(&[1.0, 1.0], 8_000);
        assert_eq!(a.len(), 16_002);
        assert_eq!(a[7_999], 0.0);
        assert_eq!(a[8_000], 1.0);
        assert_eq!(a[8_002], 0.0);
    }

    #[test]
    fn reessaie_tant_que_le_texte_est_vide() {
        let mut longueurs = Vec::new();
        let r: Result<String, ()> = transcrire_avec_reessais(&[1.0; 10], |a| {
            longueurs.push(a.len());
            Ok(if longueurs.len() < 2 { String::new() } else { "ok".into() })
        });
        assert_eq!(r.unwrap(), "ok");
        assert_eq!(longueurs, vec![16_010, 10]);
    }

    #[test]
    fn s_arrete_au_premier_texte_non_vide() {
        let mut appels = 0;
        let _: Result<String, ()> = transcrire_avec_reessais(&[1.0; 10], |_| {
            appels += 1;
            Ok("bonjour".into())
        });
        assert_eq!(appels, 1);
    }

    #[test]
    fn assemble_en_ignorant_les_vides() {
        let m = vec![" Bonjour.".to_string(), "".to_string(), "Au revoir. ".to_string()];
        assert_eq!(assembler(&m), "Bonjour. Au revoir.");
    }
}
