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
        let v = Command::new("sw_vers").arg("-productVersion").output().ok();
        let major: u32 = v
            .and_then(|o| String::from_utf8_lossy(&o.stdout).split('.').next().and_then(|m| m.trim().parse().ok()))
            .unwrap_or(0);
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
    verfuegbar()?;
    let o = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    let lz = laufzeit().expect("geprueft");
    std::fs::create_dir_all(&o).map_err(|e| e.to_string())?;
    let alle = dateien();
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
    let vermerk = serde_json::json!({"stand": PAKET_STAND, "dateien": alle.iter().map(|d| d.sha256).collect::<Vec<_>>()});
    std::fs::write(o.join("installiert.json"), vermerk.to_string()).map_err(|e| e.to_string())?;
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
    fn formate_sind_durch_16_teilbar() {
        for f in [Format::Quer, Format::Hoch, Format::Quadrat] {
            let (w, h) = f.masse();
            assert!(w % 16 == 0 && h % 16 == 0);
        }
    }
}
