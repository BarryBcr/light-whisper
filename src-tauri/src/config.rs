use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Réglages persistés dans `config.json`.
/// `#[serde(default)]` : un fichier d'une version antérieure, sans les nouveaux champs,
/// se charge avec leurs valeurs par défaut au lieu d'être rejeté.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub audio_device: String,
    pub model_size: String,
    pub language: String,
    pub engine: String,
    pub shortcut: String,
    /// Remettre l'ancien presse-papier après le collage. Désactivé par défaut : la remise
    /// trop rapide faisait coller l'ancien contenu dans les apps lentes (voir paste.rs).
    pub restaurer_presse_papier: bool,
    /// Délai avant la remise, si elle est activée.
    pub delai_restauration_ms: u64,
    /// Transcrire pendant l'enregistrement, morceau par morceau, à chaque pause.
    /// À l'arrêt, il ne reste que le dernier morceau à traiter (voir decoupe.rs).
    /// Activé par défaut : banc du 17/11/2026 sur M2 8 Go, un bloc de 60 s prend 4 s,
    /// un bloc de 180 s prend 35 s et la fin du texte est inventée.
    pub transcription_par_morceaux: bool,
}

/// Variante « Dev » : installée à côté de l'original, avec son propre dossier de
/// réglages et un autre raccourci, pour que les deux apps ne se marchent pas dessus.
/// Les modèles restent partagés (1 Go de Parakeet, inutile de le dupliquer).
#[cfg(feature = "variante-dev")]
const NOM_DOSSIER_REGLAGES: &str = "lightwhisper-dev";
#[cfg(not(feature = "variante-dev"))]
const NOM_DOSSIER_REGLAGES: &str = "lightwhisper";

#[cfg(feature = "variante-dev")]
const RACCOURCI_PAR_DEFAUT: &str = "Ctrl+Alt+Space";
#[cfg(not(feature = "variante-dev"))]
const RACCOURCI_PAR_DEFAUT: &str = "Alt+Space";

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio_device: "default".to_string(),
            model_size: "base".to_string(),
            language: "auto".to_string(),
            engine: if cfg!(feature = "variante-dev") { "parakeet" } else { "whisper" }.to_string(),
            shortcut: RACCOURCI_PAR_DEFAUT.to_string(),
            restaurer_presse_papier: false,
            delai_restauration_ms: 1000,
            transcription_par_morceaux: true,
        }
    }
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Cannot find home directory")
}

pub fn config_dir() -> PathBuf {
    home().join(NOM_DOSSIER_REGLAGES)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// Toujours `~/lightwhisper/models`, variante Dev comprise (modèles partagés).
pub fn models_dir() -> PathBuf {
    home().join("lightwhisper").join("models")
}

pub fn parakeet_models_dir() -> PathBuf {
    models_dir().join("parakeet-tdt")
}

/// Dernière dictée, pour ne jamais perdre un texte si le collage échoue.
pub fn chemin_derniere_dictee() -> PathBuf {
    config_dir().join("derniere-dictee.txt")
}

/// Journal d'une ligne par dictée : durées, collage, focus.
pub fn chemin_journal() -> PathBuf {
    config_dir().join("journal.log")
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    } else {
        AppConfig::default()
    }
}

pub fn save_config_to_disk(config: &AppConfig) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;

    let json =
        serde_json::to_string_pretty(config).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(config_path(), json).map_err(|e| format!("Failed to write config: {}", e))?;
    Ok(())
}

pub fn ensure_dirs() {
    let _ = fs::create_dir_all(config_dir());
    let _ = fs::create_dir_all(models_dir());
    let _ = fs::create_dir_all(parakeet_models_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancien_fichier_sans_nouveaux_champs_se_charge() {
        let ancien = r#"{"audio_device":"Micro MacBook Pro","model_size":"medium","language":"auto","engine":"parakeet","shortcut":"Alt+Space"}"#;
        let cfg: AppConfig = serde_json::from_str(ancien).unwrap();
        assert_eq!(cfg.engine, "parakeet");
        assert!(!cfg.restaurer_presse_papier);
        assert_eq!(cfg.delai_restauration_ms, 1000);
    }
}
