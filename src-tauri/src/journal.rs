//! Journal d'une ligne par dictée (`journal.log` dans le dossier de réglages).
//!
//! Pourquoi : la version d'origine ne mesurait rien. Impossible de savoir si une dictée
//! « arrivée tard » venait de la transcription, du retour de focus ou du collage, ni si
//! « rien ne s'est passé » était un texte vide, une annulation ou un collage perdu.
//! Chaque ligne répond à ces questions, sans données personnelles hormis la longueur
//! du texte (le texte lui-même n'est jamais journalisé).

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default)]
pub struct LigneDictee {
    pub issue: &'static str,
    pub audio_s: f64,
    pub attente_apres_arret_s: f64,
    pub transcription_s: f64,
    pub caracteres: usize,
    pub morceaux: usize,
    pub app_cible_pid: i32,
    pub focus_confirme: bool,
    pub cmd_v_envoye: bool,
    pub ancien_restaure: bool,
}

impl LigneDictee {
    pub fn formater(&self, horodatage: u64) -> String {
        format!(
            "{}\tissue={}\taudio={:.1}s\tattente={:.2}s\ttranscription={:.2}s\tcaracteres={}\tmorceaux={}\tpid={}\tfocus={}\tcmd_v={}\trestaure={}",
            horodatage,
            self.issue,
            self.audio_s,
            self.attente_apres_arret_s,
            self.transcription_s,
            self.caracteres,
            self.morceaux,
            self.app_cible_pid,
            self.focus_confirme,
            self.cmd_v_envoye,
            self.ancien_restaure,
        )
    }
}

/// Ajoute la ligne au journal. Best effort : un journal illisible ne doit jamais
/// bloquer une dictée.
pub fn ecrire(ligne: &LigneDictee) {
    let horodatage = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let texte = ligne.formater(horodatage);
    eprintln!("{}", texte);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crate::config::chemin_journal())
    {
        let _ = writeln!(f, "{}", texte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ligne_contient_les_mesures_utiles() {
        let l = LigneDictee {
            issue: "collee",
            audio_s: 62.3,
            attente_apres_arret_s: 1.234,
            caracteres: 512,
            focus_confirme: true,
            ..Default::default()
        };
        let s = l.formater(1);
        assert!(s.contains("issue=collee"));
        assert!(s.contains("audio=62.3s"));
        assert!(s.contains("attente=1.23s"));
        assert!(s.contains("focus=true"));
    }
}
