//! Plattformgerechte Ordner fuer die Ablage (Miniaturen) und die Einstellungen.
//!
//! Frueher lag die Ablage auf jedem System unter `$HOME/Library/Caches/Fotowache`.
//! Unter Linux ist das der falsche Ort, und unter Windows gibt es oft gar kein
//! `HOME` — dann landete die Ablage relativ zum Arbeitsordner. Jetzt nimmt jedes
//! System seine uebliche Stelle, nur ueber Umgebungsvariablen (keine weitere
//! Abhaengigkeit):
//!
//! | System  | Ablage                                   | Einstellungen                              |
//! |---------|------------------------------------------|--------------------------------------------|
//! | macOS   | `~/Library/Caches/Fotowache`               | `~/Library/Application Support/Fotowache`    |
//! | Windows | `%LOCALAPPDATA%\Fotowache\Cache`           | `%APPDATA%\Fotowache`                        |
//! | Linux   | `$XDG_CACHE_HOME/fotowache`, sonst `~/.cache/fotowache` | `$XDG_CONFIG_HOME/fotowache`, sonst `~/.config/fotowache` |
//!
//! Der macOS-Pfad der Ablage ist derselbe wie vorher — vorhandene Miniaturen
//! gelten weiter.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum System {
    Mac,
    Windows,
    /// Linux und die uebrigen Unix-Systeme: XDG-Regel.
    Xdg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Art {
    Ablage,
    Einstellungen,
}

fn dieses_system() -> System {
    if cfg!(target_os = "macos") {
        System::Mac
    } else if cfg!(windows) {
        System::Windows
    } else {
        System::Xdg
    }
}

/// Ordner fuer die Ablage (Miniaturen). Nie relativ zum Arbeitsordner: nennt das
/// System keinen, dient der temporaere Ordner — eine Ablage darf verloren gehen.
pub fn ablage() -> PathBuf {
    ordner(dieses_system(), Art::Ablage, &|k: &str| std::env::var_os(k))
        .unwrap_or_else(|| std::env::temp_dir().join("Fotowache"))
}

/// Ordner fuer Einstellungen (z. B. die Sprachwahl). `None`, wenn das System
/// keinen nennt — dann wird eben nichts gespeichert.
pub fn einstellungen() -> Option<PathBuf> {
    ordner(dieses_system(), Art::Einstellungen, &|k: &str| std::env::var_os(k))
}

/// Ordner aus der Zeit vor dem Namen „Fotowache" (bis 2.0: FotoRaw) einmalig
/// umbenennen — Sprachwahl, eigene Profile und Miniaturen gehen mit. Liegt am
/// neuen Ort schon etwas, bleibt alles, wie es ist.
pub fn umziehen() {
    for art in [Art::Ablage, Art::Einstellungen] {
        let Some(neu) = ordner(dieses_system(), art, &|k: &str| std::env::var_os(k)) else {
            continue;
        };
        if let Some((alt, ziel)) = alter_ort(&neu) {
            if alt.is_dir() && !ziel.exists() {
                let _ = std::fs::rename(&alt, &ziel);
            }
        }
    }
}

/// Zum neuen Ordner das Paar (alter Programmordner, neuer Programmordner):
/// der erste Pfadteil, der den Programmnamen traegt.
fn alter_ort(neu: &std::path::Path) -> Option<(PathBuf, PathBuf)> {
    let mut basis = PathBuf::new();
    for teil in neu.components() {
        match teil.as_os_str().to_str() {
            Some("Fotowache") => return Some((basis.join("FotoRaw"), basis.join("Fotowache"))),
            Some("fotowache") => return Some((basis.join("fotoraw"), basis.join("fotowache"))),
            _ => basis.push(teil),
        }
    }
    None
}

/// Die Regel je System — getrennt von der echten Umgebung, damit sie sich fuer
/// jedes System pruefen laesst.
fn ordner(system: System, art: Art, umgebung: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // Leere Werte gelten als nicht gesetzt — sonst entstuende wieder ein
    // relativer Pfad.
    let wert = |k: &str| umgebung(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    // XDG verlangt absolute Pfade; einen relativen Wert soll man uebergehen.
    let xdg = |k: &str| wert(k).filter(|p| p.is_absolute());
    match (system, art) {
        (System::Mac, Art::Ablage) => wert("HOME").map(|h| h.join("Library/Caches/Fotowache")),
        (System::Mac, Art::Einstellungen) => {
            wert("HOME").map(|h| h.join("Library/Application Support/Fotowache"))
        }
        (System::Windows, Art::Ablage) => wert("LOCALAPPDATA").map(|d| d.join("Fotowache").join("Cache")),
        (System::Windows, Art::Einstellungen) => wert("APPDATA").map(|d| d.join("Fotowache")),
        (System::Xdg, Art::Ablage) => xdg("XDG_CACHE_HOME")
            .map(|d| d.join("fotowache"))
            .or_else(|| wert("HOME").map(|h| h.join(".cache").join("fotowache"))),
        (System::Xdg, Art::Einstellungen) => xdg("XDG_CONFIG_HOME")
            .map(|d| d.join("fotowache"))
            .or_else(|| wert("HOME").map(|h| h.join(".config").join("fotowache"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Eine erfundene Umgebung aus Namen und Werten.
    fn umgebung(paare: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> {
        let paare: Vec<(String, OsString)> =
            paare.iter().map(|(n, v)| (n.to_string(), v.as_os_str().to_owned())).collect();
        move |k: &str| paare.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    /// Ein Pfad, der auf DEM System absolut ist, auf dem der Test laeuft. Was
    /// absolut ist, entscheidet `Path::is_absolute` je Plattform: unter Windows
    /// braucht es ein Laufwerk — dort ist „/daten/cache" relativ, und die
    /// XDG-Regel uebergeht ihn zu Recht (so schlug der Test dort fehl).
    fn absolut(teile: &[&str]) -> PathBuf {
        let wurzel = if cfg!(windows) { r"C:\" } else { "/" };
        unter(Path::new(wurzel), teile)
    }

    /// Basis plus Bestandteile, mit dem Trenner der Plattform zusammengesetzt —
    /// so entstehen auch die erwarteten Werte, nie als Zeichenkette mit „/".
    /// Verglichen wird dann ueber die Bestandteile (`PathBuf` ==).
    fn unter(basis: &Path, teile: &[&str]) -> PathBuf {
        teile.iter().fold(basis.to_path_buf(), |p, t| p.join(t))
    }

    #[test]
    fn linux_folgt_der_xdg_regel() {
        let home = absolut(&["home", "nutzer"]);
        let cache = absolut(&["daten", "cache"]);
        let konfig = absolut(&["daten", "konfig"]);
        let mit = umgebung(&[("HOME", &home), ("XDG_CACHE_HOME", &cache), ("XDG_CONFIG_HOME", &konfig)]);
        assert_eq!(ordner(System::Xdg, Art::Ablage, &mit), Some(unter(&cache, &["fotowache"])));
        assert_eq!(ordner(System::Xdg, Art::Einstellungen, &mit), Some(unter(&konfig, &["fotowache"])));

        // Ohne XDG-Variablen: die Vorgaben unter HOME.
        let ohne = umgebung(&[("HOME", &home)]);
        assert_eq!(ordner(System::Xdg, Art::Ablage, &ohne), Some(unter(&home, &[".cache", "fotowache"])));
        assert_eq!(
            ordner(System::Xdg, Art::Einstellungen, &ohne),
            Some(unter(&home, &[".config", "fotowache"]))
        );

        // Leer oder relativ zaehlt wie nicht gesetzt (XDG-Vorschrift).
        let schief = umgebung(&[
            ("HOME", &home),
            ("XDG_CACHE_HOME", Path::new("")),
            ("XDG_CONFIG_HOME", &unter(Path::new("relativ"), &["konfig"])),
        ]);
        assert_eq!(ordner(System::Xdg, Art::Ablage, &schief), Some(unter(&home, &[".cache", "fotowache"])));
        assert_eq!(
            ordner(System::Xdg, Art::Einstellungen, &schief),
            Some(unter(&home, &[".config", "fotowache"]))
        );

        // Gar nichts bekannt: kein Ordner — nie der Arbeitsordner.
        let leer = umgebung(&[]);
        assert_eq!(ordner(System::Xdg, Art::Ablage, &leer), None);
        assert_eq!(ordner(System::Xdg, Art::Einstellungen, &leer), None);
    }

    /// Der Mac-Pfad der Ablage muss bleiben, wie er war — sonst waeren alle
    /// vorhandenen Miniaturen auf einen Schlag weg.
    #[test]
    fn mac_ablage_bleibt_wo_sie_war() {
        let home = absolut(&["Users", "nutzer"]);
        let u = umgebung(&[("HOME", &home)]);
        assert_eq!(
            ordner(System::Mac, Art::Ablage, &u),
            Some(unter(&home, &["Library", "Caches", "Fotowache"]))
        );
        assert_eq!(
            ordner(System::Mac, Art::Einstellungen, &u),
            Some(unter(&home, &["Library", "Application Support", "Fotowache"]))
        );
    }

    /// Windows: ohne HOME, mit den eigenen Ordnern des Systems.
    #[test]
    fn windows_nimmt_appdata() {
        let lokal = absolut(&["Users", "nutzer", "AppData", "Local"]);
        let wandernd = absolut(&["Users", "nutzer", "AppData", "Roaming"]);
        let u = umgebung(&[("LOCALAPPDATA", &lokal), ("APPDATA", &wandernd)]);
        assert_eq!(ordner(System::Windows, Art::Ablage, &u), Some(unter(&lokal, &["Fotowache", "Cache"])));
        assert_eq!(ordner(System::Windows, Art::Einstellungen, &u), Some(unter(&wandernd, &["Fotowache"])));
        // HOME allein hilft unter Windows nicht weiter.
        let nur_home = umgebung(&[("HOME", &absolut(&["Users", "nutzer"]))]);
        assert_eq!(ordner(System::Windows, Art::Ablage, &nur_home), None);
    }

    #[test]
    fn alter_ort_nennt_den_programmordner() {
        let home = absolut(&["nutzer"]);
        let neu = unter(&home, &["Library", "Caches", "Fotowache"]);
        assert_eq!(
            alter_ort(&neu),
            Some((unter(&home, &["Library", "Caches", "FotoRaw"]), neu.clone()))
        );
        let neu = unter(&home, &["Fotowache", "Cache"]);
        assert_eq!(
            alter_ort(&neu),
            Some((unter(&home, &["FotoRaw"]), unter(&home, &["Fotowache"])))
        );
        let neu = unter(&home, &[".config", "fotowache"]);
        assert_eq!(alter_ort(&neu), Some((unter(&home, &[".config", "fotoraw"]), neu.clone())));
        assert_eq!(alter_ort(&home), None);
    }
}
