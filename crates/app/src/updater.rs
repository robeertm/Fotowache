//! Updater — neue Versionen mit einem Klick installieren.
//!
//! Beim Start fragt Fotowache im Hintergrund nach der neuesten Version (einmal,
//! ohne Daten ausser der Anfrage selbst). Gibt es eine neuere, erscheint unten
//! in der Statuszeile ein Knopf; ein Klick laedt das passende Paket, prueft es
//! gegen die Pruefsummen des Releases und tauscht das Programm aus.
//!
//! ## Wem gehoeren die Dateien?
//! Ein Updater, der Dateien tauscht, muss wissen, wem sie gehoeren — sonst
//! meldet er Erfolg und nichts hat sich geaendert. Deshalb je Installationsart:
//! - **App-Paket** (`…/Fotowache.app`): neues Paket entpacken, altes ersetzen,
//!   neu starten. Nur wenn der Ordner beschreibbar ist.
//! - **Installer-Installation** (Programmordner des Benutzers): der Installer
//!   laeuft still, schliesst Fotowache und startet es danach wieder.
//! - **AppImage**: die Datei selbst wird ersetzt.
//! - **Installation per Befehl** (`~/.local/opt/fotowache`): der Ordner wird ersetzt.
//! - **Entwicklungsbau** (aus einem `target`-Ordner): kein Updater — dort
//!   gehoeren die Dateien dem Bauwerkzeug.
//! `FOTOWACHE_KEINE_UPDATES=1` schaltet die Pruefung ab.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::ki_lokal::laden_von;
use crate::sprache::{t, tf};

const REPO: &str = "robeertm/Fotowache";

/// Eine verfuegbare neuere Version.
#[derive(Debug, Clone)]
pub struct Neu {
    pub version: String,
    pub paket: String,
    pub url: String,
    pub groesse: u64,
    pub sha256: String,
}

/// Wie diese Kopie installiert ist.
#[derive(Debug, Clone, PartialEq)]
pub enum Art {
    AppPaket(PathBuf),
    Installer,
    AppImage(PathBuf),
    Ordner(PathBuf),
}

/// Ermittelt die Installationsart — `None` heisst: hier wird nicht aktualisiert.
pub fn art() -> Option<Art> {
    if std::env::var_os("FOTOWACHE_KEINE_UPDATES").is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        return None; // Entwicklungsbau
    }
    if cfg!(target_os = "macos") {
        let app = exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app"))?.to_path_buf();
        let eltern = app.parent()?;
        return beschreibbar(eltern).then_some(Art::AppPaket(app));
    }
    if cfg!(windows) {
        // Nur die Installation im Programmordner des Benutzers (Installer oder
        // Installationsbefehl, beide legen sie dort an).
        let lokal = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        let soll = lokal.join("Programs").join("Fotowache");
        return exe.parent().is_some_and(|d| d.eq_ignore_case(&soll)).then_some(Art::Installer);
    }
    if let Some(a) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        return a.parent().is_some_and(beschreibbar).then_some(Art::AppImage(a));
    }
    let ordner = exe.parent()?.to_path_buf();
    (ordner.join("fotowache.desktop").is_file() && beschreibbar(&ordner)).then_some(Art::Ordner(ordner))
}

trait GleichOhneGross {
    fn eq_ignore_case(&self, o: &Path) -> bool;
}
impl GleichOhneGross for Path {
    fn eq_ignore_case(&self, o: &Path) -> bool {
        self.to_string_lossy().to_lowercase() == o.to_string_lossy().to_lowercase()
    }
}

fn beschreibbar(d: &Path) -> bool {
    let probe = d.join(format!(".fotowache-schreibprobe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

/// Paketname-Endung fuer diese Kopie.
fn endung(art: &Art) -> Option<&'static str> {
    Some(match art {
        Art::AppPaket(_) if cfg!(target_arch = "aarch64") => "-macos-arm64.zip", // Kennung: Paketname
        Art::AppPaket(_) => "-macos-x86_64.zip", // Kennung: Paketname
        Art::Installer => "-windows-x64-setup.exe", // Kennung: Paketname
        Art::AppImage(_) if cfg!(target_arch = "aarch64") => "-linux-aarch64.AppImage", // Kennung: Paketname
        Art::AppImage(_) => "-linux-x86_64.AppImage", // Kennung: Paketname
        Art::Ordner(_) if cfg!(target_arch = "aarch64") => "-linux-aarch64.tar.gz", // Kennung: Paketname
        Art::Ordner(_) => "-linux-x86_64.tar.gz", // Kennung: Paketname
    })
}

/// Versionen vergleichen ("2.10.0" > "2.9.3").
pub fn neuer(kandidat: &str, aktuell: &str) -> bool {
    let teile = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v').split(['.', '-']).map(|t| t.parse().unwrap_or(0)).take(3).collect()
    };
    teile(kandidat) > teile(aktuell)
}

fn get(url: &str) -> Result<String, String> {
    ureq::get(url)
        .header("User-Agent", "Fotowache-Updater")
        .header("Accept", "application/vnd.github+json")
        .config()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Fragt nach der neuesten Version. `Ok(None)` = aktuell.
pub fn pruefen() -> Result<Option<Neu>, String> {
    let Some(art) = art() else { return Ok(None) };
    let text = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?; // Kennung: Release-Schnittstelle
    let j: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let version = j["tag_name"].as_str().unwrap_or_default().trim_start_matches('v').to_string();
    // Pruefstand: eine Kopie kann sich als aeltere Fassung ausgeben, um den
    // ganzen Weg gegen ein echtes Release zu pruefen.
    let aktuell = std::env::var("FOTOWACHE_UPDATE_PRUEFSTAND_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    if !neuer(&version, &aktuell) {
        return Ok(None);
    }
    let endung = endung(&art).ok_or("unbekannte Installationsart")?;
    let assets = j["assets"].as_array().cloned().unwrap_or_default();
    let finde = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().find(|a| a["name"].as_str().is_some_and(pred)).cloned()
    };
    let paket = finde(&|n: &str| n.ends_with(endung)).ok_or_else(|| tf("Im Release fehlt das Paket {}", &[&endung]))?;
    let summen = finde(&|n: &str| n == "SHA256SUMS.txt").ok_or_else(|| t("Im Release fehlen die Prüfsummen").to_string())?;
    let name = paket["name"].as_str().unwrap_or_default().to_string();
    let summen_text = get(summen["browser_download_url"].as_str().unwrap_or_default())?;
    let sha256 = summen_text
        .lines()
        .find_map(|z| {
            let mut t = z.split_whitespace();
            let (h, n) = (t.next()?, t.next()?);
            (n.trim_start_matches('*') == name).then(|| h.to_lowercase())
        })
        .ok_or_else(|| tf("Keine Prüfsumme für {}", &[&name]))?;
    Ok(Some(Neu {
        version,
        url: paket["browser_download_url"].as_str().unwrap_or_default().to_string(),
        groesse: paket["size"].as_u64().unwrap_or(0),
        paket: name,
        sha256,
    }))
}

/// Laedt, prueft und installiert. Gibt den Befehl zurueck, mit dem die neue
/// Fassung gestartet wird (der Aufrufer beendet danach Fotowache).
pub fn installieren(neu: &Neu, melde: &dyn Fn(u64, u64), abbruch: &AtomicBool) -> Result<Command, String> {
    let art = art().ok_or_else(|| t("Diese Kopie wird nicht automatisch aktualisiert.").to_string())?;
    let tmp = std::env::temp_dir().join(format!("fotowache-update-{}", neu.version));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let datei = tmp.join(&neu.paket);
    // Groesse aus dem Release, Pruefsumme aus SHA256SUMS.txt — mitgerechnet beim Laden.
    if !datei.is_file() {
        laden_von(&neu.url, &neu.sha256, neu.groesse, &neu.paket, &datei, &|b| melde(b, neu.groesse), abbruch)?;
    }
    match art {
        Art::AppPaket(app) => {
            let aus = tmp.join("entpackt");
            let _ = std::fs::remove_dir_all(&aus);
            let ok = Command::new("ditto").arg("-x").arg("-k").arg(&datei).arg(&aus).status().map_err(|e| e.to_string())?;
            if !ok.success() {
                return Err(t("Entpacken fehlgeschlagen").into());
            }
            let neu_app = std::fs::read_dir(&aus)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "app"))
                .ok_or_else(|| t("Im Paket fehlt das Programm").to_string())?;
            ersetzen(&neu_app, &app)?;
            let _ = std::fs::remove_dir_all(&tmp);
            let mut c = Command::new("open");
            c.arg("-n").arg(&app);
            Ok(c)
        }
        Art::Installer => {
            // Der Installer laeuft erst nach dem Beenden — sein Ordner bleibt
            // stehen (temporaerer Ordner des Systems).
            let mut c = Command::new(&datei);
            c.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/STARTEN=1"]);
            Ok(c)
        }
        Art::AppImage(ziel) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&datei, std::fs::Permissions::from_mode(0o755));
            }
            let zwischen = ziel.with_extension("neu");
            std::fs::copy(&datei, &zwischen).map_err(|e| e.to_string())?;
            std::fs::rename(&zwischen, &ziel).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(Command::new(&ziel))
        }
        Art::Ordner(ordner) => {
            let aus = tmp.join("entpackt");
            let _ = std::fs::remove_dir_all(&aus);
            std::fs::create_dir_all(&aus).map_err(|e| e.to_string())?;
            let ok = Command::new("tar")
                .arg("xzf")
                .arg(&datei)
                .arg("-C")
                .arg(&aus)
                .arg("--strip-components=1")
                .status()
                .map_err(|e| e.to_string())?;
            if !ok.success() || !aus.join("fotowache").is_file() {
                return Err(t("Entpacken fehlgeschlagen").into());
            }
            ersetzen(&aus, &ordner)?;
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(Command::new(ordner.join("fotowache")))
        }
    }
}

/// Ersetzt `ziel` durch `quelle` (Ordner): erst daneben verschieben, dann
/// tauschen, dann das Alte loeschen — ein Abbruch mittendrin laesst nie ein
/// halbes Programm zurueck. Ein laufendes Programm darf so ersetzt werden; es
/// laeuft bis zum Neustart aus dem alten (bereits geoeffneten) Stand weiter.
fn ersetzen(quelle: &Path, ziel: &Path) -> Result<(), String> {
    let alt = ziel.with_extension("alt");
    let _ = std::fs::remove_dir_all(&alt);
    let neu = ziel.with_extension("neu");
    let _ = std::fs::remove_dir_all(&neu);
    // Quelle auf dasselbe Laufwerk holen (rename geht nicht ueber Laufwerke).
    kopieren(quelle, &neu).map_err(|e| e.to_string())?;
    std::fs::rename(ziel, &alt).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&neu, ziel) {
        let _ = std::fs::rename(&alt, ziel);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&alt);
    Ok(())
}

fn kopieren(von: &Path, nach: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        // ditto behaelt Rechte, Verweise und die Signatur des Pakets.
        let ok = Command::new("ditto").arg(von).arg(nach).status()?;
        return if ok.success() { Ok(()) } else { Err(std::io::Error::other("ditto")) };
    }
    std::fs::create_dir_all(nach)?;
    for e in std::fs::read_dir(von)? {
        let e = e?;
        let ziel = nach.join(e.file_name());
        if e.file_type()?.is_dir() {
            kopieren(&e.path(), &ziel)?;
        } else {
            std::fs::copy(e.path(), &ziel)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versionen_werden_als_zahlen_verglichen() {
        assert!(neuer("2.10.0", "2.9.3"));
        assert!(neuer("v2.1.1", "2.1.0"));
        assert!(!neuer("2.1.0", "2.1.0"));
        assert!(!neuer("2.0.9", "2.1.0"));
    }

    #[test]
    fn entwicklungsbau_wird_nicht_aktualisiert() {
        // Die Tests laufen aus target/ — dort darf der Updater nie greifen.
        assert_eq!(art(), None);
    }

    #[test]
    fn ersetzen_tauscht_ordner_vollstaendig() {
        let basis = std::env::temp_dir().join(format!("fotowache-ersetzen-{}", std::process::id()));
        let (q, z) = (basis.join("quelle"), basis.join("ziel"));
        std::fs::create_dir_all(q.join("unter")).unwrap();
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(q.join("unter/neu.txt"), "neu").unwrap();
        std::fs::write(z.join("alt.txt"), "alt").unwrap();
        ersetzen(&q, &z).unwrap();
        assert!(z.join("unter/neu.txt").is_file());
        assert!(!z.join("alt.txt").exists());
        assert!(!z.with_extension("alt").exists());
        let _ = std::fs::remove_dir_all(basis);
    }
}
