//! Bildgenerator — Bilder aus einer Beschreibung, komplett lokal (Bonus).
//!
//! Ein eigenes, optionales Paket neben der lokalen KI: eine Laufzeit fuer
//! Diffusionsmodelle (offenes Projekt, MIT) und ein offenes Bildmodell
//! (Apache-2.0) mit Text-Encoder und Bild-Decoder, zusammen rund 5,2 GB.
//! Gleiche Regeln wie bei `ki_lokal`: feste Versionen, SHA-256 beim Laden,
//! Fortsetzen nach Abbruch. Die Laufzeit laeuft nur fuer die Dauer eines Bildes.
//!
//! Gemessen auf dem Entwicklungsrechner: 1024 x 768 in rund einer Minute.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use crate::ki_lokal::{entpacken, laden, Datei, Fortschritt};
use crate::sprache::{t, tf};

const PAKET_STAND: u32 = 1;

macro_rules! laufzeit {
    ($datei:literal, $sha:literal, $groesse:literal) => {
        Some(Datei {
            name: $datei,
            url: concat!("https://github.com/leejet/stable-diffusion.cpp/releases/download/master-982-0518b34/", $datei), // Kennung: Download-Adresse
            sha256: $sha,
            groesse: $groesse,
        })
    };
}

/// Laufzeit fuer dieses System (nur dort, wo es fertige Pakete gibt).
pub fn laufzeit() -> Option<Datei> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        laufzeit!("sd-master-0518b34-bin-Darwin-macOS-26.6.2-arm64.zip", "b5b995203c1eed71e252d5363237325d6b45b8a64ab14911f3104b57f8c7644a", 36737301) // Kennung: Paketname
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        laufzeit!("sd-master-0518b34-bin-Linux-Ubuntu-24.04-x86_64-vulkan.zip", "8bc413b3e6307fbefdf985cc562f7db1cc4c68e2dc3dca9d69b66ce98b9cbb5a", 36790829) // Kennung: Paketname
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        laufzeit!("sd-master-0518b34-bin-win-vulkan-x64.zip", "c2ab91b33206beba434f739a9131916539a3d6229da153e6e0311013e6308567", 29463840) // Kennung: Paketname
    } else {
        None
    }
}

pub const DIFFUSION: Datei = Datei {
    name: "generator-bild.gguf",
    url: "https://huggingface.co/leejet/FLUX.2-klein-4B-GGUF/resolve/3b1f5a9dc3abb32238b053aeb3d823c30afdacbd/flux-2-klein-4b-Q4_0.gguf", // Kennung: Download-Adresse
    sha256: "d1023499ef3f2f82ff7c50e6778495195c1b6cc34835741778868428111f9ff4",
    groesse: 2460378560,
};

pub const TEXT: Datei = Datei {
    name: "generator-text.gguf",
    url: "https://huggingface.co/unsloth/Qwen3-4B-GGUF/resolve/22c9fc8a8c7700b76a1789366280a6a5a1ad1120/Qwen3-4B-Q4_K_M.gguf", // Kennung: Download-Adresse
    sha256: "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a",
    groesse: 2497281312,
};

pub const DECODER: Datei = Datei {
    name: "generator-decoder.safetensors",
    url: "https://huggingface.co/black-forest-labs/FLUX.2-klein-4B/resolve/e7b7dc27f91deacad38e78976d1f2b499d76a294/vae/diffusion_pytorch_model.safetensors", // Kennung: Download-Adresse
    sha256: "ca70d2202afe6415bdbcb8793ba8cd99fd159cfe6192381504d6c4d3036e0f04",
    groesse: 168120878,
};

pub fn dateien() -> Vec<Datei> {
    let mut v = Vec::new();
    v.extend(laufzeit());
    v.extend([DIFFUSION, TEXT, DECODER]);
    v
}

pub fn paketgroesse() -> u64 {
    dateien().iter().map(|d| d.groesse).sum()
}

/// Eigener Ordner neben der lokalen KI — „KI entfernen" nimmt den Generator nicht
/// mit und umgekehrt. `FOTOWACHE_GENERATOR_ORDNER` setzt ihn um (Pruefstand).
fn ordner() -> Option<PathBuf> {
    if let Some(o) = std::env::var_os("FOTOWACHE_GENERATOR_ORDNER").filter(|o| !o.is_empty()) {
        return Some(PathBuf::from(o));
    }
    crate::ordner::daten().map(|d| d.join("generator"))
}

/// Gibt es den Generator fuer dieses System? Der Text sagt, woran es scheitert.
pub fn verfuegbar() -> Result<(), String> {
    if laufzeit().is_none() {
        return Err(t("Den Bildgenerator gibt es für dieses System nicht als fertiges Paket.").into());
    }
    if cfg!(target_os = "macos") {
        // Das fertige Paket verlangt Systemversion 26 (minos 26.0, gemessen).
        // Einmal fragen: das Generator-Fenster ruft dies bei jedem Bildaufbau
        // auf, und ein Prozessstart je Bild hielt die Oberflaeche an.
        static MAJOR: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let major = *MAJOR.get_or_init(|| {
            Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).split('.').next().and_then(|m| m.trim().parse().ok()))
                .unwrap_or(0)
        });
        if major < 26 {
            return Err(t("Der Bildgenerator braucht eine neuere Systemversion (26 oder neuer).").into());
        }
    }
    if cfg!(target_os = "linux") {
        if let Some((a, b)) = crate::ki_lokal::glibc_version() {
            if (a, b) < (2, 39) {
                return Err(tf("Der Bildgenerator braucht glibc 2.39 oder neuer; hier ist {}.{} installiert.", &[&a, &b]));
            }
        }
    }
    Ok(())
}

pub fn installiert() -> bool {
    let Some(o) = ordner() else { return false };
    let stand = std::fs::read_to_string(o.join("installiert.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| j.get("stand").and_then(|s| s.as_u64()));
    stand == Some(PAKET_STAND as u64)
        && cli().is_some()
        && [DIFFUSION, TEXT, DECODER].iter().all(|d| o.join(d.name).is_file())
}

pub fn installieren(melde: &dyn Fn(Fortschritt), abbruch: &AtomicBool) -> Result<(), String> {
    paket_laden(&dateien(), "installiert.json", PAKET_STAND, melde, abbruch)
}

/// Laedt ein Paket (Laufzeit + Modelle) in den Generator-Ordner. Was schon in
/// voller Groesse da ist, wird uebersprungen — die Laufzeit teilen sich der
/// Generator und der freie Modus.
fn paket_laden(alle: &[Datei], vermerk_name: &str, stand: u32, melde: &dyn Fn(Fortschritt), abbruch: &AtomicBool) -> Result<(), String> {
    verfuegbar()?;
    let o = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    let lz = laufzeit().expect("geprueft");
    std::fs::create_dir_all(&o).map_err(|e| e.to_string())?;
    for (i, d) in alle.iter().enumerate() {
        let ziel = o.join(d.name);
        if ziel.is_file() && std::fs::metadata(&ziel).map(|m| m.len()).unwrap_or(0) == d.groesse {
            continue;
        }
        laden(d, &ziel, &|bytes| melde(Fortschritt::Laedt { datei: i + 1, von: alle.len(), bytes, gesamt: d.groesse }), abbruch)?;
    }
    melde(Fortschritt::Entpackt);
    entpacken(&o.join(lz.name), &o.join("laufzeit"))?;
    if cli().is_none() {
        return Err(t("Die KI-Laufzeit ließ sich nicht entpacken.").into());
    }
    let vermerk = serde_json::json!({"stand": stand, "dateien": alle.iter().map(|d| d.sha256).collect::<Vec<_>>()});
    std::fs::write(o.join(vermerk_name), vermerk.to_string()).map_err(|e| e.to_string())?;
    melde(Fortschritt::Fertig);
    Ok(())
}

/// Das Kommandozeilenprogramm der Laufzeit.
fn cli() -> Option<PathBuf> {
    let name = if cfg!(windows) { "sd-cli.exe" } else { "sd-cli" }; // Kennung: Programmname der Laufzeit
    let basis = ordner()?.join("laufzeit");
    let mut stapel = vec![basis];
    while let Some(d) = stapel.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stapel.push(p);
            } else if p.file_name().is_some_and(|n| n == name) {
                return Some(p);
            }
        }
    }
    None
}

/// Seitenverhaeltnis des erzeugten Bildes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Quer,
    Hoch,
    Quadrat,
}

impl Format {
    pub fn masse(self) -> (u32, u32) {
        match self {
            Format::Quer => (1024, 768),
            Format::Hoch => (768, 1024),
            Format::Quadrat => (1024, 1024),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Quer => t("quer"),
            Format::Hoch => t("hoch"),
            Format::Quadrat => t("quadratisch"),
        }
    }
}

/// Erzeugt ein Bild als PNG nach `ziel`. Dauert rund eine Minute.
pub fn erzeugen(beschreibung: &str, format: Format, seed: u64, ziel: &Path) -> Result<(), String> {
    let programm = cli().ok_or_else(|| t("Der Bildgenerator ist nicht installiert.").to_string())?;
    let o = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    let (w, h) = format.masse();
    let mut cmd = Command::new(&programm);
    cmd.arg("--diffusion-model")
        .arg(o.join(DIFFUSION.name))
        .arg("--vae")
        .arg(o.join(DECODER.name))
        .arg("--llm")
        .arg(o.join(TEXT.name))
        .arg("-p")
        .arg(beschreibung)
        .args(["--cfg-scale", "1.0", "--steps", "4", "-W", &w.to_string(), "-H", &h.to_string()])
        .args(["-s", &seed.to_string()])
        .arg("-o")
        .arg(ziel)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(d) = programm.parent() {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt; // Kennung: Modulpfad
        cmd.creation_flags(0x0800_0000);
    }
    let aus = cmd.output().map_err(|e| tf("Bildgenerator startet nicht: {}", &[&e]))?;
    if !aus.status.success() || !ziel.is_file() {
        let log = String::from_utf8_lossy(&aus.stderr);
        let ende: Vec<&str> = log.lines().rev().take(4).collect();
        return Err(tf("Bildgenerator fehlgeschlagen: {}", &[&ende.into_iter().rev().collect::<Vec<_>>().join(" / ")]));
    }
    Ok(())
}

/// Bearbeitet eine Vorlage nach Beschreibung (z. B. „ein paar Wolken am
/// Himmel") und gibt das Ergebnis in derselben Groesse zurueck. Die Seiten der
/// Vorlage muessen durch 16 teilbar sein. Dauert rund anderthalb Minuten.
pub fn bearbeiten(vorlage: &image::RgbImage, text: &str, seed: u64) -> Result<image::RgbImage, String> {
    let programm = cli().ok_or_else(|| t("Der Bildgenerator ist nicht installiert.").to_string())?;
    let o = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    let tmp = std::env::temp_dir().join(format!("fotowache-vorlage-{}-{seed}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let ein = tmp.join("vorlage.png");
    let aus = tmp.join("ergebnis.png");
    vorlage.save(&ein).map_err(|e| e.to_string())?;
    // Der Auftrag sagt ausdruecklich, dass der Rest bleiben soll — sonst
    // gestaltet das Modell gern das ganze Bild um.
    let auftrag = format!("{text}. Keep everything else in the photo exactly as it is, same framing, same colours, photorealistic.");
    let mut cmd = Command::new(&programm);
    cmd.arg("--diffusion-model")
        .arg(o.join(DIFFUSION.name))
        .arg("--vae")
        .arg(o.join(DECODER.name))
        .arg("--llm")
        .arg(o.join(TEXT.name))
        .arg("-r")
        .arg(&ein)
        .arg("-p")
        .arg(&auftrag)
        .args(["--cfg-scale", "1.0", "--steps", "4", "--sampling-method", "euler"])
        .args(["-W", &vorlage.width().to_string(), "-H", &vorlage.height().to_string()])
        .args(["-s", &seed.to_string()])
        .arg("-o")
        .arg(&aus)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(d) = programm.parent() {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt; // Kennung: Modulpfad
        cmd.creation_flags(0x0800_0000);
    }
    let lauf = cmd.output().map_err(|e| tf("Bildgenerator startet nicht: {}", &[&e]))?;
    let ergebnis = image::open(&aus).map(|b| b.to_rgb8());
    let _ = std::fs::remove_dir_all(&tmp);
    match ergebnis {
        Ok(b) if lauf.status.success() => Ok(b),
        _ => {
            let log = String::from_utf8_lossy(&lauf.stderr);
            let ende: Vec<&str> = log.lines().rev().take(4).collect();
            Err(tf("Bildgenerator fehlgeschlagen: {}", &[&ende.into_iter().rev().collect::<Vec<_>>().join(" / ")]))
        }
    }
}

// --- Freier Modus (ab 18) ----------------------------------------------------
//
// Ein zweites, offenes Bildmodell (Apache-2.0), das keinen Inhaltsfilter
// mitbringt — fuer Erwachsene, nach Bestaetigung des Alters. Feste Grenzen,
// die sich nicht abschalten lassen:
//  * nichts mit Minderjaehrigen: jede Beschreibung wird vor dem Start geprueft
//    (`frei_gesperrt`), und dem Modell wird immer gesagt, dass alle Personen
//    erwachsen sind;
//  * nur Bilder aus einer Beschreibung — nie auf Grundlage eigener Fotos (kein
//    „Hinzufuegen", keine Vorlage), damit niemand echte Menschen entkleiden kann.
//
// Gemessen auf dem Entwicklungsrechner: 1024 x 768 in rund 3,5 Minuten.

const FREI_STAND: u32 = 1;

pub const FREI_BILD: Datei = Datei {
    name: "frei-bild.gguf",
    url: "https://huggingface.co/silveroxides/Chroma1-Flash-GGUF/resolve/b0a50e822dbd7ade978a76808a395cfb60c8f66f/Chroma1-HD-Flash-Q4_0.gguf", // Kennung: Download-Adresse
    sha256: "801f8037ed9e95cbf045226e4ecb86dbce860f1795512289f097c8fc415de7f1",
    groesse: 5432053920,
};

pub const FREI_TEXT: Datei = Datei {
    name: "frei-text.gguf",
    url: "https://huggingface.co/city96/t5-v1_1-xxl-encoder-gguf/resolve/005a6ea51a7d0b84d677b3e633bb52a8c85a83d9/t5-v1_1-xxl-encoder-Q4_K_M.gguf", // Kennung: Download-Adresse
    sha256: "6be2b0b7e2de7cf2919340c88cb802a103a997ce46c53131cec91958c1db1af4",
    groesse: 2896123072,
};

pub const FREI_DECODER: Datei = Datei {
    name: "frei-decoder.safetensors",
    url: "https://huggingface.co/lodestones/Chroma1-HD/resolve/0e0c60ece1e82b17cb7f77342d765ba5024c40c0/vae/diffusion_pytorch_model.safetensors", // Kennung: Download-Adresse
    sha256: "f5b59a26851551b67ae1fe58d32e76486e1e812def4696a4bea97f16604d40a3",
    groesse: 167666902,
};

pub fn frei_dateien() -> Vec<Datei> {
    let mut v = Vec::new();
    v.extend(laufzeit());
    v.extend([FREI_BILD, FREI_TEXT, FREI_DECODER]);
    v
}

pub fn frei_paketgroesse() -> u64 {
    frei_dateien().iter().map(|d| d.groesse).sum()
}

pub fn frei_installiert() -> bool {
    let Some(o) = ordner() else { return false };
    let stand = std::fs::read_to_string(o.join("frei.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| j.get("stand").and_then(|s| s.as_u64()));
    stand == Some(FREI_STAND as u64)
        && cli().is_some()
        && [FREI_BILD, FREI_TEXT, FREI_DECODER].iter().all(|d| o.join(d.name).is_file())
}

pub fn frei_installieren(melde: &dyn Fn(Fortschritt), abbruch: &AtomicBool) -> Result<(), String> {
    if !volljaehrig_bestaetigt() {
        return Err(t("Der freie Modus braucht zuerst die Bestätigung, dass du volljährig bist.").into());
    }
    paket_laden(&frei_dateien(), "frei.json", FREI_STAND, melde, abbruch)
}

/// Entfernt nur das freie Bildmodell; der Generator bleibt.
pub fn frei_entfernen() -> Result<(), String> {
    if let Some(o) = ordner() {
        for d in [FREI_BILD, FREI_TEXT, FREI_DECODER] {
            let _ = std::fs::remove_file(o.join(d.name));
            let _ = std::fs::remove_file(o.join(d.name).with_extension("teil"));
        }
        let _ = std::fs::remove_file(o.join("frei.json"));
    }
    Ok(())
}

fn bestaetigung_datei() -> Option<PathBuf> {
    crate::ordner::einstellungen().map(|o| o.join("freier-modus.json"))
}

/// Wurde die Volljaehrigkeit auf diesem Rechner bestaetigt? Gespeichert wird nur
/// DASS und WANN — nie das Geburtsdatum.
pub fn volljaehrig_bestaetigt() -> bool {
    bestaetigung_datei()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|j| j.get("volljaehrig").and_then(|v| v.as_bool()) == Some(true))
}

/// Prueft ein Geburtsdatum gegen das heutige Datum und vermerkt die
/// Bestaetigung. Eine Ausweispruefung ist in einem Programm ohne Server nicht
/// moeglich — es ist eine Selbstauskunft wie auf den ueblichen Modellseiten.
pub fn volljaehrigkeit_bestaetigen(tag: u32, monat: u32, jahr: i32) -> Result<(), String> {
    let heute = heute();
    if !(1..=31).contains(&tag) || !(1..=12).contains(&monat) || jahr < 1900 || (jahr, monat, tag) > heute {
        return Err(t("Bitte ein gültiges Geburtsdatum eingeben (TT.MM.JJJJ).").into());
    }
    if alter((jahr, monat, tag), heute) < 18 {
        return Err(t("Der freie Modus ist erst ab 18 Jahren verfügbar.").into());
    }
    let p = bestaetigung_datei().ok_or_else(|| t("Kein Ordner für Einstellungen gefunden.").to_string())?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let (j, m, t) = heute;
    let vermerk = serde_json::json!({"volljaehrig": true, "bestaetigt_am": format!("{j:04}-{m:02}-{t:02}")});
    std::fs::write(p, vermerk.to_string()).map_err(|e| e.to_string())
}

/// Bestaetigung zuruecknehmen — der freie Modus ist dann wieder gesperrt.
pub fn bestaetigung_zuruecknehmen() {
    if let Some(p) = bestaetigung_datei() {
        let _ = std::fs::remove_file(p);
    }
}

/// Volle Jahre zwischen Geburtstag und heute, beides (Jahr, Monat, Tag).
fn alter(geburt: (i32, u32, u32), heute: (i32, u32, u32)) -> i32 {
    let mut a = heute.0 - geburt.0;
    if (heute.1, heute.2) < (geburt.1, geburt.2) {
        a -= 1;
    }
    a
}

/// Heutiges Datum (UTC reicht fuer eine Altersgrenze in Jahren).
fn heute() -> (i32, u32, u32) {
    let tage = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    // Kalenderdatum aus Tagen seit 1970 (Verfahren nach H. Hinnant).
    let z = tage + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let tag = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let monat = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let jahr = (yoe + era * 400 + if monat <= 2 { 1 } else { 0 }) as i32;
    (jahr, monat, tag)
}

/// Woerter, die im freien Modus nie in einer Beschreibung stehen duerfen — alles,
/// was auf Kinder oder Jugendliche zielt, in den haeufigsten Sprachen. Bewusst
/// streng: im freien Modus beschreibt man Erwachsene als Frau oder Mann.
const GESPERRT: &[&str] = &[
    // Englisch
    "child", "children", "childs", "kid", "kids", "kiddie", "minor", "minors", "underage", "teen", "teens",
    "teenage", "teenager", "teenagers", "preteen", "pre-teen", "tween", "tweens", "girl", "girls", "boy", "boys",
    "schoolgirl", "schoolgirls", "schoolboy", "schoolboys", "toddler", "toddlers", "infant", "infants", "baby",
    "babies", "juvenile", "juveniles", "adolescent", "adolescents", "youngster", "youngsters", "loli", "lolis",
    "lolita", "shota", "shotacon", "lolicon", "jailbait", "pedo", "pedophile", "paedophile", "cp", "young",
    "younger", "youthful", "youth", "little", "petite", "student", "students", "pupil", "pupils", "daughter",
    "son", "niece", "nephew", "puberty", "prepubescent", "pubescent", "kindergarten", "elementary", "highschool",
    "middleschool", "schoolkid", "chibi",
    // Deutsch
    "kind", "kinder", "kindes", "kindlich", "kindliche", "kindlichen", "kleinkind", "säugling", "baby", "babys",
    "mädchen", "maedchen", "junge", "jungen", "bub", "buben", "teenie", "teenies", "jugendlich", "jugendliche",
    "jugendlichen", "jugendlicher", "minderjährig", "minderjährige", "minderjährigen", "minderjaehrig",
    "schüler", "schülerin", "schülerinnen", "schueler", "schuelerin", "tochter", "sohn", "nichte", "neffe",
    "pubertät", "pubertaet", "grundschule", "kita", "jung", "junges", "junger", "jüngere", "klein", "kleine",
    "kleines", "kleiner",
    // Französisch, Spanisch, Italienisch, Niederländisch, Polnisch
    "enfant", "enfants", "fille", "fillette", "garçon", "ado", "ados", "adolescente", "niño", "niña", "niños",
    "niñas", "chica", "chico", "menor", "menores", "bambino", "bambina", "bambini", "ragazza", "ragazzo",
    "meisje", "jongen", "kinderen", "dziecko", "dzieci", "dziewczynka", "chłopiec", "nastolatka", "nastolatek",
];

/// Zeiteinheiten, die nach einer Zahl ein Alter angeben.
const ALTERSANGABE: &[&str] = &[
    "y", "yo", "yrs", "yr", "year", "years", "jahr", "jahre", "jahren", "jährig", "jährige", "jähriger",
    "jährigen", "ans", "an", "años", "anni", "jaar", "lat", "lata",
];

/// Steht in der Beschreibung etwas, das im freien Modus gesperrt ist? Prueft
/// Woerter (nicht Wortteile — „cowboy" bleibt erlaubt, „boy" nicht) und
/// Altersangaben unter 18 („16 years", „15-jährige", „14yo").
pub fn frei_gesperrt(text: &str) -> bool {
    let klein = text.to_lowercase();
    let woerter: Vec<&str> = klein
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .flat_map(|w| std::iter::once(w).chain(w.split('-')))
        .filter(|w| !w.is_empty())
        .collect();
    if woerter.iter().any(|w| GESPERRT.contains(w)) {
        return true;
    }
    for (i, w) in woerter.iter().enumerate() {
        // Zahl mit angehaengter Einheit („14yo", „16jährige") oder Zahl + Einheit.
        let ziffern: String = w.chars().take_while(|c| c.is_ascii_digit()).collect();
        if ziffern.is_empty() {
            continue;
        }
        let Ok(n) = ziffern.parse::<u32>() else { continue };
        if n >= 18 {
            continue;
        }
        let rest = &w[ziffern.len()..];
        let folgt = woerter.get(i + 1).copied().unwrap_or("");
        if ALTERSANGABE.contains(&rest) || (rest.is_empty() && ALTERSANGABE.contains(&folgt)) {
            return true;
        }
    }
    false
}

/// Woerter, mit denen jemand ausdruecklich KEIN Foto will (de/en/fr/es/it/nl/pl).
const ZEICHENSTIL: &[&str] = &[
    "comic", "comics", "manga", "anime", "cartoon", "zeichnung", "gezeichnet", "illustration", "illustriert",
    "gemälde", "gemalt", "malerei", "aquarell", "ölbild", "skizze", "drawing", "drawn", "painting", "painted",
    "watercolor", "watercolour", "sketch", "3d", "pixelart", "dessin", "peinture", "dibujo", "pintura",
    "acuarela", "disegno", "pittura", "fumetto", "tekening", "schilderij", "rysunek", "kreskówka",
];

/// Der Auftrag an das freie Modell. Ohne Stilangabe malt es (gemessen: deutsche
/// wie englische Beschreibungen kamen als Illustration heraus, mit Foto-Vorgabe
/// als Foto) — also ist ein Foto der Standard, ausser die Beschreibung
/// verlangt ausdruecklich einen Zeichenstil.
pub fn frei_auftrag(beschreibung: &str) -> String {
    let klein = beschreibung.to_lowercase();
    let gezeichnet = klein
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| ZEICHENSTIL.contains(&w) || w.starts_with("comic") || w.starts_with("zeichn"));
    if gezeichnet {
        return beschreibung.trim().to_string();
    }
    format!(
        "Photograph, shot on a full-frame camera with a 50mm lens, natural light, realistic skin texture, \
         film grain, true-to-life colours. {}",
        beschreibung.trim()
    )
}

/// Erzeugt ein Bild mit dem freien Modell. Gesperrte Beschreibungen kommen gar
/// nicht erst bei der Laufzeit an.
pub fn frei_erzeugen(beschreibung: &str, format: Format, seed: u64, ziel: &Path) -> Result<(), String> {
    if !volljaehrig_bestaetigt() {
        return Err(t("Der freie Modus braucht zuerst die Bestätigung, dass du volljährig bist.").into());
    }
    if frei_gesperrt(beschreibung) {
        return Err(t(
            "Diese Beschreibung ist im freien Modus gesperrt: nichts, was auf Kinder oder Jugendliche zielt. \
             Beschreibe Erwachsene (Frau, Mann).",
        )
        .into());
    }
    let programm = cli().ok_or_else(|| t("Der Bildgenerator ist nicht installiert.").to_string())?;
    let o = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    if !frei_installiert() {
        return Err(t("Das freie Bildmodell ist nicht installiert.").into());
    }
    let (w, h) = format.masse();
    // Immer dazu: alle Personen sind Erwachsene. Ein Negativ-Text wuerde bei
    // diesem Modell nur mit doppelter Rechenzeit (und schwach) wirken; die
    // Wortsperre oben ist die eigentliche Grenze.
    let auftrag = format!("{}. All people shown are adults, at least 25 years old.", frei_auftrag(beschreibung));
    let mut cmd = Command::new(&programm);
    cmd.arg("--diffusion-model")
        .arg(o.join(FREI_BILD.name))
        .arg("--vae")
        .arg(o.join(FREI_DECODER.name))
        .arg("--t5xxl")
        .arg(o.join(FREI_TEXT.name))
        .arg("-p")
        .arg(&auftrag)
        .args(["--cfg-scale", "1.0", "--sampling-method", "euler", "--steps", "8"])
        .args(["-W", &w.to_string(), "-H", &h.to_string()])
        .args(["-s", &seed.to_string()])
        .args(["--model-args", "chroma_use_dit_mask=false"]) // Kennung: Schalter der Laufzeit
        .arg("-o")
        .arg(ziel)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(d) = programm.parent() {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt; // Kennung: Modulpfad
        cmd.creation_flags(0x0800_0000);
    }
    let aus = cmd.output().map_err(|e| tf("Bildgenerator startet nicht: {}", &[&e]))?;
    if !aus.status.success() || !ziel.is_file() {
        let log = String::from_utf8_lossy(&aus.stderr);
        let ende: Vec<&str> = log.lines().rev().take(4).collect();
        return Err(tf("Bildgenerator fehlgeschlagen: {}", &[&ende.into_iter().rev().collect::<Vec<_>>().join(" / ")]));
    }
    Ok(())
}

pub fn entfernen() -> Result<(), String> {
    if let Some(o) = ordner() {
        if o.exists() {
            std::fs::remove_dir_all(o).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dateien_sind_festgenagelt() {
        for d in [DIFFUSION, TEXT, DECODER].into_iter().chain(laufzeit()) {
            assert_eq!(d.sha256.len(), 64, "{}", d.name);
            assert!(d.url.contains(if d.name.ends_with(".zip") { "master-982-0518b34" } else { "/resolve/" }));
        }
        assert!(paketgroesse() > 5_000_000_000 || laufzeit().is_none());
    }

    #[test]
    fn freie_dateien_sind_festgenagelt() {
        for d in [FREI_BILD, FREI_TEXT, FREI_DECODER] {
            assert_eq!(d.sha256.len(), 64, "{}", d.name);
            assert!(d.url.contains("/resolve/") && !d.url.contains("/resolve/main/"), "{}", d.name);
        }
    }

    #[test]
    fn sperre_trifft_minderjaehrige_und_nicht_erwachsene() {
        for gesperrt in [
            "a teen on the beach",
            "Schoolgirl in uniform",
            "ein Mädchen am Strand",
            "two kids playing",
            "a 16 year old",
            "eine 15-jährige",
            "14yo model",
            "young woman",
            "LOLI art",
            "petite girl",
            "un niño",
        ] {
            assert!(frei_gesperrt(gesperrt), "muesste gesperrt sein: {gesperrt}");
        }
        for erlaubt in [
            "a nude adult woman, studio light, figure study",
            "a cowboy at sunset",
            "zwei Erwachsene am Strand, 35 Jahre alt",
            "portrait of a 40 year old man",
            "a red bicycle against a stone wall",
            "a woman in her thirties",
        ] {
            assert!(!frei_gesperrt(erlaubt), "faelschlich gesperrt: {erlaubt}");
        }
    }

    #[test]
    fn freier_auftrag_ist_ein_foto_ausser_auf_wunsch() {
        assert!(frei_auftrag("Eine Frau im Café").starts_with("Photograph"));
        assert!(frei_auftrag("A man at the harbour").starts_with("Photograph"));
        assert_eq!(frei_auftrag("Ein Comic von einer Katze"), "Ein Comic von einer Katze");
        assert_eq!(frei_auftrag("watercolor painting of a lake"), "watercolor painting of a lake");
        assert_eq!(frei_auftrag("Zeichnung eines Hauses"), "Zeichnung eines Hauses");
        assert!(frei_auftrag("Comicverlag im Altbau").starts_with("Comic"));
    }

    #[test]
    fn alter_in_vollen_jahren() {
        assert_eq!(alter((2008, 10, 22), (2026, 10, 21)), 17);
        assert_eq!(alter((2008, 10, 21), (2026, 10, 21)), 18);
        assert_eq!(alter((1980, 1, 1), (2026, 10, 21)), 46);
        let (j, m, t) = heute();
        assert!(j >= 2026 && (1..=12).contains(&m) && (1..=31).contains(&t));
    }

    #[test]
    fn formate_sind_durch_16_teilbar() {
        for f in [Format::Quer, Format::Hoch, Format::Quadrat] {
            let (w, h) = f.masse();
            assert!(w % 16 == 0 && h % 16 == 0);
        }
    }
}
