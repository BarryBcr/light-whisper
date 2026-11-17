//! Banc de vitesse de la transcription Parakeet, hors de l'app.
//!
//! Pourquoi ce banc existe : l'app ne chronomètre rien, et la question « pourquoi la
//! dictée arrive tard » ne se tranche qu'avec des chiffres par durée d'audio. Il reprend
//! les appels de `src-tauri/src/stt.rs` (from_pretrained + transcribe_samples sur tout
//! le bloc) et, en option, la transcription par morceaux de `src-tauri/src/decoupe.rs`.
//!
//! Usage :
//!   banc-transcription <dossier_modele> <fichier.wav> [--threads 4] [--coreml]
//!                      [--durees 10,60,180,300] [--morceaux]
//!
//! Sortie : une ligne TSV par durée (durée audio, temps 1re et 2e transcription,
//! nombre de mots, premiers et derniers mots pour repérer une troncature).

use parakeet_rs::{ExecutionConfig, ParakeetTDT, Transcriber};

// Le découpage exact de l'app, inclus tel quel (fonctions pures).
#[path = "../../src-tauri/src/coupure.rs"]
#[allow(dead_code)]
mod coupure;
use std::time::Instant;

fn lire_wav_16k_mono(chemin: &str) -> Vec<f32> {
    let mut lecteur = hound::WavReader::open(chemin).expect("WAV illisible");
    let spec = lecteur.spec();
    assert_eq!(spec.sample_rate, 16_000, "le banc attend du 16 kHz");
    assert_eq!(spec.channels, 1, "le banc attend du mono");
    lecteur
        .samples::<i16>()
        .map(|s| s.expect("échantillon illisible") as f32 / 32_768.0)
        .collect()
}

fn argument(args: &[String], nom: &str) -> Option<String> {
    args.iter()
        .position(|a| a == nom)
        .and_then(|i| args.get(i + 1).cloned())
}

fn bornes_mots(texte: &str) -> (usize, String, String) {
    let mots: Vec<&str> = texte.split_whitespace().collect();
    let debut = mots.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
    let fin = mots
        .iter()
        .rev()
        .take(4)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    (mots.len(), debut, fin)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dossier_modele = args.get(1).expect("dossier du modèle manquant");
    let fichier_wav = args.get(2).expect("fichier WAV manquant");
    let threads: usize = argument(&args, "--threads")
        .map(|t| t.parse().expect("--threads entier"))
        .unwrap_or(4);
    let durees: Vec<usize> = argument(&args, "--durees")
        .unwrap_or_else(|| "10,60,180,300".to_string())
        .split(',')
        .map(|d| d.parse().expect("durée entière en secondes"))
        .collect();

    let mut config = ExecutionConfig::new().with_intra_threads(threads);
    if args.iter().any(|a| a == "--coreml") {
        #[cfg(feature = "coreml")]
        {
            config = config
                .with_execution_provider(parakeet_rs::ExecutionProvider::CoreML)
                .with_coreml_cache_dir(std::env::temp_dir().join("banc-coreml-cache"));
        }
        #[cfg(not(feature = "coreml"))]
        panic!("recompiler avec --features coreml");
    }

    let audio = lire_wav_16k_mono(fichier_wav);
    let t0 = Instant::now();
    let mut modele =
        ParakeetTDT::from_pretrained(dossier_modele, Some(config)).expect("chargement modèle");
    eprintln!("chargement modèle : {:.2} s", t0.elapsed().as_secs_f64());

    // Diagnostic : transcrire une seule plage, sur un modèle neuf, plusieurs fois.
    if let Some(plage) = argument(&args, "--segment") {
        let (a, b) = plage.split_once(',').expect("--segment debut,fin en échantillons");
        let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
        let morceau = audio[a..b].to_vec();
        for essai in 1..=3 {
            let r = modele.transcribe_samples(morceau.clone(), 16_000, 1, None).unwrap();
            println!("essai {essai} : [{}]", r.text);
        }
        let mut avec_marge = vec![0.0f32; 8_000];
        avec_marge.extend_from_slice(&morceau);
        avec_marge.extend(vec![0.0f32; 8_000]);
        let r = modele.transcribe_samples(avec_marge, 16_000, 1, None).unwrap();
        println!("avec 0,5 s de silence autour : [{}]", r.text);
        return;
    }

    if args.iter().any(|a| a == "--morceaux") {
        banc_morceaux(&mut modele, &audio, &durees, argument(&args, "--sortie"));
        return;
    }

    println!("duree_s\t1re_s\t2e_s\tmots\tdebut\tfin");
    for duree in durees {
        let n = (duree * 16_000).min(audio.len());
        let extrait = audio[..n].to_vec();
        let mut temps = Vec::new();
        let mut texte = String::new();
        for _ in 0..2 {
            let t = Instant::now();
            let resultat = modele
                .transcribe_samples(extrait.clone(), 16_000, 1, None)
                .expect("transcription");
            temps.push(t.elapsed().as_secs_f64());
            texte = resultat.text.trim().to_string();
        }
        if let Some(dossier) = argument(&args, "--sortie") {
            let _ = std::fs::write(format!("{}/bloc-{}s.txt", dossier, duree), &texte);
        }
        let (mots, debut, fin) = bornes_mots(&texte);
        println!(
            "{}\t{:.2}\t{:.2}\t{}\t{}\t{}",
            n / 16_000,
            temps[0],
            temps[1],
            mots,
            debut,
            fin
        );
    }
}

/// Rejoue la transcription au fil de l'eau de l'app : le temps avance par pas de 0,4 s,
/// on coupe dès que `coupure::trouver_coupure` le permet, puis on mesure ce qu'il reste
/// à transcrire après l'arrêt. Vérifie aussi que chaque morceau est transcrit plus vite
/// qu'il ne dure (sinon le retard s'accumulerait pendant la dictée).
fn banc_morceaux(modele: &mut ParakeetTDT, audio: &[f32], durees: &[usize], sortie: Option<String>) {
    println!("duree_s\tmorceaux\tpire_morceau_s\tattente_apres_arret_s\tmots\tdebut\tfin");
    for &duree in durees {
        let n = (duree * 16_000).min(audio.len());
        let extrait = &audio[..n];
        let mut position = 0usize;
        let mut maintenant = 0usize;
        let mut textes = Vec::new();
        let mut pire = 0f64;
        while maintenant < n {
            maintenant = (maintenant + 6_400).min(n); // 0,4 s
            if let Some(c) = coupure::trouver_coupure(&extrait[position..maintenant], 16_000) {
                let t = Instant::now();
                let texte = transcrire(modele, &extrait[position..position + c]);
                pire = pire.max(t.elapsed().as_secs_f64());
                if std::env::var("BANC_DETAIL").is_ok() {
                    eprintln!(
                        "[{}-{} ech] {}",
                        position,
                        position + c,
                        texte
                    );
                }
                textes.push(texte);
                position += c;
            }
        }
        let t = Instant::now();
        let reste = &extrait[position..];
        if reste.len() >= coupure::ECHANTILLONS_MIN_16K {
            textes.push(transcrire(modele, reste));
        }
        let attente = t.elapsed().as_secs_f64();
        let texte = coupure::assembler(&textes);
        if let Some(dossier) = &sortie {
            let _ = std::fs::write(format!("{}/morceaux-{}s.txt", dossier, duree), &texte);
        }
        let (mots, debut, fin) = bornes_mots(&texte);
        println!(
            "{}\t{}\t{:.2}\t{:.2}\t{}\t{}\t{}",
            n / 16_000,
            textes.len(),
            pire,
            attente,
            mots,
            debut,
            fin
        );
    }
}

/// Même appel que `ParakeetEngine::transcribe` dans l'app (réessais compris).
fn transcrire(modele: &mut ParakeetTDT, audio: &[f32]) -> String {
    coupure::transcrire_avec_reessais(audio, |a| {
        modele.transcribe_samples(a, 16_000, 1, None).map(|r| r.text)
    })
    .expect("transcription")
}
