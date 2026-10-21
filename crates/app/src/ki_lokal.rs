//! Lokale KI — installieren, starten, fragen. Kein Byte verlaesst den Rechner.
//!
//! ## Was installiert wird (ein Klick, einmalig)
//! - eine Laufzeit fuer Sprach-/Bildmodelle (offenes Projekt, MIT-Lizenz) als
//!   fertiges Programm fuer dieses System,
//! - ein offenes Bildverstehens-Modell (Apache-2.0, ohne Inhaltsfilter — es
//!   beschreibt jedes Foto, statt sich zu weigern) samt Bild-Teil,
//! - ein Retusche-Modell zum Entfernen von Objekten (Apache-2.0, siehe
//!   `pipeline::heal_ki`).
//!
//! Jede Datei ist auf eine feste Version und ihre SHA-256 festgenagelt; die
//! Pruefsumme wird beim Herunterladen mitgerechnet, eine abweichende Datei wird
//! verworfen. Ein abgebrochener Download wird beim naechsten Versuch fortgesetzt.
//!
//! ## Wie gefragt wird
//! Die Laufzeit laeuft als eigener Prozess nur auf 127.0.0.1, wird beim ersten
//! KI-Auftrag gestartet und nach einer Ruhepause wieder beendet (das Modell
//! belegt rund 4 GB Arbeitsspeicher). Fotowache spricht sie ueber die uebliche
//! Chat-Schnittstelle an.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sprache::{t, tf};
use sha2::{Digest, Sha256};

/// Angaben einer Datei, die erst zur Laufzeit feststehen.
struct DateiRef<'a> {
    url: &'a str,
    sha256: &'a str,
    groesse: u64,
    name: &'a str,
}

/// Eine herunterzuladende Datei.
#[derive(Debug, Clone, Copy)]
pub struct Datei {
    /// Name im KI-Ordner.
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub groesse: u64,
}

/// Version der Laufzeit — steht auch im Installationsvermerk.
const LAUFZEIT_VERSION: &str = "b11669";
/// Stand des ganzen Pakets. Aendert sich eine Datei, steigt er, und die
/// Anwendung bietet die Aktualisierung an.
const PAKET_STAND: u32 = 1;

macro_rules! laufzeit {
    ($datei:literal, $sha:literal, $groesse:literal) => {
        Some(Datei {
            name: $datei,
            url: concat!("https://github.com/ggml-org/llama.cpp/releases/download/b11669/", $datei), // Kennung: Download-Adresse
            sha256: $sha,
            groesse: $groesse,
        })
    };
}

/// Die Laufzeit fuer dieses System (die Fassung fuer die Grafikschnittstelle des
/// Systems; ohne passende Grafik rechnet sie auf der CPU).
pub fn laufzeit() -> Option<Datei> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        laufzeit!("llama-b11669-bin-macos-arm64.tar.gz", "dbed55493d72bc1ba2babc8770d669c2289b5bc4ee9fb2a4dfc158d45b7d13fd", 12191582) // Kennung: Paketname
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        laufzeit!("llama-b11669-bin-macos-x64.tar.gz", "cab28238d78158109b48deb27b05d50e0dcef56275d2e76da9ca8281526c7532", 11677404) // Kennung: Paketname
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        laufzeit!("llama-b11669-bin-ubuntu-vulkan-x64.tar.gz", "e8709e5ba6c1b093dea7f3de57f4aba842f9a188fba6b465782d65d6fcfa4d9f", 30497889) // Kennung: Paketname
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        laufzeit!("llama-b11669-bin-ubuntu-vulkan-arm64.tar.gz", "d283522e479c0d532e3a04d715dd852ffb2d95925caa6d41ef2ba53a65e46a51", 24187561) // Kennung: Paketname
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        laufzeit!("llama-b11669-bin-win-vulkan-x64.zip", "2c07574e7bde71321cb785282b20cc821e951132149eeceeea32ddebdbf0598e", 32182350) // Kennung: Paketname
    } else {
        None
    }
}

/// Laeuft die fertige Laufzeit auf diesem System? Die Fassungen fuer die
/// glibc-Systeme sind gegen glibc 2.38 gebaut (gemessen: mit 2.35 startet sie
/// nicht, mit 2.39 ohne Grafikkarte rechnet sie auf der CPU). Der Text erklaert,
/// woran es scheitert.
pub fn verfuegbar() -> Result<(), String> {
    if laufzeit().is_none() {
        return Err(t("Für dieses System gibt es keine fertige KI-Laufzeit.").into());
    }
    if cfg!(target_os = "linux") {
        if let Some((major, minor)) = glibc_version() {
            if (major, minor) < (2, 38) {
                return Err(tf(
                    "Die lokale KI braucht eine neuere Systembibliothek (glibc 2.38 oder neuer); hier ist {}.{} installiert.",
                    &[&major, &minor],
                ));
            }
        }
    }
    Ok(())
}

/// glibc-Version aus `ldd --version` (erste Zeile endet mit der Version).
pub(crate) fn glibc_version() -> Option<(u32, u32)> {
    let aus = Command::new("ldd").arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&aus.stdout);
    glibc_aus_text(text.lines().next()?)
}

fn glibc_aus_text(zeile: &str) -> Option<(u32, u32)> {
    let v = zeile.split_whitespace().last()?;
    let mut t = v.split('.');
    Some((t.next()?.parse().ok()?, t.next()?.parse().ok()?))
}

/// Das Bildverstehens-Modell.
pub const MODELL: Datei = Datei {
    name: "bildmodell.gguf",
    url: "https://huggingface.co/mradermacher/Huihui-Qwen3.5-4B-abliterated-GGUF/resolve/4a5daa6fbefca5fe822dc65fcb95cc4576fa9720/Huihui-Qwen3.5-4B-abliterated.Q4_K_M.gguf", // Kennung: Download-Adresse
    sha256: "423f10b6ec2d99c3378143d7cd3b80eb4887b3ed92103103ac59173b404f4f7c",
    groesse: 2707514688,
};

/// Der Bild-Teil des Modells (uebersetzt Pixel in Modell-Eingaben).
pub const BILDTEIL: Datei = Datei {
    name: "bildteil.gguf",
    url: "https://huggingface.co/mradermacher/Huihui-Qwen3.5-4B-abliterated-GGUF/resolve/4a5daa6fbefca5fe822dc65fcb95cc4576fa9720/Huihui-Qwen3.5-4B-abliterated.mmproj-Q8_0.gguf", // Kennung: Download-Adresse
    sha256: "650e2af8754b8b10e21debdbaf352cab43cd2435ec8ed56b893b9b581ae4a95c",
    groesse: 366894720,
};

/// Das Retusche-Modell (Objekte entfernen).
pub const RETUSCHE: Datei = Datei {
    name: "retusche.onnx",
    url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/c3c0c9e468934d62e79c329e35d82dd09ff8c444/lama_fp32.onnx", // Kennung: Download-Adresse
    sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
    groesse: 208044816,
};

/// Alle Dateien des Pakets fuer dieses System.
pub fn dateien() -> Vec<Datei> {
    let mut v = Vec::new();
    v.extend(laufzeit());
    v.extend([MODELL, BILDTEIL, RETUSCHE]);
    v
}

/// Gesamtgroesse des Pakets in Bytes (fuer die Anzeige vor dem Klick).
pub fn paketgroesse() -> u64 {
    dateien().iter().map(|d| d.groesse).sum()
}

/// Ordner der lokalen KI. `FOTOWACHE_KI_ORDNER` setzt ihn um (Pruefstand, CI).
pub fn ordner() -> Option<PathBuf> {
    if let Some(o) = std::env::var_os("FOTOWACHE_KI_ORDNER").filter(|o| !o.is_empty()) {
        return Some(PathBuf::from(o));
    }
    crate::ordner::daten().map(|d| d.join("ki"))
}

fn vermerk_pfad() -> Option<PathBuf> {
    ordner().map(|o| o.join("installiert.json"))
}

/// Ist das Paket vollstaendig installiert (und aktuell)?
pub fn installiert() -> bool {
    let Some(v) = vermerk_pfad() else { return false };
    let Ok(text) = std::fs::read_to_string(v) else { return false };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    j.get("stand").and_then(|s| s.as_u64()) == Some(PAKET_STAND as u64)
        && server_programm().is_some()
        && [MODELL, BILDTEIL, RETUSCHE]
            .iter()
            .all(|d| ordner().is_some_and(|o| o.join(d.name).is_file()))
}

/// Pfad des Retusche-Modells, wenn installiert.
pub fn retusche_modell() -> Option<PathBuf> {
    let p = ordner()?.join(RETUSCHE.name);
    (installiert() && p.is_file()).then_some(p)
}

/// Fortschritt der Installation — fuer Balken und Kommandozeile.
#[derive(Debug, Clone)]
pub enum Fortschritt {
    Laedt { datei: usize, von: usize, bytes: u64, gesamt: u64 },
    Entpackt,
    Fertig,
}

/// Installiert das ganze Paket. `abbruch` beendet den Download sauber (der
/// angefangene Teil bleibt liegen und wird beim naechsten Mal fortgesetzt).
pub fn installieren(melde: &dyn Fn(Fortschritt), abbruch: &AtomicBool) -> Result<(), String> {
    let ordner = ordner().ok_or_else(|| t("Kein Datenordner gefunden.").to_string())?;
    verfuegbar()?;
    let Some(lz) = laufzeit() else {
        return Err(t("Für dieses System gibt es keine fertige KI-Laufzeit.").into());
    };
    std::fs::create_dir_all(&ordner).map_err(|e| e.to_string())?;
    let alle = dateien();
    for (i, d) in alle.iter().enumerate() {
        let ziel = ordner.join(d.name);
        if ziel.is_file() && std::fs::metadata(&ziel).map(|m| m.len()).unwrap_or(0) == d.groesse {
            // Schon da (frueher geprueft) — nicht noch einmal laden.
            continue;
        }
        laden(d, &ziel, &|bytes| melde(Fortschritt::Laedt { datei: i + 1, von: alle.len(), bytes, gesamt: d.groesse }), abbruch)?;
    }
    melde(Fortschritt::Entpackt);
    entpacken(&ordner.join(lz.name), &ordner.join("laufzeit"))?;
    if server_programm().is_none() {
        return Err(t("Die KI-Laufzeit ließ sich nicht entpacken.").into());
    }
    let vermerk = serde_json::json!({
        "stand": PAKET_STAND,
        "laufzeit": LAUFZEIT_VERSION,
        "dateien": alle.iter().map(|d| serde_json::json!({"name": d.name, "sha256": d.sha256})).collect::<Vec<_>>(),
    });
    std::fs::write(ordner.join("installiert.json"), serde_json::to_string_pretty(&vermerk).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    pipeline::heal_ki::modell_setzen(retusche_modell());
    melde(Fortschritt::Fertig);
    Ok(())
}

/// Laedt nur die Laufzeit (wenige MB) ueber denselben Weg wie die Installation,
/// entpackt sie und startet das Programm mit `--version` — fuer den Bauablauf
/// auf jedem System, ohne die Modelle (Gigabytes) zu laden.
pub fn laufzeit_pruefen(lz: &Datei, ordner: &Path, programm: &str) -> Result<String, String> {
    std::fs::create_dir_all(ordner).map_err(|e| e.to_string())?;
    let archiv = ordner.join(lz.name);
    if !archiv.is_file() {
        laden(lz, &archiv, &|_| {}, &AtomicBool::new(false))?;
    }
    let ziel = ordner.join("laufzeit");
    entpacken(&archiv, &ziel)?;
    let mut stapel = vec![ziel];
    let mut gefunden = None;
    while let Some(d) = stapel.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stapel.push(p);
            } else if p.file_name().is_some_and(|n| n == programm) {
                gefunden = Some(p);
            }
        }
    }
    let p = gefunden.ok_or_else(|| format!("{programm} fehlt im Archiv"))?;
    let aus = Command::new(&p)
        .arg("--version")
        .current_dir(p.parent().unwrap_or(ordner))
        .output()
        .map_err(|e| format!("{programm}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&aus.stdout), String::from_utf8_lossy(&aus.stderr));
    if !aus.status.success() {
        return Err(format!("{programm} --version: {}", text.trim()));
    }
    Ok(text.lines().filter(|z| !z.trim().is_empty()).last().unwrap_or("").trim().to_string())
}

/// Entfernt das ganze Paket (gibt die Gigabytes wieder frei).
pub fn entfernen() -> Result<(), String> {
    beenden();
    pipeline::heal_ki::modell_setzen(None);
    if let Some(o) = ordner() {
        if o.exists() {
            std::fs::remove_dir_all(&o).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Laedt eine Datei, rechnet dabei die SHA-256 mit und setzt einen
/// abgebrochenen Download fort.
pub(crate) fn laden(d: &Datei, ziel: &Path, melde: &dyn Fn(u64), abbruch: &AtomicBool) -> Result<(), String> {
    laden_von(d.url, d.sha256, d.groesse, d.name, ziel, melde, abbruch)
}

/// Wie `laden`, mit Angaben, die erst zur Laufzeit feststehen (Updater).
pub(crate) fn laden_von(
    url: &str,
    sha256: &str,
    groesse: u64,
    name: &str,
    ziel: &Path,
    melde: &dyn Fn(u64),
    abbruch: &AtomicBool,
) -> Result<(), String> {
    let d = DateiRef { url, sha256, groesse, name };
    let teil = ziel.with_extension("teil");
    let mut hasher = Sha256::new();
    let mut vorhanden = 0u64;
    if let Ok(mut f) = std::fs::File::open(&teil) {
        // Bisher Geladenes in die Pruefsumme nehmen.
        let mut puffer = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut puffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&puffer[..n]);
            vorhanden += n as u64;
        }
        if vorhanden > d.groesse {
            vorhanden = 0;
            hasher = Sha256::new();
            let _ = std::fs::remove_file(&teil);
        }
    }
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let mut anfrage = agent.get(d.url);
    if vorhanden > 0 {
        anfrage = anfrage.header("Range", &format!("bytes={vorhanden}-"));
    }
    let mut antwort = anfrage.call().map_err(|e| format!("{}: {e}", d.name))?;
    if vorhanden > 0 && antwort.status().as_u16() != 206 {
        // Der Server setzt nicht fort — von vorn.
        vorhanden = 0;
        hasher = Sha256::new();
    }
    let mut datei = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(vorhanden > 0)
        .truncate(vorhanden == 0)
        .open(&teil)
        .map_err(|e| e.to_string())?;
    let mut leser = antwort.body_mut().as_reader();
    let mut puffer = vec![0u8; 1 << 20];
    let mut bytes = vorhanden;
    let mut zuletzt = Instant::now();
    loop {
        if abbruch.load(Ordering::Relaxed) {
            return Err(t("abgebrochen").into());
        }
        let n = leser.read(&mut puffer).map_err(|e| format!("{}: {e}", d.name))?;
        if n == 0 {
            break;
        }
        datei.write_all(&puffer[..n]).map_err(|e| e.to_string())?;
        hasher.update(&puffer[..n]);
        bytes += n as u64;
        if zuletzt.elapsed() > Duration::from_millis(250) {
            melde(bytes);
            zuletzt = Instant::now();
        }
    }
    datei.flush().map_err(|e| e.to_string())?;
    drop(datei);
    melde(bytes);
    let summe = hex(&hasher.finalize());
    if summe != d.sha256 {
        let _ = std::fs::remove_file(&teil);
        return Err(tf("{}: Prüfsumme stimmt nicht — Datei verworfen", &[&d.name]));
    }
    std::fs::rename(&teil, ziel).map_err(|e| e.to_string())?;
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Entpackt das Laufzeit-Archiv mit dem `tar` des Systems (alle drei Zielsysteme
/// bringen eines mit, das auch .zip kann).
pub(crate) fn entpacken(archiv: &Path, ziel: &Path) -> Result<(), String> {
    if ziel.exists() {
        std::fs::remove_dir_all(ziel).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(ziel).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archiv)
        .arg("-C")
        .arg(ziel)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err(t("Entpacken fehlgeschlagen").into());
    }
    Ok(())
}

/// Das Server-Programm der Laufzeit (liegt je nach Archiv in einem Unterordner).
fn server_programm() -> Option<PathBuf> {
    let name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" }; // Kennung: Programmname der Laufzeit
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

// ---------------------------------------------------------------------------
// Der laufende Dienst
// ---------------------------------------------------------------------------

struct Dienst {
    kind: Child,
    port: u16,
    zuletzt: Instant,
}

static DIENST: Mutex<Option<Dienst>> = Mutex::new(None);
static WAECHTER: AtomicBool = AtomicBool::new(false);
/// Nach so langer Ruhe wird das Modell entladen (Arbeitsspeicher frei).
const RUHE: Duration = Duration::from_secs(15 * 60);

fn freier_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

fn pid_datei() -> Option<PathBuf> {
    ordner().map(|o| o.join("dienst.pid"))
}

/// Ein Dienst, den ein abgestuerztes Fotowache hinterlassen hat, belegt noch
/// Gigabytes — beenden, bevor ein neuer startet.
fn waise_beenden() {
    let Some(p) = pid_datei() else { return };
    if let Ok(t) = std::fs::read_to_string(&p) {
        if let Ok(pid) = t.trim().parse::<u32>() {
            #[cfg(unix)]
            {
                // Nur, wenn es wirklich unser Server-Programm ist.
                let ist_unserer = Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains("ki/laufzeit"))
                    .unwrap_or(false);
                if ist_unserer {
                    let _ = Command::new("kill").arg(pid.to_string()).status();
                }
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status();
            }
        }
        let _ = std::fs::remove_file(p);
    }
}

fn starten() -> Result<Dienst, String> {
    let programm = server_programm().ok_or(t("Die lokale KI ist nicht installiert."))?;
    let o = ordner().ok_or(t("Kein Datenordner gefunden."))?;
    waise_beenden();
    let port = freier_port()?;
    let log = std::fs::File::create(o.join("dienst.log")).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&programm);
    cmd.arg("-m")
        .arg(o.join(MODELL.name))
        .arg("--mmproj")
        .arg(o.join(BILDTEIL.name))
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["-c", "8192", "-ngl", "99", "-np", "1", "--no-webui", "--reasoning-budget", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // Die Bibliotheken liegen neben dem Programm.
    if let Some(d) = programm.parent() {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt; // Kennung: Modulpfad
        // Kein Konsolenfenster fuer den Dienst.
        cmd.creation_flags(0x0800_0000);
    }
    let mut kind = cmd.spawn().map_err(|e| tf("KI-Dienst startet nicht: {}", &[&e]))?;
    if let Some(p) = pid_datei() {
        let _ = std::fs::write(p, kind.id().to_string());
    }
    // Warten, bis das Modell geladen ist.
    let beginn = Instant::now();
    loop {
        if let Ok(Some(s)) = kind.try_wait() {
            let log = std::fs::read_to_string(o.join("dienst.log")).unwrap_or_default();
            let ende: String = log.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(tf("KI-Dienst beendet ({}):\n{}", &[&s, &ende]));
        }
        let gesund = ureq::get(&format!("http://127.0.0.1:{port}/health"))
            .config()
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .call()
            .map(|r| r.status().as_u16() == 200)
            .unwrap_or(false);
        if gesund {
            break;
        }
        if beginn.elapsed() > Duration::from_secs(180) {
            let _ = kind.kill();
            return Err(t("KI-Dienst wurde nach drei Minuten nicht bereit.").into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    waechter_starten();
    Ok(Dienst { kind, port, zuletzt: Instant::now() })
}

/// Entlaedt das Modell nach einer Ruhepause.
fn waechter_starten() {
    if WAECHTER.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(30));
        let mut g = match DIENST.lock() {
            Ok(g) => g,
            Err(_) => continue,
        };
        if g.as_ref().is_some_and(|d| d.zuletzt.elapsed() > RUHE) {
            if let Some(mut d) = g.take() {
                let _ = d.kind.kill();
                let _ = d.kind.wait();
                if let Some(p) = pid_datei() {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    });
}

/// Beendet den Dienst (beim Schliessen von Fotowache).
pub fn beenden() {
    if let Ok(mut g) = DIENST.lock() {
        if let Some(mut d) = g.take() {
            let _ = d.kind.kill();
            let _ = d.kind.wait();
            if let Some(p) = pid_datei() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Port des laufenden Dienstes — startet ihn bei Bedarf.
fn port() -> Result<u16, String> {
    let mut g = DIENST.lock().map_err(|_| "KI-Dienst gesperrt".to_string())?;
    let lebt = g.as_mut().is_some_and(|d| matches!(d.kind.try_wait(), Ok(None)));
    if !lebt {
        *g = Some(starten()?);
    }
    let d = g.as_mut().expect("eben gestartet");
    d.zuletzt = Instant::now();
    Ok(d.port)
}

/// Fragt das Modell. `verlauf` sind die bisherigen Paare (Frage, Antwort), die
/// erste Frage traegt das Bild. Gibt den Antworttext zurueck.
pub fn fragen(bild_jpeg: &[u8], verlauf: &[(&str, &str)], frage: &str, max_tokens: u32) -> Result<String, String> {
    let port = port()?;
    let bild = format!("data:image/jpeg;base64,{}", crate::ai::base64_encode(bild_jpeg));
    let mut nachrichten = Vec::new();
    let mut erste = true;
    let user = |text: &str, erste: &mut bool| {
        if *erste {
            *erste = false;
            serde_json::json!({"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": bild}},
                {"type": "text", "text": text},
            ]})
        } else {
            serde_json::json!({"role": "user", "content": text})
        }
    };
    for (f, a) in verlauf {
        nachrichten.push(user(f, &mut erste));
        nachrichten.push(serde_json::json!({"role": "assistant", "content": a}));
    }
    nachrichten.push(user(frage, &mut erste));
    let body = serde_json::json!({
        "messages": nachrichten,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "chat_template_kwargs": {"enable_thinking": false},
    });
    let mut antwort = ureq::post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .config()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .send_json(&body)
        .map_err(|e| format!("KI-Dienst: {e}"))?;
    let text = antwort.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let j: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Ok(mut g) = DIENST.lock() {
        if let Some(d) = g.as_mut() {
            d.zuletzt = Instant::now();
        }
    }
    j.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(str::to_string)
        .ok_or_else(|| t("KI-Dienst: leere Antwort").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jede_unterstuetzte_plattform_hat_eine_laufzeit() {
        if cfg!(any(
            all(target_os = "macos", any(target_arch = "aarch64", target_arch = "x86_64")),
            all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64")),
            all(target_os = "windows", target_arch = "x86_64"),
        )) {
            let l = laufzeit().expect("Laufzeit fuer dieses System");
            assert_eq!(l.sha256.len(), 64);
            assert!(l.url.ends_with(l.name));
        }
        for d in [MODELL, BILDTEIL, RETUSCHE] {
            assert_eq!(d.sha256.len(), 64, "{}", d.name);
            assert!(d.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(d.groesse > 1_000_000);
        }
        assert!(paketgroesse() > 3_000_000_000);
    }

    #[test]
    fn glibc_wird_aus_ldd_gelesen() {
        assert_eq!(glibc_aus_text("ldd (Distribution GLIBC 2.35-0dist3.8) 2.35"), Some((2, 35)));
        assert_eq!(glibc_aus_text("ldd (GNU libc) 2.39"), Some((2, 39)));
        assert_eq!(glibc_aus_text("unbekannt"), None);
    }

    #[test]
    fn hex_ist_klein_und_zweistellig() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
