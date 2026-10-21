//! Fotowache — nativer Foto-Entwickler.
//!
//! Aufbau der Oberflaeche bewusst wie in der Web-Version, damit sich niemand
//! umgewoehnen muss: Bild in der Mitte, Regler rechts in denselben Gruppen
//! (Licht · Farbe · Farbverlauf · Effekte · Details · Objektiv), Filmstreifen unten.
//! Neu und der eigentliche Grund fuer die Neuentwicklung: echte Stapelverarbeitung
//! und volle Aufloesung beim Export.
//!
//! Die Oberflaeche spricht Deutsch und Englisch — jeder sichtbare Text geht durch
//! `sprache::t` / `sprache::tf` (siehe `sprache.rs`).

// Fuer die Plattform in der Bedingung darunter: eine Fensteranwendung — sonst
// oeffnet sich beim Start aus dem Startmenue neben dem Programm ein schwarzes
// Konsolenfenster. Fuer die Kommandozeilen-Wege haengt sich das Programm an die
// aufrufende Konsole an (`konsole_anhaengen`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai;
mod generator;
mod ki_lokal;
mod export;
mod grundpanel;
mod ordner;
mod panorama;
mod presets;
mod sprache;
mod updater;
mod zusatz;

use eframe::egui;
use pipeline::{params::Edit, Gpu, GpuImage};
use sprache::{t, tf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Wie gross die Arbeitskopie fuers Regeln ist. Gross genug fuer einen 5K-Schirm,
/// klein genug, dass ein Reglerzug in Millisekunden durchlaeuft.
const PREVIEW_MAX: u32 = 2400;
const THUMB_MAX: u32 = 320;

fn main() -> eframe::Result<()> {
    // Stapel ohne Fenster: `fotowache --stapel <ordner|dateien…> [--ziel <ordner>]`.
    // Praktisch fuer grosse Mengen ueber Nacht — und der Weg, auf dem sich die
    // Stapelverarbeitung ueberpruefen laesst, ohne durch die Oberflaeche zu klicken.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Ordner aus der Zeit vor dem Namen „Fotowache" mitnehmen (einmalig).
    ordner::umziehen();
    // Retusche-Modell der lokalen KI bekannt machen (falls installiert).
    pipeline::heal_ki::modell_setzen(ki_lokal::retusche_modell());
    #[cfg(windows)]
    if argv.first().is_some_and(|a| a.starts_with("--")) {
        konsole_anhaengen();
    }
    if argv.first().map(|a| a == "--version").unwrap_or(false) {
        // Aus der EINEN autoritativen Stelle (workspace Cargo.toml) — dazu die
        // LibRaw-Version: welche Kameras gehen, haengt an ihr.
        println!("Fotowache {} · LibRaw {}", env!("CARGO_PKG_VERSION"), rawcore::libraw_version());
        std::process::exit(0);
    }
    if argv.first().map(|a| a == "--stapel").unwrap_or(false) {
        let code = run_batch_cli(&argv[1..]);
        ki_lokal::beenden();
        std::process::exit(code);
    }
    // Lokale KI ohne Fenster installieren / entfernen.
    if argv.first().map(|a| a == "--ki-installieren").unwrap_or(false) {
        if ki_lokal::installiert() {
            println!("{}", t("Lokale KI ist installiert."));
            std::process::exit(0);
        }
        let abbruch = std::sync::atomic::AtomicBool::new(false);
        let zuletzt = std::cell::Cell::new(0u64);
        let ergebnis = ki_lokal::installieren(
            &|f| match f {
                ki_lokal::Fortschritt::Laedt { datei, von, bytes, gesamt } => {
                    // Hoechstens alle 100 MB eine Zeile.
                    if bytes / 100_000_000 != zuletzt.get() || bytes == gesamt {
                        zuletzt.set(bytes / 100_000_000);
                        println!("[{datei}/{von}] {:.0} / {:.0} MB", bytes as f64 / 1e6, gesamt as f64 / 1e6);
                    }
                }
                ki_lokal::Fortschritt::Entpackt => println!("{}", t("KI wird eingerichtet…")),
                ki_lokal::Fortschritt::Fertig => println!("{}", t("Lokale KI ist installiert.")),
            },
            &abbruch,
        );
        if let Err(e) = ergebnis {
            eprintln!("{}", tf("KI-Installation: {}", &[&e]));
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    // Bildgenerator ohne Fenster: installieren und ein Bild erzeugen.
    if argv.first().map(|a| a == "--generator-installieren").unwrap_or(false) {
        let abbruch = std::sync::atomic::AtomicBool::new(false);
        let zuletzt = std::cell::Cell::new(0u64);
        let r = generator::installieren(
            &|f| {
                if let ki_lokal::Fortschritt::Laedt { datei, von, bytes, gesamt } = f {
                    if bytes / 100_000_000 != zuletzt.get() || bytes == gesamt {
                        zuletzt.set(bytes / 100_000_000);
                        println!("[{datei}/{von}] {:.0} / {:.0} MB", bytes as f64 / 1e6, gesamt as f64 / 1e6);
                    }
                }
            },
            &abbruch,
        );
        match r {
            Ok(()) => println!("{}", t("Bildgenerator ist installiert.")),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    if argv.first().map(|a| a == "--bild").unwrap_or(false) {
        // fotowache --bild "<Beschreibung>" --ziel bild.png [--format quer|hoch|quadrat] [--seed n]
        let mut text = None;
        let mut ziel = PathBuf::from("bild.png");
        let mut format = generator::Format::Quer;
        let mut seed = 1u64;
        let mut it = argv[1..].iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--ziel" => ziel = it.next().map(PathBuf::from).unwrap_or(ziel),
                "--format" => {
                    format = match it.next().map(String::as_str) {
                        Some("hoch") => generator::Format::Hoch,
                        Some("quadrat") | Some("quadratisch") => generator::Format::Quadrat,
                        _ => generator::Format::Quer,
                    }
                }
                "--seed" => seed = it.next().and_then(|s| s.parse().ok()).unwrap_or(seed),
                other => text = Some(other.to_string()),
            }
        }
        let Some(text) = text else {
            eprintln!("fotowache --bild \"<…>\" --ziel bild.png [--format quer|hoch|quadrat] [--seed n]");
            std::process::exit(2);
        };
        let beginn = std::time::Instant::now();
        match generator::erzeugen(&text, format, seed, &ziel) {
            Ok(()) => println!("{} ({:.0} s)", ziel.display(), beginn.elapsed().as_secs_f32()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    // Bauablauf: laufen die KI-Laufzeiten auf diesem System? (ohne Modelle)
    if argv.first().map(|a| a == "--ki-laufzeit-pruefen").unwrap_or(false) {
        let basis = std::env::temp_dir().join(format!("fotowache-laufzeit-{}", std::process::id()));
        let mut fehler = false;
        let pruefe = |name: &str, verfuegbar: Result<(), String>, lz: Option<ki_lokal::Datei>, programm: &str| -> bool {
            match (verfuegbar, lz) {
                (Err(grund), _) => {
                    println!("{name}: nicht verfügbar — {grund}");
                    true
                }
                (Ok(()), Some(lz)) => match ki_lokal::laufzeit_pruefen(&lz, &basis.join(name), programm) {
                    Ok(v) => {
                        println!("{name}: OK — {v}");
                        true
                    }
                    Err(e) => {
                        eprintln!("{name}: FEHLER — {e}");
                        false
                    }
                },
                (Ok(()), None) => true,
            }
        };
        let server = if cfg!(windows) { "llama-server.exe" } else { "llama-server" }; // Kennung: Programmname der Laufzeit
        let cli = if cfg!(windows) { "sd-cli.exe" } else { "sd-cli" }; // Kennung: Programmname der Laufzeit
        fehler |= !pruefe("ki", ki_lokal::verfuegbar(), ki_lokal::laufzeit(), server);
        fehler |= !pruefe("generator", generator::verfuegbar(), generator::laufzeit(), cli);
        let _ = std::fs::remove_dir_all(&basis);
        std::process::exit(if fehler { 1 } else { 0 });
    }
    // Hinzufuegen ohne Fenster (Pruefstand): fotowache --hinzufuegen <raw>
    // --text "…" --bereich x0,y0,x1,y1  — markiert den Bereich, setzt ein und
    // legt das Ergebnis in die Begleitdatei (exportieren dann mit --stapel).
    if argv.first().map(|a| a == "--hinzufuegen").unwrap_or(false) {
        let (mut raw_pfad, mut text, mut bereich) = (None, None, None);
        let mut it = argv[1..].iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--text" => text = it.next().cloned(),
                "--bereich" => {
                    bereich = it.next().and_then(|b| {
                        let v: Vec<f32> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
                    })
                }
                other => raw_pfad = Some(PathBuf::from(other)),
            }
        }
        let (Some(raw_pfad), Some(text), Some([x0, y0, x1, y1])) = (raw_pfad, text, bereich) else {
            eprintln!("fotowache --hinzufuegen <raw> --text \"…\" --bereich x0,y0,x1,y1");
            std::process::exit(2);
        };
        let raw = match rawcore::decode(&raw_pfad, &rawcore::DecodeOptions::default()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let grund = grundstand(&raw_pfad, &raw);
        let mut edit = load_sidecar(&raw_pfad).map(|(e, _)| e).unwrap_or_else(|| grund.clone());
        startwerte_nachziehen(&mut edit, &grund);
        // Den Bereich mit Flecken fuellen (Radius 4 % der Breite, dicht gesetzt).
        let r = 0.04f32;
        let seiten = raw.height as f32 / raw.width as f32;
        let mut flecken = Vec::new();
        let mut y = y0 + r / seiten;
        while y <= y1 - r / seiten + 1e-6 {
            let mut x = x0 + r;
            while x <= x1 - r + 1e-6 {
                flecken.push([x, y, r]);
                x += r;
            }
            y += r / seiten;
        }
        let beginn = std::time::Instant::now();
        match zusatz::erzeugen(&raw, &raw_pfad, &edit, flecken, &text, 7) {
            Ok(z) => {
                println!("{} ({:.0} s)", z.datei, beginn.elapsed().as_secs_f32());
                edit.zusaetze.push(z);
                edit.bearbeitet = true;
                let stars = load_sidecar(&raw_pfad).map(|(_, s)| s).unwrap_or(0);
                save_sidecar(&raw_pfad, &edit, stars);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    // Panorama ohne Fenster: fotowache --panorama <fotos…> [--projektion zylinder|kugel|eben]
    // [--raender-fuellen] [--ohne-zuschnitt] [--ohne-parallaxe] [--voll]
    if argv.first().map(|a| a == "--panorama").unwrap_or(false) {
        let mut auftrag = panorama::Auftrag {
            dateien: Vec::new(),
            projektion: pipeline::panorama::Projektion::Zylinder,
            belichtung_angleichen: true,
            parallaxe_ausgleichen: true,
            raender_fuellen: false,
            zuschneiden: true,
            max_seite: 8000,
        };
        let mut it = argv[1..].iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--projektion" => {
                    auftrag.projektion = match it.next().map(String::as_str) {
                        Some("kugel") => pipeline::panorama::Projektion::Kugel,
                        Some("eben") => pipeline::panorama::Projektion::Eben,
                        _ => pipeline::panorama::Projektion::Zylinder,
                    }
                }
                "--raender-fuellen" => auftrag.raender_fuellen = true,
                "--ohne-zuschnitt" => auftrag.zuschneiden = false,
                "--ohne-parallaxe" => auftrag.parallaxe_ausgleichen = false,
                "--voll" => auftrag.max_seite = 0,
                other => auftrag.dateien.push(PathBuf::from(other)),
            }
        }
        let beginn = std::time::Instant::now();
        let letzte = std::cell::Cell::new(-1i32);
        match panorama::erstellen(&auftrag, &|a, text| {
            let p = (a * 100.0) as i32;
            if p / 5 != letzte.get() / 5 {
                letzte.set(p);
                println!("{p:3} %  {text}");
            }
        }) {
            Ok(p) => println!("{} ({:.0} s)", p.display(), beginn.elapsed().as_secs_f32()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    // Update ohne Fenster: pruefen und installieren (ohne Neustart).
    if argv.first().map(|a| a == "--update").unwrap_or(false) {
        match updater::pruefen() {
            Ok(None) => {
                println!("{}", if updater::art().is_some() { t("Fotowache ist aktuell.") } else { t("Diese Kopie wird nicht automatisch aktualisiert.") });
            }
            Ok(Some(neu)) => {
                println!("{}", tf("Neue Version {} — wird geladen …", &[&neu.version]));
                let abbruch = std::sync::atomic::AtomicBool::new(false);
                if let Err(e) = updater::installieren(&neu, &|_, _| {}, &abbruch) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                println!("{}", tf("Aktualisiert auf {}.", &[&neu.version]));
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    if argv.first().map(|a| a == "--generator-entfernen").unwrap_or(false) {
        match generator::entfernen() {
            Ok(()) => println!("{}", t("Bildgenerator entfernt.")),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    if argv.first().map(|a| a == "--ki-entfernen").unwrap_or(false) {
        match ki_lokal::entfernen() {
            Ok(()) => println!("{}", t("Lokale KI entfernt.")),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }
    // Miniatur ohne Fenster: `fotowache --miniatur <raw> <ziel.png>` — derselbe Weg,
    // den Galerie und Filmstreifen nehmen. Nur so laesst sich deren Helligkeit
    // MESSEN statt behaupten (die Galerie war einmal viel zu dunkel, weil dieser
    // Pfad die Grundhelligkeit nicht kannte — nie wieder ungeprueft).
    if argv.first().map(|a| a == "--miniatur").unwrap_or(false) {
        let (Some(quelle), Some(ziel)) = (argv.get(1), argv.get(2)) else {
            eprintln!("{}", t("fotowache --miniatur <raw> <ziel.png>"));
            std::process::exit(2);
        };
        let opts = rawcore::DecodeOptions {
            demosaic: rawcore::DemosaicQuality::Schnell,
            half_size: true,
            ..Default::default()
        };
        let raw = match rawcore::decode(Path::new(quelle), &opts) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{quelle}: {}", sprache::raw_fehler(&e));
                std::process::exit(1);
            }
        };
        let (w, h, rgba) = quick_thumb(&raw, THUMB_MAX);
        let Some(buf) = image::RgbaImage::from_raw(w, h, rgba) else {
            std::process::exit(1);
        };
        if let Err(e) = image::DynamicImage::ImageRgba8(buf).to_rgb8().save(ziel) {
            eprintln!("{ziel}: {e}");
            std::process::exit(1);
        }
        println!("{ziel} {w}x{h}");
        std::process::exit(0);
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_min_inner_size([1100.0, 700.0])
            .with_title(concat!("Fotowache ", env!("CARGO_PKG_VERSION")))
            .with_icon(fenster_symbol()),
        ..Default::default()
    };
    eframe::run_native(
        "Fotowache",
        opts,
        Box::new(|cc| {
            egui_extras_style(&cc.egui_ctx);
            let mut app = App::new();
            // Startparameter: `fotowache <ordner|datei…>` — praktisch beim Arbeiten und
            // unverzichtbar zum Pruefen, ohne jedes Mal durch den Dateidialog zu klicken.
            let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
            if !args.is_empty() {
                app.open_paths(args);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Stapelverarbeitung von der Kommandozeile.
fn run_batch_cli(args: &[String]) -> i32 {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut settings = export::ExportSettings::default();
    let mut use_ai = false;
    let mut ohne_automatik = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ziel" => {
                if let Some(d) = it.next() {
                    settings.target_dir = Some(PathBuf::from(d));
                }
            }
            "--breite" => {
                if let Some(v) = it.next() {
                    settings.max_dim = v.parse().ok();
                }
            }
            "--tiff" => settings.format = export::Format::Tiff16,
            "--png" => settings.format = export::Format::Png,
            "--ki" => use_ai = true,
            // Alle Regler auf null — das unbearbeitete Foto (Vergleiche, Pruefstand).
            "--ohne-automatik" => ohne_automatik = true,
            other => inputs.push(PathBuf::from(other)),
        }
    }
    // Ohne installierte lokale KI waere `--ki` still wirkungslos — lieber gleich
    // sagen, was fehlt.
    if use_ai && !ki_lokal::installiert() {
        eprintln!(
            "{}",
            t("Die lokale KI ist nicht installiert: erst `fotowache --ki-installieren` \
               (einmalig, lädt die Modelle herunter).")
        );
        return 2;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for p in inputs {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                let mut f: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| rawcore::is_raw_path(p))
                    .collect();
                f.sort();
                files.extend(f);
            }
        } else if rawcore::is_raw_path(&p) {
            files.push(p);
        }
    }
    if files.is_empty() {
        eprintln!("{}", t("Keine RAW-Dateien gefunden."));
        return 2;
    }
    let gpu = match pollster::block_on(Gpu::new()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{}", tf("Grafikkarte: {}", &[&sprache::gpu_fehler(&e)]));
            return 3;
        }
    };
    println!("{}", tf("{} Dateien · {}", &[&files.len(), &gpu.backend]));
    let (mut ok, mut failed) = (0, 0);
    for (i, f) in files.iter().enumerate() {
        let beginn = std::time::Instant::now();
        let result = if use_ai {
            develop_with_ai(&gpu, f, &presets::start_edit(), &settings, ai::Strength::Normal)
        } else if ohne_automatik {
            rawcore::decode(f, &rawcore::DecodeOptions::default())
                .map_err(export::ExportError::from)
                .and_then(|raw| {
                    let mut e = presets::start_edit();
                    e.rotate = drehung_aus_kamera(raw.meta.flip);
                    export::save_developed(&gpu, &raw, f, &e, &settings)
                })
        } else {
            // Bereits gesicherte Bearbeitung hat Vorrang; sonst Startwerte MIT
            // gemessener Grundhelligkeit. Fehlt einer Begleitdatei die
            // Grundausstattung (base_ev 0), wird sie nachgezogen wie überall.
            rawcore::decode(f, &rawcore::DecodeOptions::default())
                .map_err(export::ExportError::from)
                .and_then(|raw| {
                    let grund = grundstand(f, &raw);
                    let mut edit = load_sidecar(f).map(|(e, _)| e).unwrap_or_else(|| grund.clone());
                    startwerte_nachziehen(&mut edit, &grund);
                    export::save_developed(&gpu, &raw, f, &edit, &settings)
                })
        };
        match result {
            Ok(target) => {
                ok += 1;
                println!(
                    "[{}/{}] {} -> {} ({:.1?})",
                    i + 1,
                    files.len(),
                    name_of(f),
                    name_of(&target),
                    beginn.elapsed()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("[{}/{}] {}: {e}", i + 1, files.len(), name_of(f));
            }
        }
    }
    println!("{}", tf("fertig: {} ok, {} fehlgeschlagen", &[&ok, &failed]));
    i32::from(failed > 0)
}

/// Farben des Programms — an einer Stelle, damit nichts auseinanderlaeuft.
mod farbe {
    use eframe::egui::Color32;
    pub const GRUND: Color32 = Color32::from_rgb(16, 17, 21);
    pub const FLAECHE: Color32 = Color32::from_rgb(24, 26, 32);
    pub const FLAECHE_HELL: Color32 = Color32::from_rgb(32, 35, 43);
    pub const RAND: Color32 = Color32::from_rgb(46, 50, 60);
    pub const TEXT: Color32 = Color32::from_rgb(226, 230, 238);
    pub const TEXT_LEISE: Color32 = Color32::from_rgb(138, 146, 162);
    /// Akzent: ein ruhiges Bernstein. Kraeftig genug zum Fuehren, aber es
    /// konkurriert nicht mit den Farben im Foto — der eigentliche Grund, warum
    /// Bildprogramme keine bunten Oberflaechen haben.
    pub const AKZENT: Color32 = Color32::from_rgb(232, 168, 72);
    pub const AKZENT_LEISE: Color32 = Color32::from_rgb(84, 62, 30);
    /// Gruen fuer „fertig bearbeitet" — Punkt in der Galerie und Rahmen um die
    /// Miniaturen. Eine Stelle, damit Galerie und Filmstreifen dieselbe Sprache
    /// sprechen.
    pub const BEARBEITET: Color32 = Color32::from_rgb(120, 200, 130);
    /// Untergrund der schwebenden Leisten ueber dem Bild. Bewusst nicht ganz
    /// deckend: man soll sehen, dass darunter das Foto weitergeht.
    pub const SCHWEBE: Color32 = Color32::from_rgba_premultiplied(14, 15, 19, 226);
}

/// Das Programmsymbol fuer Fenster und Taskleiste; wo das Programm als
/// .app-Paket kommt, nimmt das System das Symbol aus dem Paket.
fn fenster_symbol() -> egui::IconData {
    match image::load_from_memory(include_bytes!("../assets/fotowache-256.png")) {
        Ok(bild) => {
            let rgba = bild.to_rgba8();
            egui::IconData {
                width: rgba.width(),
                height: rgba.height(),
                rgba: rgba.into_raw(),
            }
        }
        Err(_) => egui::IconData::default(),
    }
}

/// Als Fensteranwendung (Kopf dieser Datei) hat das Programm keine eigene
/// Konsole. Aus einer Eingabeaufforderung aufgerufen (`fotowache --stapel …`),
/// haengt es sich an deren Konsole, damit Fortschritt und Meldungen dort
/// erscheinen.
#[cfg(windows)]
fn konsole_anhaengen() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // Schlaegt fehl, wenn es keine aufrufende Konsole gibt (Doppelklick) —
    // dann gibt es auch niemanden, der die Ausgabe liest.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn egui_extras_style(ctx: &egui::Context) {
    // `set_theme` ist noetig, weil eframe sonst das Systemthema uebernimmt und
    // alles hier still ueberschreibt (erst am fertigen Fenster sichtbar).
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = farbe::GRUND;
    v.window_fill = farbe::FLAECHE;
    v.extreme_bg_color = Color32::from_rgb(11, 12, 15);
    v.faint_bg_color = farbe::FLAECHE;
    v.override_text_color = Some(farbe::TEXT);
    v.window_stroke = egui::Stroke::new(1.0_f32, farbe::RAND);
    v.window_corner_radius = egui::CornerRadius::same(12);

    // Flaechen statt Kaesten: keine Rahmen um jeden Knopf — das war der
    // „90er-Look". Unterschieden wird ueber Fuellung und Rundung.
    let r = egui::CornerRadius::same(8);
    v.widgets.noninteractive.bg_fill = farbe::FLAECHE;
    v.widgets.noninteractive.weak_bg_fill = farbe::FLAECHE;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, farbe::RAND);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, farbe::TEXT_LEISE);

    v.widgets.inactive.bg_fill = farbe::FLAECHE_HELL;
    v.widgets.inactive.weak_bg_fill = farbe::FLAECHE_HELL;
    v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    v.widgets.inactive.corner_radius = r;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, farbe::TEXT);

    v.widgets.hovered.bg_fill = Color32::from_rgb(44, 48, 58);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(44, 48, 58);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, farbe::AKZENT_LEISE);
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);

    v.widgets.active.bg_fill = farbe::AKZENT_LEISE;
    v.widgets.active.weak_bg_fill = farbe::AKZENT_LEISE;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, farbe::AKZENT);
    v.widgets.active.corner_radius = r;
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);

    v.widgets.open.bg_fill = farbe::FLAECHE_HELL;
    v.widgets.open.corner_radius = r;

    v.selection.bg_fill = farbe::AKZENT_LEISE;
    v.selection.stroke = egui::Stroke::new(1.0_f32, farbe::AKZENT);
    v.slider_trailing_fill = true; // gefuellter Reglerweg — zeigt den Wert schon im Umriss
    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.slider_width = 118.0;
    style.spacing.item_spacing = egui::vec2(9.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(14);
    style.spacing.indent = 14.0;
    style.visuals.indent_has_left_vline = false;
    // Etwas groessere Grundschrift und klar abgestufte Ueberschriften: der
    // wichtigste Hebel gegen den „hoelzernen" Eindruck.
    use egui::{FontFamily::Proportional, FontId, TextStyle::*};
    style.text_styles = [
        (Heading, FontId::new(15.0, Proportional)),
        (Body, FontId::new(13.0, Proportional)),
        (Monospace, FontId::new(12.0, egui::FontFamily::Monospace)),
        (Button, FontId::new(13.0, Proportional)),
        (Small, FontId::new(11.0, Proportional)),
    ]
    .into();
    ctx.set_style(style);
}

use eframe::egui::Color32;

/// Filter der Galerie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Alle,
    Bearbeitet,
    Unbearbeitet,
    MitSternen,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::Alle => t("Alle"),
            Filter::Bearbeitet => t("Bearbeitet"),
            Filter::Unbearbeitet => t("Offen"),
            Filter::MitSternen => t("★ bewertet"),
        }
    }
}

/// Was ein Arbeitsfaden zurueckmeldet.
enum Msg {
    Preview {
        path: PathBuf,
        raw: Box<rawcore::RawImage>,
        /// Die fertig verkleinerte Arbeitskopie (f16-RGBA) — im Lade-Thread
        /// gerechnet. Auf dem UI-Thread bleibt nur noch der GPU-Upload; alles
        /// andere hiess vorher: Programm steht eine halbe Sekunde.
        klein: (u32, u32, Vec<u16>),
        /// Fertige Startwerte (Grundhelligkeit, ISO-Rauschminderung …) — im
        /// Lade-Thread ueber `startwerte()` gerechnet. EINE Stelle: jede
        /// von Hand nachgebaute Teilmenge hat frueher etwas vergessen.
        start: Edit,
        /// Erkanntes Objektiv (Profil + gemessene Randabdunklung).
        objektiv: pipeline::objektiv::Objektiv,
    },
    /// Neu aufgebaute Arbeitskopie nach einer Retusche-Aenderung.
    Arbeitskopie {
        path: PathBuf,
        klein: (u32, u32, Vec<u16>),
    },
    PreviewFailed {
        path: PathBuf,
        error: String,
    },
    Thumb {
        path: PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Batch(export::BatchMsg),
    /// Fortschritt der KI-Installation (Text, Anteil 0..1).
    KiFortschritt(String, f32),
    /// Installation beendet (Fehlertext oder Erfolg).
    KiInstalliert(Result<(), String>),
    GenFortschritt(String, f32),
    GenInstalliert(Result<(), String>),
    /// Ein erzeugtes Bild liegt bereit (Pfad der PNG-Datei).
    GenFertig(Result<PathBuf, String>),
    /// Panorama: Fortschritt (Anteil, Text) / fertig.
    PanoFortschritt(f32, String),
    PanoFertig(Result<PathBuf, String>),
    /// Updater: neuere Version gefunden / Ladefortschritt / fertig installiert.
    UpdateGefunden(updater::Neu),
    UpdateFortschritt(u64, u64),
    UpdateFertig(Result<std::process::Command, String>),
    /// Ein eingesetztes Stueck (Hinzufuegen) ist fertig.
    ZusatzFertig { path: PathBuf, ergebnis: Result<pipeline::params::Zusatz, String> },
    Ai {
        path: PathBuf,
        result: Box<Result<ai::AiResult, String>>,
    },
    /// Eine Zeile fuers KI-Protokoll, waehrend die Arbeit noch laeuft.
    Protokoll(LogZeile),
}

/// Eine Zeile im KI-Protokoll — nach dem Muster der Web-Version (`aiLog`):
/// Zeitstempel, kurze Meldung, optional ein Detail zum Aufklappen.
#[derive(Debug, Clone)]
pub struct LogZeile {
    /// Sekunden seit Beginn des Laufs. Bewusst relativ statt Uhrzeit: bei einem
    /// mehrstufigen Lauf will man wissen, wie lange die Runden gedauert haben.
    pub bei: f32,
    pub art: LogArt,
    pub text: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogArt {
    Info,
    Erfolg,
    Fehler,
}

impl LogArt {
    fn farbe(self) -> Color32 {
        match self {
            LogArt::Info => farbe::TEXT_LEISE,
            LogArt::Erfolg => farbe::AKZENT,
            LogArt::Fehler => Color32::from_rgb(230, 120, 110),
        }
    }
}

struct App {
    gpu: Option<Arc<Gpu>>,
    gpu_error: Option<String>,

    gallery: Vec<PathBuf>,
    current: Option<usize>,
    edits: HashMap<PathBuf, Edit>,
    /// Grundstand je Foto (Automatik bzw. XMP-Einstellungen) — Basis fuer
    /// Vorlagen und „Zuruecksetzen".
    grundstaende: HashMap<PathBuf, Edit>,
    selected: Vec<bool>,

    /// Aktuell geladenes Bild auf der GPU (verkleinerte Arbeitskopie).
    image: Option<GpuImage>,
    image_meta: Option<rawcore::RawMeta>,
    /// Das volle RAW bleibt im Speicher — nur so kann der Zoom echte Originalpixel
    /// zeigen statt die Vorschau aufzublasen.
    raw_full: Option<std::sync::Arc<rawcore::RawImage>>,
    /// 1.0 = eingepasst, groesser = hineingezoomt (1:1 bei `zoom_native`).
    zoom: f32,
    pan: egui::Vec2,
    detail: Option<GpuImage>,
    detail_rect: Option<(u32, u32, u32, u32)>,
    /// Der nachgeladene Ausschnitt als eigene Textur — bewusst NEBEN der Vorschau
    /// und nicht statt ihr: er deckt nur einen Teil des Bildes ab.
    detail_tex: Option<egui::TextureHandle>,
    texture: Option<egui::TextureHandle>,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumbs_pending: std::collections::HashSet<PathBuf>,
    /// Welche Fotos eine Begleitdatei auf der Platte haben — einmal beim Oeffnen
    /// ermittelt. `is_edited` fragte sonst die Festplatte, und zwar fuer jedes
    /// Foto in jedem gezeichneten Frame: bei einem Ordner vom NAS stand das
    /// Programm damit komplett.
    hat_sidecar: std::collections::HashSet<PathBuf>,
    /// Welches Foto der Filmstreifen zuletzt zentriert hat — bei Wechsel
    /// rollt er zum aktuellen Bild.
    film_zuletzt: Option<usize>,
    /// Die zuletzt fertig entwickelte Vorschau, klein — damit Galerie und
    /// Filmstreifen nach dem Bearbeiten das ENTWICKELTE Foto zeigen, nicht den
    /// Rohzustand (so machen es auch gaengige Raw-Entwickler).
    thumb_aktuell: Option<(u32, u32, Vec<u8>)>,

    loading: Option<PathBuf>,
    status: String,
    last_render_ms: f32,
    histogram: [ [f32; 64]; 3],
    needs_render: bool,

    undo: Vec<Edit>,
    redo: Vec<Edit>,

    /// Sternebewertung je Foto (0..5). Wird in der Begleitdatei gesichert.
    stars: HashMap<PathBuf, u8>,
    fullscreen: bool,
    crop_mode: bool,
    crop_drag: Option<(egui::Pos2, egui::Pos2)>,

    /// Wann die Maus zuletzt bewegt wurde — im Vollbild blenden sich die
    /// schwebenden Leisten danach aus, damit nur noch das Foto da ist.
    maus_zuletzt: std::time::Instant,
    /// Reparaturpinsel aktiv („Objekte entfernen").
    heal_mode: bool,
    /// Weissabgleich-Pipette aktiv (W): der naechste Klick ins Bild waehlt
    /// eine neutrale Stelle.
    pipette: bool,
    /// Clipping-Anzeige (J): ausgefressene Lichter rot, abgesoffene Tiefen blau.
    clipping: bool,
    /// Kopierte Einstellungen (⇧⌘C) fuer andere Fotos (⇧⌘V).
    zwischenablage: Option<Edit>,
    /// Laeuft gerade ein Reglerzug? Ein Zug = EIN Rueckgaengig-Schritt.
    zug_laeuft: bool,
    /// Ausgefressene/abgesoffene Anteile der letzten Vorschau (fuer die Warnung).
    clip_anteil: (f32, f32),
    /// Pinselgroesse als Anteil der Bildbreite.
    heal_radius: f32,
    /// Die Arbeitskopie auf der GPU muss neu aufgebaut werden. Noetig, weil die
    /// Retusche vor allen Reglern sitzt — sie steckt in den Pixeln, nicht im Shader.
    needs_reupload: bool,
    /// Ein Hintergrund-Thread baut die Arbeitskopie gerade neu auf.
    reupload_laeuft: bool,
    /// Ein Decode-Thread laeuft gerade. Es laeuft immer nur EINER: jedes volle
    /// RAW belegt beim Dekodieren um die 260 MB — wer schnell durch zehn Fotos
    /// blaettert, haette sonst zehn Threads und Gigabytes gleichzeitig im
    /// Speicher (gemeldete Speicherprobleme).
    decode_laeuft: bool,
    /// Waehrend ein Decode laeuft, wartet hier hoechstens der NEUESTE Wunsch —
    /// Zwischenschritte beim Blaettern interessieren nicht mehr.
    decode_naechster: Option<PathBuf>,

    /// Adresse des KI-Servers; leer = keine KI (siehe `ki_endpunkt`).
    /// Lokale KI installiert? (Knopf „Magisch entwickeln" statt „installieren")
    ki_installiert: bool,
    /// Laufende Installation: Fortschrittstext und Anteil.
    ki_laedt: Option<(String, f32)>,
    ki_abbruch: Arc<std::sync::atomic::AtomicBool>,
    /// Neue Retusche-Stellen fuellt die KI (sonst klassisch kopieren).
    heal_ki_modus: bool,
    /// Panorama: Fenster, Optionen, laufender Auftrag, Ergebnis.
    pano_fenster: bool,
    pano_projektion: pipeline::panorama::Projektion,
    pano_belichtung: bool,
    pano_parallaxe: bool,
    pano_fuellen: bool,
    pano_zuschneiden: bool,
    pano_volle_aufloesung: bool,
    pano_laeuft: Option<(f32, String)>,
    pano_ergebnis: Option<PathBuf>,
    pano_meldung: String,
    /// Updater: gefundene neuere Version, laufender Download, Meldung.
    update_neu: Option<updater::Neu>,
    update_laedt: Option<(u64, u64)>,
    update_meldung: String,
    update_geprueft: bool,
    update_abbruch: Arc<std::sync::atomic::AtomicBool>,
    /// Hinzufuegen: Pinsel markiert den Bereich, der Generator setzt ein.
    hinzufuegen_modus: bool,
    zusatz_flecken: Vec<[f32; 3]>,
    zusatz_text: String,
    zusatz_seit: Option<std::time::Instant>,
    zusatz_meldung: String,
    /// Bildgenerator (Bonus): Fenster, Installation, Auftrag, Ergebnis.
    gen_fenster: bool,
    gen_installiert: bool,
    gen_laedt: Option<(String, f32)>,
    gen_abbruch: Arc<std::sync::atomic::AtomicBool>,
    gen_text: String,
    gen_format: generator::Format,
    gen_seed: u64,
    gen_seit: Option<std::time::Instant>,
    gen_ergebnis: Option<(PathBuf, egui::TextureHandle)>,
    gen_meldung: String,
    ai_strength: ai::Strength,
    ai_busy: bool,
    ai_note: String,
    /// Reglerstand von VOR dem KI-Start — wird erst am Ende als
    /// Rueckgaengig-Schritt abgelegt, weil die Zwischenstaende der Runden
    /// den aktuellen Stand fortlaufend ueberschreiben.
    ai_undo: Option<Edit>,
    /// Wie oft die KI ihr eigenes Ergebnis nachkontrolliert (0–2), wie im Web.
    /// Das mitlaufende KI-Protokoll und ob es gerade zu sehen ist.
    ai_log: Vec<LogZeile>,
    show_log: bool,
    /// Vorher/Nachher: zeigt kurzzeitig den unbearbeiteten Stand.
    show_before: bool,
    /// Galerieansicht statt Einzelbild.
    gallery_view: bool,
    filter: Filter,

    /// Ungesicherte Aenderungen — die Begleitdatei wird verzoegert geschrieben,
    /// sonst schriebe jeder Reglerzug eine Datei.
    dirty: bool,
    dirty_at: Option<std::time::Instant>,

    show_batch: bool,
    batch: export::ExportSettings,
    /// Vollautomatik: jedes Foto bekommt SEINE eigenen gemessenen Startwerte
    /// (Belichtung/Lichter/Schatten/Schwarz/Rauschen), wie der Kommandozeilen-
    /// Stapel. Aus = die aktuelle Einstellung des offenen Bildes auf alle anwenden.
    batch_auto: bool,
    /// Im Stapel jedes Foto einzeln von der KI entwickeln lassen.
    batch_ai: bool,
    batch_running: bool,
    batch_done: usize,
    batch_total: usize,
    batch_log: Vec<String>,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Der egui-Kontext, gemerkt beim ersten Zeichnen — gebraucht, um ausserhalb
    /// von `update` (z. B. beim Sichern) eine Textur anlegen zu koennen.
    egui_ctx: Option<egui::Context>,
}

impl App {
    fn new() -> Self {
        let (tx, rx) = channel();
        // Der Bildschirmname kommt aus dem Farbprofil — in der Sprache der
        // Oberflaeche (vor dem Lesen setzen, das passiert in Gpu::new).
        pipeline::monitor::namen_auf_englisch(sprache::aktuell() == sprache::Sprache::En);
        let (gpu, gpu_error) = match pollster::block_on(Gpu::new()) {
            Ok(g) => (Some(Arc::new(g)), None),
            Err(e) => (None, Some(sprache::gpu_fehler(&e))),
        };
        let status = grundstatus(gpu.as_deref());
        Self {
            gpu,
            gpu_error,
            gallery: Vec::new(),
            current: None,
            edits: HashMap::new(),
            grundstaende: HashMap::new(),
            selected: Vec::new(),
            image: None,
            image_meta: None,
            raw_full: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            detail: None,
            detail_rect: None,
            detail_tex: None,
            texture: None,
            thumbs: HashMap::new(),
            thumbs_pending: Default::default(),
            hat_sidecar: Default::default(),
            film_zuletzt: None,
            thumb_aktuell: None,
            loading: None,
            status,
            last_render_ms: 0.0,
            histogram: [[0.0; 64]; 3],
            needs_render: false,
            undo: Vec::new(),
            redo: Vec::new(),
            stars: HashMap::new(),
            fullscreen: false,
            crop_mode: false,
            crop_drag: None,
            maus_zuletzt: std::time::Instant::now(),
            heal_mode: false,
            pipette: false,
            clipping: false,
            zwischenablage: None,
            zug_laeuft: false,
            clip_anteil: (0.0, 0.0),
            heal_radius: 0.012,
            needs_reupload: false,
            reupload_laeuft: false,
            decode_laeuft: false,
            decode_naechster: None,
            // Die KI entwickelt server-seitig — dort liegen die Zugangsdaten. Genau
            // eine Stelle damit, statt einer Kopie auf jedem Rechner.
            // ⚠️ Die Adresse als IP angeben, nicht als lokalen Rechnernamen: den
            // loest zwar curl (ueber mDNS/Bonjour) auf, Rusts Namensaufloesung aber
            // nicht — die KI wuerde sonst still uebersprungen.
            ki_installiert: ki_lokal::installiert(),
            ki_laedt: None,
            ki_abbruch: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            heal_ki_modus: ki_lokal::retusche_modell().is_some(),
            pano_fenster: false,
            pano_projektion: pipeline::panorama::Projektion::Zylinder,
            pano_belichtung: true,
            pano_parallaxe: true,
            pano_fuellen: false,
            pano_zuschneiden: true,
            pano_volle_aufloesung: false,
            pano_laeuft: None,
            pano_ergebnis: None,
            pano_meldung: String::new(),
            update_neu: None,
            update_laedt: None,
            update_meldung: String::new(),
            update_geprueft: false,
            update_abbruch: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            hinzufuegen_modus: false,
            zusatz_flecken: Vec::new(),
            zusatz_text: String::new(),
            zusatz_seit: None,
            zusatz_meldung: String::new(),
            gen_fenster: false,
            gen_installiert: generator::installiert(),
            gen_laedt: None,
            gen_abbruch: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            gen_text: String::new(),
            gen_format: generator::Format::Quer,
            gen_seed: 1,
            gen_seit: None,
            gen_ergebnis: None,
            gen_meldung: String::new(),
            ai_strength: ai::Strength::Normal,
            ai_busy: false,
            ai_note: String::new(),
            ai_undo: None,
            ai_log: Vec::new(),
            show_log: false,
            show_before: false,
            gallery_view: false,
            filter: Filter::Alle,
            dirty: false,
            dirty_at: None,
            show_batch: false,
            batch: Default::default(),
            batch_auto: true,
            batch_ai: false,
            batch_running: false,
            batch_done: 0,
            batch_total: 0,
            batch_log: Vec::new(),
            tx,
            rx,
            egui_ctx: None,
        }
    }

    /// Gilt ein Foto als bearbeitet? Grundlage ist die Begleitdatei: sobald
    /// jemand etwas verstellt hat, weicht sie vom Startzustand ab.
    fn is_edited(&self, path: &Path) -> bool {
        match self.edits.get(path) {
            // Seit Prozess 2 ein echtes Kennzeichen: die Automatik setzt
            // sichtbare Reglerwerte, die zaehlen nicht als Bearbeitung.
            Some(e) if e.prozess >= pipeline::params::PROZESS => e.bearbeitet,
            Some(e) => alt_bearbeitet(e),
            // ⚠️ Hier stand `sidecar_path(path).exists()` — ein Festplattenzugriff.
            // Diese Funktion laeuft aber fuer JEDES Foto in JEDEM Frame (Galerie-
            // Kopfzeile: viermal). Bei einem Ordner vom NAS hiess das tausende
            // SMB-Zugriffe pro Frame — das Programm fror ein und reagierte nicht
            // mehr. Deshalb: einmal beim Oeffnen nachsehen, danach nur Speicher.
            None => self.hat_sidecar.contains(path),
        }
    }

    /// Reihenfolge der Fotos nach aktuellem Filter.
    fn filtered(&self) -> Vec<usize> {
        (0..self.gallery.len())
            .filter(|i| {
                let p = &self.gallery[*i];
                match self.filter {
                    Filter::Alle => true,
                    Filter::Bearbeitet => self.is_edited(p),
                    Filter::Unbearbeitet => !self.is_edited(p),
                    Filter::MitSternen => self.stars.get(p).copied().unwrap_or(0) > 0,
                }
            })
            .collect()
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.current.and_then(|i| self.gallery.get(i).cloned())
    }

    fn edit(&mut self) -> &mut Edit {
        let path = self.current_path().unwrap_or_default();
        // Wer hier schreibt, ist ein Nutzerschritt (Zuschnitt, Retusche,
        // Drehung, Vorlage) — also ist das Foto ab jetzt bearbeitet.
        let e = self.edits.entry(path).or_insert_with(presets::start_edit);
        e.bearbeitet = true;
        e
    }

    fn edit_ref(&self) -> Edit {
        self.current_path()
            .and_then(|p| self.edits.get(&p).cloned())
            .unwrap_or_else(presets::start_edit)
    }

    fn push_undo(&mut self) {
        let e = self.edit_ref();
        self.undo.push(e);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let mut files: Vec<PathBuf> = Vec::new();
        for p in paths {
            if p.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    let mut found: Vec<PathBuf> = rd
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| rawcore::is_raw_path(p))
                        .collect();
                    found.sort();
                    files.extend(found);
                }
            } else if rawcore::is_raw_path(&p) {
                files.push(p);
            }
        }
        if files.is_empty() {
            self.status = t("Keine RAW-Dateien gefunden.").into();
            return;
        }
        // Begleitdateien gleich mitlesen: nur so weiss die Galerie beim Oeffnen
        // schon, was bearbeitet ist, ohne jedes Foto anzufassen.
        for f in &files {
            if let Some((edit, stars)) = load_sidecar(f) {
                self.hat_sidecar.insert(f.clone());
                self.edits.insert(f.clone(), edit);
                if stars > 0 {
                    self.stars.insert(f.clone(), stars);
                }
            }
        }
        self.selected = vec![false; files.len()];
        // Miniaturen des vorherigen Ordners freigeben — GPU-Texturen sammeln
        // sich sonst ueber den Arbeitstag an (jeder Ordner ~100 MB).
        let neu: std::collections::HashSet<&PathBuf> = files.iter().collect();
        self.thumbs.retain(|p, _| neu.contains(p));
        self.gallery = files;
        self.zaehler_auffrischen();
        self.select(0);
    }

    fn select(&mut self, index: usize) {
        if index >= self.gallery.len() {
            return;
        }
        self.save_current_sidecar();
        self.current = Some(index);
        self.undo.clear();
        self.redo.clear();
        let path = self.gallery[index].clone();
        if let Some((edit, stars)) = load_sidecar(&path) {
            self.edits.insert(path.clone(), edit);
            if stars > 0 {
                self.stars.insert(path.clone(), stars);
            }
        }
        self.loading = Some(path.clone());
        self.texture = None;
        self.image = None;
        self.raw_full = None;
        self.detail = None;
        self.detail_rect = None;
        self.detail_tex = None;
        self.thumb_aktuell = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.starte_decode(path);
    }

    /// Startet das Dekodieren — oder merkt es vor, wenn schon eines laeuft.
    fn starte_decode(&mut self, path: PathBuf) {
        if self.decode_laeuft {
            self.decode_naechster = Some(path);
            return;
        }
        self.decode_laeuft = true;
        let tx = self.tx.clone();
        // Bereits gesicherte Retuschestellen kennt der Haupt-Thread — mitgeben,
        // damit der Lade-Thread die Arbeitskopie fertig aufbauen kann.
        let (heal, heal_ki, zusaetze) = self
            .edits
            .get(&path)
            .map(|e| (e.heal.clone(), e.heal_ki.clone(), e.zusaetze.clone()))
            .unwrap_or_default();
        std::thread::spawn(move || {
            match rawcore::decode(&path, &rawcore::DecodeOptions::default()) {
                Ok(raw) => {
                    // Alles Teure passiert HIER, nicht auf dem UI-Thread:
                    // Verkleinern (40 MP!), Retusche einrechnen, Belichtung messen,
                    // Objektiv erkennen (mit Vignette-Messung am Bild).
                    let stuecke = zusatz::stuecke(&path, &zusaetze);
                    let retusche = pipeline::Retusche { heal: &heal, heal_ki: &heal_ki, zusaetze: &stuecke };
                    let klein = GpuImage::vorbereiten(&raw, Some(PREVIEW_MAX), &retusche);
                    let start = grundstand(&path, &raw);
                    let objektiv = pipeline::objektiv::erkennen(&raw);
                    match klein {
                        Ok(klein) => {
                            let _ = tx.send(Msg::Preview {
                                path,
                                raw: Box::new(raw),
                                klein,
                                start,
                                objektiv,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::PreviewFailed {
                                path,
                                error: sprache::gpu_fehler(&e),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::PreviewFailed {
                        path,
                        error: sprache::raw_fehler(&e),
                    });
                }
            }
        });
    }

    fn request_thumb(&mut self, path: &Path) {
        if self.thumbs.contains_key(path) || self.thumbs_pending.contains(path) {
            return;
        }
        // Nicht mehr als eine Handvoll gleichzeitig — sonst blockieren 200 Fotos
        // beim Oeffnen eines Ordners alle Kerne und die Oberflaeche ruckelt.
        if self.thumbs_pending.len() >= 4 {
            return;
        }
        self.thumbs_pending.insert(path.to_path_buf());
        let tx = self.tx.clone();
        let p = path.to_path_buf();
        std::thread::spawn(move || {
            // Erst in der Ablage nachsehen — das ist der Unterschied zwischen
            // „Projekt oeffnet sofort" und „Projekt rechnet erst mal drei Minuten".
            let cache = thumb_cache_path(&p);
            if let Ok(bytes) = std::fs::read(&cache) {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let _ = tx.send(Msg::Thumb {
                        path: p,
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                    });
                    return;
                }
            }
            // Schnellweg: die eingebettete Kamera-Vorschau (ein grosses JPEG in der
            // RAF) — Millisekunden statt ein bis zwei Sekunden. Genau das war die
            // Wurzel von 100 % CPU-Last und troepfelnden Vorschauen: bisher wurde fuer JEDE
            // Miniatur das komplette 40-MP-RAW entwickelt. Der Look ist der Kamera-
            // Look (der eingestellte Bildmodus); sobald ein Foto geoeffnet/entwickelt
            // wird, ersetzt die grosse Ansicht die Miniatur ohnehin durch den
            // Fotowache-Look.
            if let Some(t) = rawcore::embedded_thumb(&p) {
                if let Ok(img) = image::load_from_memory(&t.jpeg) {
                    let voll = img.to_rgba8();
                    let (mut w, mut h, mut px) =
                        verkleinere_rgba(&voll, voll.width(), voll.height(), THUMB_MAX);
                    // Kameralage genau wie in der grossen Ansicht und in quick_thumb:
                    // das eingebettete JPEG liegt in Sensor-Lage, die Drehung machen wir.
                    let dreh = drehung_aus_kamera(t.flip);
                    if dreh != 0 {
                        let (gd, nw, nh) = pipeline::ausrichten(&px, w, h, 4, dreh, false, false);
                        w = nw;
                        h = nh;
                        px = gd;
                    }
                    if let Some(buf) = image::RgbaImage::from_raw(w, h, px.clone()) {
                        let _ = image::DynamicImage::ImageRgba8(buf).to_rgb8().save(&cache);
                    }
                    let _ = tx.send(Msg::Thumb { path: p, width: w, height: h, rgba: px });
                    return;
                }
            }
            // Rueckfall: keine eingebettete JPEG-Vorschau (oder unlesbar) — dann
            // doch die volle Entwicklung (halbe Aufloesung, schnelles Demosaic).
            let opts = rawcore::DecodeOptions {
                demosaic: rawcore::DemosaicQuality::Schnell,
                half_size: true,
                ..Default::default()
            };
            if let Ok(raw) = rawcore::decode(&p, &opts) {
                let (w, h, rgba) = quick_thumb(&raw, THUMB_MAX);
                if let Some(buf) = image::RgbaImage::from_raw(w, h, rgba.clone()) {
                    let _ = image::DynamicImage::ImageRgba8(buf).to_rgb8().save(&cache);
                }
                let _ = tx.send(Msg::Thumb {
                    path: p,
                    width: w,
                    height: h,
                    rgba,
                });
            }
        });
    }

    fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Preview {
                    path,
                    raw,
                    klein,
                    start,
                    objektiv,
                } => {
                    self.decode_laeuft = false;
                    if let Some(n) = self.decode_naechster.take() {
                        self.starte_decode(n);
                    }
                    if self.loading.as_ref() == Some(&path) {
                        self.loading = None;
                        if let Some(gpu) = &self.gpu {
                            let (kw, kh, daten) = klein;
                            match GpuImage::aus_vorbereitet(gpu, kw, kh, &daten, &raw.meta, raw.width) {
                                Ok(mut img) => {
                                    img.objektiv = objektiv;
                                    self.image = Some(img);
                                    self.image_meta = Some(raw.meta.clone());
                                    self.raw_full = Some(std::sync::Arc::new(*raw));
                                    self.zoom = 1.0;
                                    self.pan = egui::Vec2::ZERO;
                                    self.detail = None;
                                    self.detail_rect = None;
                                    self.detail_tex = None;
                                    // Grundhelligkeit nur bei NEUEN Fotos setzen —
                                    // eine gespeicherte Bearbeitung darf nie
                                    // nachtraeglich aufgehellt werden. AUSNAHME:
                                    // eine Begleitdatei ganz ohne Grundausstattung
                                    // (base_ev 0 — entstanden durch das fruehere
                                    // Blaetter-Leck) wird hier geheilt, sonst
                                    // bliebe das Foto fuer immer abgesoffen.
                                    self.grundstaende.insert(path.clone(), start.clone());
                                    if self
                                        .edits
                                        .get(&path)
                                        .is_some_and(|e| e.prozess < pipeline::params::PROZESS)
                                    {
                                        sichere_v1(&path);
                                    }
                                    let geheilt = match self.edits.entry(path) {
                                        std::collections::hash_map::Entry::Occupied(mut o) => {
                                            startwerte_nachziehen(o.get_mut(), &start)
                                        }
                                        std::collections::hash_map::Entry::Vacant(v) => {
                                            v.insert(start);
                                            false
                                        }
                                    };
                                    if geheilt {
                                        self.mark_dirty();
                                    }
                                    self.needs_render = true;
                                }
                                Err(e) => self.status = format!("GPU: {}", sprache::gpu_fehler(&e)),
                            }
                        }
                    }
                }
                Msg::Arbeitskopie { path, klein } => {
                    self.reupload_laeuft = false;
                    // Nur uebernehmen, wenn das Foto noch das aktuelle ist — sonst
                    // ersetzt eine alte Retusche das inzwischen gewechselte Bild.
                    if self.current_path().as_deref() == Some(path.as_path()) {
                        if let (Some(gpu), Some(meta)) = (&self.gpu, &self.image_meta) {
                            let (kw, kh, daten) = klein;
                            let sensor_breite = self.raw_full.as_ref().map(|r| r.width).unwrap_or(kw);
                            match GpuImage::aus_vorbereitet(gpu, kw, kh, &daten, meta, sensor_breite) {
                                Ok(mut img) => {
                                    // Objektiv der alten Arbeitskopie uebernehmen —
                                    // sonst verloere eine Retusche die Entzerrung.
                                    if let Some(alt) = &self.image {
                                        img.objektiv = alt.objektiv;
                                    }
                                    self.image = Some(img);
                                    self.detail = None;
                                    self.detail_rect = None;
                                    self.detail_tex = None;
                                    self.needs_render = true;
                                }
                                Err(e) => self.status = format!("GPU: {}", sprache::gpu_fehler(&e)),
                            }
                        }
                    }
                }
                Msg::PreviewFailed { path, error } => {
                    self.decode_laeuft = false;
                    if let Some(n) = self.decode_naechster.take() {
                        self.starte_decode(n);
                    }
                    if self.loading.as_ref() == Some(&path) {
                        self.loading = None;
                        self.status = format!("{}: {error}", name_of(&path));
                    }
                }
                Msg::Thumb {
                    path,
                    width,
                    height,
                    rgba,
                } => {
                    self.thumbs_pending.remove(&path);
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [width as usize, height as usize],
                        &rgba,
                    );
                    let tex = ctx.load_texture(
                        path.to_string_lossy().to_string(),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(path, tex);
                }
                Msg::Batch(b) => self.on_batch(b),
                Msg::Protokoll(z) => self.ai_log.push(z),
                Msg::KiFortschritt(text, anteil) => self.ki_laedt = Some((text, anteil)),
                Msg::GenFortschritt(text, anteil) => self.gen_laedt = Some((text, anteil)),
                Msg::UpdateGefunden(neu) => self.update_neu = Some(neu),
                Msg::PanoFortschritt(anteil, text) => self.pano_laeuft = Some((anteil, text)),
                Msg::PanoFertig(ergebnis) => {
                    let bericht = self.pano_laeuft.take().map(|(_, t)| t).unwrap_or_default();
                    match ergebnis {
                        Ok(p) => {
                            self.pano_meldung = bericht;
                            self.pano_ergebnis = Some(p);
                        }
                        Err(e) => self.pano_meldung = e,
                    }
                }
                Msg::UpdateFortschritt(b, g) => self.update_laedt = Some((b, g)),
                Msg::UpdateFertig(ergebnis) => {
                    self.update_laedt = None;
                    match ergebnis {
                        Ok(mut start) => {
                            // Bearbeitung sichern, KI-Dienst beenden, neue Fassung
                            // starten, diese beenden.
                            self.save_current_sidecar();
                            ki_lokal::beenden();
                            match start.spawn() {
                                Ok(_) => std::process::exit(0),
                                Err(e) => self.update_meldung = tf("Neustart: {}", &[&e]),
                            }
                        }
                        Err(e) => self.update_meldung = tf("Update: {}", &[&e]),
                    }
                }
                Msg::ZusatzFertig { path, ergebnis } => {
                    self.zusatz_seit = None;
                    match ergebnis {
                        Ok(z) => {
                            if self.current_path().as_ref() == Some(&path) {
                                self.push_undo();
                            }
                            let mut e = self.edits.get(&path).cloned().unwrap_or_else(|| self.edit_ref());
                            e.zusaetze.push(z);
                            e.bearbeitet = true;
                            self.edits.insert(path, e);
                            self.zusatz_flecken.clear();
                            self.zusatz_meldung = t("Eingesetzt.").into();
                            self.needs_reupload = true;
                            self.mark_dirty();
                        }
                        Err(e) => self.zusatz_meldung = e,
                    }
                }
                Msg::GenInstalliert(ergebnis) => {
                    self.gen_laedt = None;
                    match ergebnis {
                        Ok(()) => self.gen_installiert = true,
                        Err(e) => self.gen_meldung = tf("Installation: {}", &[&e]),
                    }
                }
                Msg::GenFertig(ergebnis) => {
                    self.gen_seit = None;
                    match ergebnis.and_then(|p| {
                        let img = image::open(&p).map_err(|e| e.to_string())?.to_rgba8();
                        let (w, h) = img.dimensions();
                        let farbbild = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
                        Ok((p, farbbild))
                    }) {
                        Ok((p, farbbild)) => {
                            let tex = ctx.load_texture("generator", farbbild, egui::TextureOptions::LINEAR);
                            self.gen_ergebnis = Some((p, tex));
                            self.gen_meldung.clear();
                        }
                        Err(e) => self.gen_meldung = e,
                    }
                }
                Msg::KiInstalliert(ergebnis) => {
                    self.ki_laedt = None;
                    match ergebnis {
                        Ok(()) => {
                            self.ki_installiert = true;
                            self.heal_ki_modus = true;
                            self.status = t("Lokale KI ist installiert.").into();
                        }
                        Err(e) => self.status = tf("KI-Installation: {}", &[&e]),
                    }
                }
                Msg::Ai { path, result } => {
                    self.ai_busy = false;
                    match *result {
                        Ok(r) => {
                            // Der Rueckgaengig-Stand ist der Schnappschuss von VOR
                            // dem KI-Start — die Zwischenstaende haben `edits`
                            // laengst ueberschrieben, `edit_ref()` waere hier
                            // schon das KI-Ergebnis.
                            if let Some(vorher) = self.ai_undo.take() {
                                self.undo.push(vorher);
                            }
                            self.ai_note = if r.scene_label.is_empty() {
                                tf("KI: {} Regler gesetzt", &[&r.applied.len()])
                            } else {
                                format!("{} · {} — {}", r.scene_label, r.look, r.description)
                            };
                            let mut fertig = r.edit;
                            fertig.bearbeitet = true;
                            self.edits.insert(path, fertig);
                            self.needs_render = true;
                            self.mark_dirty();
                        }
                        Err(e) => {
                            self.ai_undo = None;
                            self.ai_note = e;
                        }
                    }
                }
            }
        }
    }

    fn on_batch(&mut self, msg: export::BatchMsg) {
        use export::BatchMsg::*;
        match msg {
            Started { total } => {
                self.batch_running = true;
                self.batch_total = total;
                self.batch_done = 0;
                self.batch_log.clear();
            }
            Done { file, target, .. } => {
                self.batch_done += 1;
                self.batch_log
                    .push(tf("{} wurde {}", &[&name_of(&file), &name_of(&target)]));
            }
            Failed { file, error, .. } => {
                self.batch_done += 1;
                self.batch_log
                    .push(tf("FEHLER bei {}: {}", &[&name_of(&file), &error]));
            }
            Finished { ok, failed } => {
                self.batch_running = false;
                self.status = if failed == 0 {
                    tf("Stapel fertig: {} Fotos entwickelt", &[&ok])
                } else {
                    tf("Stapel fertig: {} ok, {} fehlgeschlagen", &[&ok, &failed])
                };
            }
        }
    }

    fn render_preview(&mut self, ctx: &egui::Context) {
        let (Some(gpu), Some(img)) = (&self.gpu, &self.image) else {
            return;
        };
        let edit = if self.show_before {
            // „Vorher" vergleicht den LOOK, nicht die Geometrie: Grundhelligkeit,
            // Zuschnitt, Drehung und Geraderichten bleiben stehen — sonst springt
            // beim Halten des Knopfs ein Hochformat kurz ins Querformat, und der
            // Zuschnitt saesse an der falschen Stelle.
            let cur = self.edit_ref();
            let mut e = presets::start_edit();
            e.base_ev = cur.base_ev;
            e.crop = cur.crop;
            e.rotate = cur.rotate;
            e.flip_h = cur.flip_h;
            e.flip_v = cur.flip_v;
            e.straighten = cur.straighten;
            e.lens_auto = cur.lens_auto;
            e.lens_profil = cur.lens_profil.clone();
            e
        } else {
            self.edit_ref()
        };
        let t = std::time::Instant::now();
        // `render_screen` statt `render`: die Vorschau geht ins erkannte
        // Monitorprofil, nicht nach sRGB. Der Export bleibt sRGB — siehe
        // `pipeline::monitor`.
        let rgba = img.render_screen(gpu, &edit);
        self.last_render_ms = t.elapsed().as_secs_f32() * 1000.0;
        // Histogramm aus dem ungedrehten Bild: eine Drehung sortiert nur Pixel um,
        // die Verteilung bleibt gleich — und so bleibt es billig.
        self.histogram = histogram(&rgba);
        let mut rgba = rgba;
        self.clip_anteil = clipping_markieren(&mut rgba, self.clipping && !self.show_before);
        // Drehen/Spiegeln zuletzt und auf der CPU, verlustfrei.
        let (rgba, pw, ph) = pipeline::ausrichten(
            &rgba,
            img.width,
            img.height,
            4,
            edit.rotate,
            edit.flip_h,
            edit.flip_v,
        );
        // Zuschnitt: **genau dieselbe Reihenfolge wie beim Export** (erst drehen,
        // dann schneiden), sonst zeigt die Vorschau etwas anderes als die Datei.
        //
        // Waehrend man den Rahmen zieht, bleibt das ganze Bild stehen — man muss ja
        // sehen, was man wegschneidet. Erst beim Verlassen des Werkzeugs wird der
        // Ausschnitt gezeigt.
        let (rgba, pw, ph) = if self.crop_mode {
            (rgba, pw, ph)
        } else {
            export::crop_rgba(&rgba, pw, ph, edit.crop)
        };
        let color =
            egui::ColorImage::from_rgba_unmultiplied([pw as usize, ph as usize], &rgba);
        match &mut self.texture {
            Some(tex) => tex.set(color, egui::TextureOptions::LINEAR),
            None => {
                self.texture =
                    Some(ctx.load_texture("vorschau", color, egui::TextureOptions::LINEAR))
            }
        }
        // Der nachgeladene Ausschnitt zeigt sonst noch die vorherigen Reglerwerte.
        // Verwerfen genuegt — `maybe_load_detail` holt ihn beim naechsten Zeichnen
        // mit den neuen Werten zurueck.
        self.detail_rect = None;
        self.detail_tex = None;
        // Eine kleine Kopie der fertig entwickelten Ansicht merken: beim Sichern
        // wird sie zur Miniatur fuer Galerie und Filmstreifen — die zeigen dann
        // das ENTWICKELTE Foto statt des Rohzustands, wie in gaengigen
        // Raw-Entwicklern.
        // Nicht im Vorher-Blick und nicht im Zuschneide-Werkzeug (das zeigt
        // absichtlich das ganze Bild samt weggeschnittenem Rand).
        if !self.show_before && !self.crop_mode {
            self.thumb_aktuell = Some(verkleinere_rgba(&rgba, pw, ph, THUMB_MAX));
        }
        self.needs_render = false;
    }

    /// Baut die Arbeitskopie auf der Grafikkarte neu auf.
    ///
    /// Noetig nach jeder Aenderung an den Retuschestellen: die stecken in den
    /// Pixeln, nicht in einem Regler. Kostet den Aufbau der verkleinerten Kopie
    /// (ein Durchgang ueber das volle Bild) — nicht das teure Dekodieren, das
    /// Ergebnis liegt ja schon im Speicher.
    fn reupload(&mut self) {
        // Im Hintergrund: das Verkleinern des vollen Bildes ist der teure Teil,
        // und waehrend einer Retusche-Sitzung passiert es nach jedem Strich.
        // Auf dem UI-Thread hiess das: nach jedem Strich stand das Programm kurz.
        if self.reupload_laeuft {
            return; // der laufende Aufbau liefert gleich; danach kommt der naechste
        }
        let Some(raw) = self.raw_full.clone() else {
            self.needs_reupload = false;
            return;
        };
        let Some(path) = self.current_path() else {
            self.needs_reupload = false;
            return;
        };
        let e = self.edit_ref();
        let (heal, heal_ki, zusaetze) = (e.heal, e.heal_ki, e.zusaetze);
        let tx = self.tx.clone();
        self.needs_reupload = false;
        self.reupload_laeuft = true;
        std::thread::spawn(move || {
            let stuecke = zusatz::stuecke(&path, &zusaetze);
            let retusche = pipeline::Retusche { heal: &heal, heal_ki: &heal_ki, zusaetze: &stuecke };
            if let Ok(klein) = GpuImage::vorbereiten(&raw, Some(PREVIEW_MAX), &retusche) {
                let _ = tx.send(Msg::Arbeitskopie { path, klein });
            }
        });
    }

    /// Zaehlt nach, wie viele Fotos bearbeitet sind.
    ///
    /// Bewusst gemerkt statt bei jedem Bild neu gerechnet: fuer Fotos ohne
    /// geladene Bearbeitung sieht `is_edited` auf der Festplatte nach. Bei 500
    /// Fotos und 60 Bildern je Sekunde waeren das dreissigtausend Dateizugriffe
    /// pro Sekunde — fuer eine Zahl in der Fusszeile.
    ///
    /// Vorher stand die Zahl nur einmal beim Oeffnen da und blieb dann stehen:
    /// man schnitt ein Foto zu und die Fusszeile behauptete weiter „0 bearbeitet".
    fn zaehler_auffrischen(&mut self) {
        let n = self
            .gallery
            .iter()
            .filter(|f| self.is_edited(f))
            .count();
        self.status = tf("{} Fotos · {} bearbeitet", &[&self.gallery.len(), &n]);
    }

    /// Nach einem Sprachwechsel: die Statuszeile neu beschriften. Sie ist der
    /// einzige gemerkte Text, der sonst bis zur naechsten Meldung in der alten
    /// Sprache stehen bliebe.
    fn status_neu_beschriften(&mut self) {
        if self.gallery.is_empty() {
            self.status = grundstatus(self.gpu.as_deref());
        } else {
            self.zaehler_auffrischen();
        }
    }

    fn save_current_sidecar(&mut self) {
        if let Some(p) = self.current_path() {
            // Nie eine Begleitdatei ERFINDEN: solange das Foto nicht fertig
            // geladen ist, gibt es keine Bearbeitung im Speicher — und damit
            // nichts zu sichern. Vorher schrieb dieser Weg beim schnellen
            // Weiterblaettern start_edit() OHNE gemessene Grundhelligkeit auf
            // die Platte; das Foto lag danach dauerhaft 2–3 Blenden zu dunkel
            // (voellig abgesoffen), und keine Startwerte-Verbesserung kam je
            // bei ihm an, weil die Begleitdatei Vorrang hat.
            let Some(e) = self.edits.get(&p).cloned() else {
                self.dirty = false;
                self.dirty_at = None;
                return;
            };
            let stars = self.stars.get(&p).copied().unwrap_or(0);
            save_sidecar(&p, &e, stars);
            self.hat_sidecar.insert(p.clone());
            // Miniatur auffrischen: Galerie und Filmstreifen sollen den
            // bearbeiteten Stand zeigen. Auch in die Ablage schreiben, damit es
            // den naechsten Programmstart ueberlebt.
            if let Some((tw, th, rgba)) = self.thumb_aktuell.clone() {
                if let Some(buf) = image::RgbaImage::from_raw(tw, th, rgba.clone()) {
                    let _ = image::DynamicImage::ImageRgba8(buf)
                        .to_rgb8()
                        .save(thumb_cache_path(&p));
                }
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [tw as usize, th as usize],
                    &rgba,
                );
                if let Some(tex) = self.thumbs.get_mut(&p) {
                    tex.set(img, egui::TextureOptions::LINEAR);
                } else if let Some(ctx) = &self.egui_ctx {
                    let tex = ctx.load_texture(
                        p.to_string_lossy().to_string(),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(p, tex);
                }
            }
        }
        self.dirty = false;
        self.dirty_at = None;
        self.zaehler_auffrischen();
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.dirty_at = Some(std::time::Instant::now());
    }

    /// „Magisch entwickeln" — die lokale KI schaut sich das Foto an und
    /// entwickelt es passend zur Szene (siehe `ai`).
    ///
    /// Geschickt wird die **bereits entwickelte Vorschau**: die KI soll
    /// beurteilen, was man auf dem Bildschirm sieht.
    fn start_ai(&mut self) {
        let (Some(gpu), Some(img), Some(path)) = (&self.gpu, &self.image, self.current_path())
        else {
            return;
        };
        if self.ai_busy || !self.ki_installiert {
            return;
        }
        let edit = self.edit_ref();
        let rgba = img.render(gpu, &edit);
        let (w, h) = (img.width, img.height);
        let strength = self.ai_strength;
        let tx = self.tx.clone();
        let name = name_of(&path);
        // Zum Nachmessen eine kleine eigene Arbeitskopie im Thread — mit dem
        // erkannten Objektiv und der Retusche, wie das Foto auf dem Schirm.
        let objektiv = img.objektiv;
        let gpu2 = gpu.clone();
        let raw2 = self.raw_full.clone();
        let path_ki = path.clone();
        self.ai_busy = true;
        self.ai_undo = Some(edit.clone());
        self.ai_log.clear();
        self.ai_note = t("Lokale KI schaut sich das Foto an…").into();

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let melde = |art: LogArt, text: String, detail: String| {
                let _ = tx.send(Msg::Protokoll(LogZeile {
                    bei: start.elapsed().as_secs_f32(),
                    art,
                    text,
                    detail,
                }));
            };
            melde(
                LogArt::Info,
                tf("{} — lokale KI, Stärke {}", &[&name, &strength.label()]),
                tf("Bild: {}×{}, verkleinert auf 896 px. Nichts verlässt diesen Rechner.", &[&w, &h]),
            );
            let klein = raw2.as_ref().and_then(|raw| {
                let stuecke = zusatz::stuecke(&path_ki, &edit.zusaetze);
                let retusche = pipeline::Retusche { heal: &edit.heal, heal_ki: &edit.heal_ki, zusaetze: &stuecke };
                let mut k = GpuImage::upload_mit(&gpu2, raw, Some(900), &retusche).ok()?;
                k.objektiv = objektiv;
                Some(k)
            });
            let probe = |e: &Edit| klein.as_ref().map(|k| (k.render(&gpu2, e), k.width, k.height));
            let result = ai::develop(&rgba, w, h, &edit, strength, Some(&probe)).map_err(|e| e.to_string());
            match &result {
                Ok(r) => melde(
                    LogArt::Erfolg,
                    tf(
                        "{} · {}: {} Werte verändert",
                        &[&r.scene_label, &r.look, &r.geaendert.len()],
                    ),
                    tf(
                        "{}\n\n{}\n\nAntwort im Wortlaut:\n{}",
                        &[&r.description, &r.geaendert.join("\n"), &r.roh],
                    ),
                ),
                Err(e) => melde(LogArt::Fehler, tf("fehlgeschlagen: {}", &[e]), String::new()),
            }
            if let Ok(r) = &result {
                if !r.zurechtgerueckt.is_empty() {
                    melde(
                        LogArt::Info,
                        tf("{} Grundeinstellung(en) beibehalten", &[&r.zurechtgerueckt.len()]),
                        r.zurechtgerueckt.join("\n"),
                    );
                }
            }
            melde(
                LogArt::Info,
                tf("fertig nach {} s", &[&format!("{:.1}", start.elapsed().as_secs_f32())]),
                String::new(),
            );
            let _ = tx.send(Msg::Ai {
                path,
                result: Box::new(result),
            });
        });
    }

    fn hinzufuegen_fenster(&mut self, ctx: &egui::Context) {
        egui::Window::new(t("Hinzufügen"))
            .id(egui::Id::new("Hinzufuegen"))
            .default_width(420.0)
            .default_pos(egui::pos2(ctx.content_rect().width() - 760.0, 120.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(t("1. Den Bereich im Foto anmalen, in den etwas hinzukommen soll."));
                ui.label(t("2. Beschreiben, was dort erscheinen soll:"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.zusatz_text)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text(t("z. B. ein paar weiße Wolken am blauen Himmel")),
                );
                ui.horizontal(|ui| {
                    let laeuft = self.zusatz_seit.is_some();
                    let bereit = !self.zusatz_flecken.is_empty() && !self.zusatz_text.trim().is_empty();
                    if ui.add_enabled(!laeuft && bereit, egui::Button::new(t("Hinzufügen"))).clicked() {
                        self.zusatz_starten();
                    }
                    if ui
                        .add_enabled(!laeuft && !self.zusatz_flecken.is_empty(), egui::Button::new(t("Markierung leeren")))
                        .clicked()
                    {
                        self.zusatz_flecken.clear();
                    }
                    if let Some(seit) = self.zusatz_seit {
                        ui.add(egui::Spinner::new());
                        ui.label(tf("läuft seit {} s (~1,5 min)", &[&seit.elapsed().as_secs()]));
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                });
                if !self.zusatz_meldung.is_empty() {
                    ui.label(egui::RichText::new(&self.zusatz_meldung).size(11.0));
                }
                let zusaetze = self.edit_ref().zusaetze;
                if !zusaetze.is_empty() {
                    ui.separator();
                    let mut weg = None;
                    for (i, z) in zusaetze.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("✕").on_hover_text(t("Wieder entfernen")).clicked() {
                                weg = Some(i);
                            }
                            ui.label(&z.text);
                        });
                    }
                    if let Some(i) = weg {
                        self.push_undo();
                        self.edit().zusaetze.remove(i);
                        self.needs_reupload = true;
                        self.mark_dirty();
                    }
                }
            });
    }

    /// Setzt im markierten Bereich ein, was beschrieben ist (Hintergrund, ~1,5 min).
    fn zusatz_starten(&mut self) {
        let (Some(raw), Some(path)) = (self.raw_full.clone(), self.current_path()) else {
            return;
        };
        if self.zusatz_seit.is_some() {
            return;
        }
        let edit = self.edit_ref();
        let flecken = self.zusatz_flecken.clone();
        let text = self.zusatz_text.trim().to_string();
        // Jede Erzeugung bekommt ihren eigenen Startwert (Dateiname, Variante).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        let tx = self.tx.clone();
        self.zusatz_seit = Some(std::time::Instant::now());
        self.zusatz_meldung.clear();
        std::thread::spawn(move || {
            let ergebnis = zusatz::erzeugen(&raw, &path, &edit, flecken, &text, seed);
            let _ = tx.send(Msg::ZusatzFertig { path, ergebnis });
        });
    }

    fn panorama_fenster(&mut self, ctx: &egui::Context) {
        let mut offen = self.pano_fenster;
        let dateien: Vec<PathBuf> = self
            .gallery
            .iter()
            .zip(&self.selected)
            .filter(|(_, s)| **s)
            .map(|(p, _)| p.clone())
            .collect();
        let mut oeffnen = None;
        egui::Window::new(t("Panorama erstellen"))
            .id(egui::Id::new("Panorama"))
            .open(&mut offen)
            .default_width(460.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(tf("{} markierte Fotos, Reihenfolge egal.", &[&dateien.len()]));
                use pipeline::panorama::Projektion as P;
                egui::ComboBox::from_label(t("Projektion"))
                    .selected_text(match self.pano_projektion {
                        P::Zylinder => t("Zylinder (waagrechte Reihen)"),
                        P::Kugel => t("Kugel (sehr weit, mehrere Reihen)"),
                        P::Eben => t("Eben (wenige Bilder, gerade Linien)"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.pano_projektion, P::Zylinder, t("Zylinder (waagrechte Reihen)"));
                        ui.selectable_value(&mut self.pano_projektion, P::Kugel, t("Kugel (sehr weit, mehrere Reihen)"));
                        ui.selectable_value(&mut self.pano_projektion, P::Eben, t("Eben (wenige Bilder, gerade Linien)"));
                    });
                ui.checkbox(&mut self.pano_belichtung, t("Helligkeit der Fotos angleichen"));
                ui.checkbox(&mut self.pano_parallaxe, t("Parallaxe ausgleichen"))
                    .on_hover_text(t("Aus der Hand fotografiert, liegen nahe Gegenstände in den Nachbarfotos leicht versetzt. Der Ausgleich verschiebt die Fotos an den Nähten so, dass keine Doppelkonturen entstehen."));
                ui.add_enabled_ui(pipeline::heal_ki::verfuegbar(), |ui| {
                    ui.checkbox(&mut self.pano_fuellen, t("Leere Ränder mit der KI füllen"))
                        .on_hover_text(t("Statt abzuschneiden ergänzt die lokale KI die ausgefransten Ränder aus dem Bildinhalt."));
                });
                ui.add_enabled_ui(!self.pano_fuellen, |ui| {
                    ui.checkbox(&mut self.pano_zuschneiden, t("Automatisch auf den vollen Bildbereich zuschneiden"));
                });
                ui.checkbox(&mut self.pano_volle_aufloesung, t("Volle Auflösung (langsamer, viel Arbeitsspeicher)"));
                ui.label(
                    egui::RichText::new(t("Das Panorama wird als Rohdatei (DNG) neben den Fotos gespeichert — danach wie jedes Foto entwickeln, den Zuschnitt mit R anpassen."))
                        .size(11.0)
                        .weak(),
                );
                ui.separator();
                if let Some((anteil, text)) = &self.pano_laeuft {
                    ui.add(egui::ProgressBar::new(*anteil).show_percentage());
                    ui.label(text);
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                } else if ui
                    .add_enabled(dateien.len() >= 2, egui::Button::new(t("Panorama erstellen")))
                    .clicked()
                {
                    let auftrag = panorama::Auftrag {
                        dateien: dateien.clone(),
                        projektion: self.pano_projektion,
                        belichtung_angleichen: self.pano_belichtung,
                        parallaxe_ausgleichen: self.pano_parallaxe,
                        raender_fuellen: self.pano_fuellen,
                        zuschneiden: self.pano_zuschneiden,
                        max_seite: if self.pano_volle_aufloesung { 0 } else { 8000 },
                    };
                    self.pano_laeuft = Some((0.0, t("Startet…").into()));
                    self.pano_ergebnis = None;
                    self.pano_meldung.clear();
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let tx2 = tx.clone();
                        let r = panorama::erstellen(&auftrag, &|a, text| {
                            let _ = tx2.send(Msg::PanoFortschritt(a, text));
                        });
                        let _ = tx.send(Msg::PanoFertig(r));
                    });
                }
                if !self.pano_meldung.is_empty() {
                    ui.label(egui::RichText::new(&self.pano_meldung).size(11.0));
                }
                if let Some(p) = &self.pano_ergebnis {
                    if ui.button(tf("{} öffnen", &[&name_of(p)])).clicked() {
                        oeffnen = Some(p.clone());
                    }
                }
            });
        self.pano_fenster = offen;
        if let Some(p) = oeffnen {
            if let Some(ordner) = p.parent() {
                self.open_paths(vec![ordner.to_path_buf()]);
                if let Some(i) = self.gallery.iter().position(|g| *g == p) {
                    self.gallery_view = false;
                    self.select(i);
                }
            }
            self.pano_fenster = false;
        }
    }

    /// Statuszeile: Update-Knopf, nur wenn es wirklich etwas Neues gibt.
    fn update_knopf(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.update_geprueft {
            self.update_geprueft = true;
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                if let Ok(Some(neu)) = updater::pruefen() {
                    let _ = tx.send(Msg::UpdateGefunden(neu));
                }
            });
        }
        if let Some((b, g)) = self.update_laedt {
            ui.add(egui::ProgressBar::new(b as f32 / g.max(1) as f32).desired_width(90.0).show_percentage());
            ui.label(egui::RichText::new(t("Update lädt…")).size(11.0));
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if !self.update_meldung.is_empty() {
            ui.label(egui::RichText::new(&self.update_meldung).size(11.0).color(farbe::AKZENT));
        }
        let Some(neu) = self.update_neu.clone() else { return };
        if ui
            .button(egui::RichText::new(tf("Update {} installieren", &[&neu.version])).size(11.0).color(farbe::AKZENT))
            .on_hover_text(tf(
                "Lädt Version {} ({} MB), prüft die Prüfsumme und startet Fotowache neu. Deine Bearbeitungen bleiben erhalten.",
                &[&neu.version, &(neu.groesse / 1_000_000)],
            ))
            .clicked()
        {
            self.update_laedt = Some((0, neu.groesse));
            self.update_meldung.clear();
            let tx = self.tx.clone();
            let abbruch = self.update_abbruch.clone();
            std::thread::spawn(move || {
                let tx2 = tx.clone();
                let r = updater::installieren(&neu, &|b, g| {
                    let _ = tx2.send(Msg::UpdateFortschritt(b, g));
                }, &abbruch);
                let _ = tx.send(Msg::UpdateFertig(r));
            });
        }
    }

    /// Installiert den Bildgenerator im Hintergrund.
    fn gen_installieren(&mut self) {
        if self.gen_laedt.is_some() {
            return;
        }
        self.gen_abbruch.store(false, std::sync::atomic::Ordering::Relaxed);
        self.gen_laedt = Some((t("KI wird vorbereitet…").into(), 0.0));
        let tx = self.tx.clone();
        let abbruch = self.gen_abbruch.clone();
        std::thread::spawn(move || {
            let gesamt = generator::paketgroesse() as f64;
            let fertig_vorher = std::cell::Cell::new(0u64);
            let letzte = std::cell::Cell::new(0usize);
            let melde = |f: ki_lokal::Fortschritt| {
                let (text, anteil) = match f {
                    ki_lokal::Fortschritt::Laedt { datei, von, bytes, gesamt: g } => {
                        if datei != letzte.get() {
                            if letzte.get() > 0 {
                                fertig_vorher.set(fertig_vorher.get() + generator::dateien()[letzte.get() - 1].groesse);
                            }
                            letzte.set(datei);
                        }
                        let ist = (fertig_vorher.get() + bytes.min(g)) as f64;
                        (
                            tf(
                                "KI lädt: Datei {} von {} · {} von {} GB",
                                &[&datei, &von, &format!("{:.1}", ist / 1e9), &format!("{:.1}", gesamt / 1e9)],
                            ),
                            (ist / gesamt) as f32,
                        )
                    }
                    ki_lokal::Fortschritt::Entpackt => (t("KI wird eingerichtet…").to_string(), 1.0),
                    ki_lokal::Fortschritt::Fertig => (t("Bildgenerator ist installiert.").to_string(), 1.0),
                };
                let _ = tx.send(Msg::GenFortschritt(text, anteil));
            };
            let _ = tx.send(Msg::GenInstalliert(generator::installieren(&melde, &abbruch)));
        });
    }

    /// Erzeugt ein Bild im Hintergrund (rund eine Minute).
    fn gen_starten(&mut self) {
        if self.gen_seit.is_some() || self.gen_text.trim().is_empty() {
            return;
        }
        let dir = ordner::ablage().join("generator");
        let _ = std::fs::create_dir_all(&dir);
        let ziel = dir.join(format!("bild-{}-{}.png", self.gen_seed, std::process::id()));
        let (text, format, seed) = (self.gen_text.clone(), self.gen_format, self.gen_seed);
        let tx = self.tx.clone();
        self.gen_seit = Some(std::time::Instant::now());
        self.gen_meldung.clear();
        std::thread::spawn(move || {
            let r = generator::erzeugen(&text, format, seed, &ziel).map(|()| ziel);
            let _ = tx.send(Msg::GenFertig(r));
        });
    }

    fn generator_fenster(&mut self, ctx: &egui::Context) {
        let mut offen = self.gen_fenster;
        egui::Window::new(t("Bild erzeugen"))
            .id(egui::Id::new("Bildgenerator"))
            .open(&mut offen)
            .default_width(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                if let Err(grund) = generator::verfuegbar() {
                    ui.label(grund);
                    return;
                }
                if let Some((text, anteil)) = self.gen_laedt.clone() {
                    ui.add(egui::ProgressBar::new(anteil).show_percentage());
                    ui.label(text);
                    if ui.button(t("abbrechen")).clicked() {
                        self.gen_abbruch.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    return;
                }
                if !self.gen_installiert {
                    ui.label(t(
                        "Erzeugt Bilder aus einer Beschreibung — komplett auf diesem Rechner, mit einem \
                         offenen Bildmodell. Dafür wird einmalig ein eigenes Paket geladen.",
                    ));
                    if ui
                        .button(tf("Bildgenerator installieren ({} GB)", &[&format!("{:.1}", generator::paketgroesse() as f64 / 1e9)]))
                        .clicked()
                    {
                        self.gen_installieren();
                    }
                    if !self.gen_meldung.is_empty() {
                        ui.colored_label(farbe::AKZENT, &self.gen_meldung);
                    }
                    return;
                }
                ui.label(t("Was soll zu sehen sein? (Englisch klappt am besten)"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.gen_text)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("a red bicycle leaning against an old stone wall, morning light, photograph"),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("gen_format")
                        .selected_text(self.gen_format.label())
                        .show_ui(ui, |ui| {
                            for f in [generator::Format::Quer, generator::Format::Hoch, generator::Format::Quadrat] {
                                ui.selectable_value(&mut self.gen_format, f, f.label());
                            }
                        });
                    let laeuft = self.gen_seit.is_some();
                    if ui
                        .add_enabled(!laeuft && !self.gen_text.trim().is_empty(), egui::Button::new(t("Erzeugen")))
                        .clicked()
                    {
                        self.gen_starten();
                    }
                    if ui
                        .add_enabled(!laeuft && self.gen_ergebnis.is_some(), egui::Button::new(t("Nochmal (andere Variante)")))
                        .clicked()
                    {
                        self.gen_seed = self.gen_seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 1;
                        self.gen_starten();
                    }
                    if let Some(seit) = self.gen_seit {
                        ui.add(egui::Spinner::new());
                        ui.label(tf("läuft seit {} s (~1 min)", &[&seit.elapsed().as_secs()]));
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                });
                if !self.gen_meldung.is_empty() {
                    ui.colored_label(farbe::AKZENT, &self.gen_meldung);
                }
                if let Some((pfad, tex)) = &self.gen_ergebnis {
                    let groesse = tex.size_vec2();
                    let breite = ui.available_width().min(groesse.x);
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(breite, breite * groesse.y / groesse.x)));
                    if ui.button(t("Speichern unter…")).clicked() {
                        if let Some(ziel) = rfd::FileDialog::new().set_file_name("bild.png").add_filter("PNG", &["png"]).save_file() {
                            self.gen_meldung = match std::fs::copy(pfad, &ziel) {
                                Ok(_) => tf("Gespeichert: {}", &[&ziel.display()]),
                                Err(e) => e.to_string(),
                            };
                        }
                    }
                }
            });
        self.gen_fenster = offen;
    }

    /// Installiert die lokale KI im Hintergrund (ein Klick).
    fn ki_installieren(&mut self) {
        if self.ki_laedt.is_some() {
            return;
        }
        self.ki_abbruch.store(false, std::sync::atomic::Ordering::Relaxed);
        self.ki_laedt = Some((t("KI wird vorbereitet…").into(), 0.0));
        let tx = self.tx.clone();
        let abbruch = self.ki_abbruch.clone();
        std::thread::spawn(move || {
            let gesamt = ki_lokal::paketgroesse() as f64;
            let fertig_vorher = std::cell::Cell::new(0u64);
            let letzte_datei = std::cell::Cell::new(0usize);
            let melde = |f: ki_lokal::Fortschritt| {
                let (text, anteil) = match f {
                    ki_lokal::Fortschritt::Laedt { datei, von, bytes, gesamt: g } => {
                        if datei != letzte_datei.get() {
                            // Vorige Datei ist fertig: ihre Groesse zaehlt voll.
                            if letzte_datei.get() > 0 {
                                let d = ki_lokal::dateien()[letzte_datei.get() - 1];
                                fertig_vorher.set(fertig_vorher.get() + d.groesse);
                            }
                            letzte_datei.set(datei);
                        }
                        let ist = (fertig_vorher.get() + bytes.min(g)) as f64;
                        (
                            tf(
                                "KI lädt: Datei {} von {} · {} von {} GB",
                                &[&datei, &von, &format!("{:.1}", ist / 1e9), &format!("{:.1}", gesamt / 1e9)],
                            ),
                            (ist / gesamt) as f32,
                        )
                    }
                    ki_lokal::Fortschritt::Entpackt => (t("KI wird eingerichtet…").to_string(), 1.0),
                    ki_lokal::Fortschritt::Fertig => (t("Lokale KI ist installiert.").to_string(), 1.0),
                };
                let _ = tx.send(Msg::KiFortschritt(text, anteil));
            };
            let ergebnis = ki_lokal::installieren(&melde, &abbruch);
            let _ = tx.send(Msg::KiInstalliert(ergebnis));
        });
    }

    fn set_stars(&mut self, n: u8) {
        if let Some(p) = self.current_path() {
            if n == 0 {
                self.stars.remove(&p);
            } else {
                self.stars.insert(p, n);
            }
            self.mark_dirty();
        }
    }

    fn start_batch(&mut self) {
        let Some(gpu) = self.gpu.clone() else { return };
        let files: Vec<PathBuf> = if self.selected.iter().any(|s| *s) {
            self.gallery
                .iter()
                .zip(&self.selected)
                .filter(|(_, s)| **s)
                .map(|(p, _)| p.clone())
                .collect()
        } else {
            self.gallery.clone()
        };
        if files.is_empty() {
            return;
        }
        // Regelquelle pro Foto festlegen — das Original wird nie ueberschrieben:
        //  Some(e) → diese Bearbeitung nehmen (schon geladen / von Hand gedreht,
        //            bzw. im „auf alle anwenden"-Modus das aktuell offene Bild).
        //  None    → Vollautomatik: der Worker misst die Startwerte pro Foto selbst
        //            (Grundhelligkeit, Lichter/Schatten, Schwarz, Rauschen) — exakt
        //            dieselbe Rechnung wie der Kommandozeilen-Stapel.
        let auto = self.batch_auto;
        let fallback = self.edit_ref();
        let edits: Vec<Option<Edit>> = files
            .iter()
            .map(|f| {
                if let Some(e) = self.edits.get(f) {
                    Some(e.clone())
                } else if auto {
                    None
                } else {
                    Some(fallback.clone())
                }
            })
            .collect();
        let settings = self.batch.clone();
        let tx = self.tx.clone();
        let use_ai = self.batch_ai && self.ki_installiert;
        let strength = self.ai_strength;
        self.show_batch = false;
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Batch(export::BatchMsg::Started { total: files.len() }));
            let mut ok = 0usize;
            let mut failed = 0usize;
            for (file, edit) in files.iter().zip(edits.iter()) {
                let result = if use_ai {
                    // Die KI-Entwicklung zieht die Startwerte ohnehin pro Foto nach;
                    // ohne eigene Bearbeitung ist die neutrale Basis der Startpunkt.
                    let base = edit.clone().unwrap_or_else(presets::start_edit);
                    develop_with_ai(&gpu, file, &base, &settings, strength)
                } else if let Some(edit) = edit {
                    export::export_one(&gpu, file, edit, &settings)
                } else {
                    // Vollautomatik pro Foto — exakt der Kommandozeilen-Pfad:
                    // Begleitdatei hat Vorrang, sonst neutrale Regler; danach die
                    // gemessenen Startwerte nachziehen.
                    rawcore::decode(file, &rawcore::DecodeOptions::default())
                        .map_err(export::ExportError::from)
                        .and_then(|raw| {
                            let grund = grundstand(file, &raw);
                            let mut e = load_sidecar(file).map(|(e, _)| e).unwrap_or_else(|| grund.clone());
                            startwerte_nachziehen(&mut e, &grund);
                            export::save_developed(&gpu, &raw, file, &e, &settings)
                        })
                };
                match result {
                    Ok(target) => {
                        ok += 1;
                        let _ = tx.send(Msg::Batch(export::BatchMsg::Done {
                            file: file.clone(),
                            target,
                        }));
                    }
                    Err(e) => {
                        failed += 1;
                        let _ = tx.send(Msg::Batch(export::BatchMsg::Failed {
                            file: file.clone(),
                            error: e.to_string(),
                        }));
                    }
                }
            }
            let _ = tx.send(Msg::Batch(export::BatchMsg::Finished { ok, failed }));
        });
    }
}

/// Ablage fuer Vorschaubilder. Ohne sie muesste beim Zurueckkommen in ein
/// Projekt jedes RAW neu dekodiert werden — bei 300 Fotos Minuten statt Sekunden.
/// Bildpunkt vom **angezeigten** ins **gespeicherte** Bild rechnen.
///
/// Noetig, weil die Retusche vor der Drehung sitzt (sie steckt in den Pixeln) und
/// man auf das gedrehte Bild klickt. Ohne diese Umrechnung landet der Fleck bei
/// jedem Hochformat an der falschen Stelle — und zwar erst beim Export sichtbar.
fn heal_punkt_zurueckdrehen(mut x: f32, mut y: f32, e: &Edit) -> (f32, f32) {
    if e.flip_h {
        x = 1.0 - x;
    }
    if e.flip_v {
        y = 1.0 - y;
    }
    match e.rotate.rem_euclid(360) {
        90 => (y, 1.0 - x),
        180 => (1.0 - x, 1.0 - y),
        270 => (1.0 - y, x),
        _ => (x, y),
    }
}

/// Die Gegenrichtung — fuer die Markierungen, die im Bild angezeigt werden.
fn heal_punkt_drehen(x: f32, y: f32, e: &Edit) -> (f32, f32) {
    let (mut tx, mut ty) = match e.rotate.rem_euclid(360) {
        90 => (1.0 - y, x),
        180 => (1.0 - x, 1.0 - y),
        270 => (y, 1.0 - x),
        _ => (x, y),
    };
    if e.flip_h {
        tx = 1.0 - tx;
    }
    if e.flip_v {
        ty = 1.0 - ty;
    }
    (tx, ty)
}

/// Anteil im **angezeigten** Bild → Anteil im **Quellbild**.
///
/// Die Anzeige ist gedreht und zugeschnitten, die Quelle nicht. Wer das verwechselt,
/// laedt beim Zoomen den falschen Bildausschnitt nach — sichtbar erst, wenn ein
/// Foto gedreht ODER beschnitten ist, also genau dann, wenn man es nicht mehr sucht.
fn anzeige_zu_quelle(dx: f32, dy: f32, e: &Edit) -> (f32, f32) {
    let [cx, cy, cw, ch] = e.crop.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    heal_punkt_zurueckdrehen(cx + dx * cw, cy + dy * ch, e)
}

/// Die Gegenrichtung: Quelle → Anzeige.
fn quelle_zu_anzeige(sx: f32, sy: f32, e: &Edit) -> (f32, f32) {
    let (ox, oy) = heal_punkt_drehen(sx, sy, e);
    let [cx, cy, cw, ch] = e.crop.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    ((ox - cx) / cw.max(1e-6), (oy - cy) / ch.max(1e-6))
}

/// Umschliessendes Rechteck von vier durch eine Abbildung geschickten Ecken.
/// Bei Vierteldrehungen bleibt aus einem Rechteck wieder eins — die Ecken
/// wechseln nur ihre Rollen, deshalb reicht Minimum und Maximum.
fn ecken_bbox(
    a: (f32, f32),
    b: (f32, f32),
    e: &Edit,
    abb: fn(f32, f32, &Edit) -> (f32, f32),
) -> (f32, f32, f32, f32) {
    let p = [
        abb(a.0, a.1, e),
        abb(b.0, a.1, e),
        abb(a.0, b.1, e),
        abb(b.0, b.1, e),
    ];
    let xs = p.iter().map(|q| q.0);
    let ys = p.iter().map(|q| q.1);
    (
        xs.clone().fold(f32::MAX, f32::min),
        ys.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
        ys.fold(f32::MIN, f32::max),
    )
}

/// Lagesensor der Kamera → Drehung in Grad.
///
/// LibRaw gibt die Lage in derselben Zaehlweise weiter, die auch EXIF benutzt:
/// 0 = normal, 3 = auf dem Kopf, 5 = nach links gekippt, 6 = nach rechts.
/// Ohne diese Umsetzung liegt jedes Hochformat beim Oeffnen quer.
fn drehung_aus_kamera(flip: i32) -> i32 {
    match flip {
        3 => 180,
        5 => 270,
        6 => 90,
        _ => 0,
    }
}

fn thumb_cache_path(raw: &Path) -> PathBuf {
    let mut key = raw.to_string_lossy().to_string();
    // Versionsnummer im Schluessel: aendert sich die Miniatur-Entwicklung (wie
    // beim Helligkeits-Fix), waeren sonst alle ALTEN, dunklen Miniaturen weiter
    // aus der Ablage gekommen — der Fehler saehe aus wie nicht behoben.
    // v4: Kameralage-Doppeldrehung behoben (user_flip=0) — alte Miniaturen
    // von Hochformat-Fotos lagen quer im Cache.
    // v5: Miniaturen kommen jetzt aus dem eingebetteten Kamera-JPEG (Millisekunden
    // statt Sekunden) — anderer Look als die alten, voll entwickelten. Alte
    // Cache-Einträge sonst gemischt mit neuen; deshalb Version hoch.
    key.push_str("|miniatur-v5");
    if let Ok(md) = std::fs::metadata(raw) {
        if let Ok(t) = md.modified() {
            if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                // Zeitstempel mit in den Schluessel: wird das RAW ersetzt,
                // gilt die alte Vorschau nicht mehr.
                key.push_str(&format!("|{}", d.as_secs()));
            }
        }
    }
    // Schlichter, stabiler Streuwert (FNV-1a) — kein Bedarf fuer Kryptografie.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    // Plattformgerechter Ort, siehe `ordner` (dort steht auch, wo er derselbe
    // blieb wie bisher).
    let dir = ordner::ablage().join("thumbs");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{h:016x}.jpg"))
}

/// Pfad der Begleitdatei. Sie liegt neben dem RAW und enthaelt Regler + Sterne.
/// Das Original wird nie angefasst — RAW-Dateien bleiben unberuehrt, wie es sich
/// gehoert; alles Bearbeitete ist jederzeit widerrufbar.
fn sidecar_path(raw: &Path) -> PathBuf {
    neben(raw, ".fotowache.json")
}

/// Begleitdatei unter dem Namen aus der Zeit vor „Fotowache" (bis 2.0 hiess das
/// Programm FotoRaw). Sie wird weiter gelesen und beim naechsten Speichern durch
/// die neue ersetzt — so geht keine Bearbeitung verloren.
fn sidecar_path_alt(raw: &Path) -> PathBuf {
    neben(raw, ".fotoraw.json")
}

fn neben(raw: &Path, endung: &str) -> PathBuf {
    let mut s = raw.as_os_str().to_os_string();
    s.push(endung);
    PathBuf::from(s)
}

/// Die Begleitdatei, die es gibt: die neue, sonst die alte.
fn vorhandene_begleitdatei(raw: &Path) -> Option<PathBuf> {
    [sidecar_path(raw), sidecar_path_alt(raw)].into_iter().find(|p| p.is_file())
}

fn load_sidecar(raw: &Path) -> Option<(Edit, u8)> {
    let text = std::fs::read_to_string(vorhandene_begleitdatei(raw)?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let edit = v
        .get("edit")
        .and_then(|e| serde_json::from_value::<Edit>(e.clone()).ok())?;
    let stars = v.get("stars").and_then(|s| s.as_u64()).unwrap_or(0) as u8;
    Some((edit, stars))
}

fn save_sidecar(raw: &Path, edit: &Edit, stars: u8) {
    let v = serde_json::json!({
        "programm": "Fotowache",
        "edit": edit,
        "stars": stars,
    });
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        // Erst wenn die neue Datei sicher geschrieben ist, faellt die alte weg.
        if std::fs::write(sidecar_path(raw), text).is_ok() {
            let alt = sidecar_path_alt(raw);
            if alt.is_file() {
                let _ = std::fs::remove_file(alt);
            }
        }
    }
}

/// Ein Foto im Stapel von der KI entwickeln lassen und speichern.
///
/// Dekodiert bewusst nur **einmal**: aus demselben RAW entsteht erst die kleine
/// Ansicht fuer die KI und danach der Export in voller Aufloesung.
/// Faellt die KI aus, wird trotzdem exportiert — mit den mitgegebenen Reglern.
/// Ein Stapel darf nicht komplett scheitern, nur weil der KI-Server kurz nicht
/// antwortet.
/// Startwerte fuer ein Foto: neutrale Regler **plus die gemessene Grundhelligkeit**.
///
/// An einer Stelle, weil es sie vorher an dreien gab — und zwei davon (die
/// Kommandozeile und der KI-Stapel) haben die Grundhelligkeit schlicht vergessen.
/// Ergebnis: die Dateien kamen viel zu dunkel heraus, und die KI bekam ein
/// duesteres Bild zu sehen, das sie dann mit Belichtung „reparierte".
fn startwerte(raw: &rawcore::RawImage) -> Edit {
    let mut e = presets::start_edit();
    // Kameralage gehoert zur Grundausstattung: seit LibRaw nicht mehr selbst
    // dreht (user_flip = 0), muss JEDER Weg — Editor, Stapel, KI-Stapel — die
    // Drehung aus den Startwerten bekommen.
    e.rotate = drehung_aus_kamera(raw.meta.flip);
    // Die Automatik — aus echten Bearbeitungen der Referenz-Entwicklung gelernt
    // (siehe pipeline::auto). Die Werte stehen sichtbar in den Reglern, wie nach
    // „Auto" in gaengigen Raw-Entwicklern; die Belichtung als Grundhelligkeit
    // (der Regler zeigt die Summe in Blenden).
    let v = pipeline::auto::vorschlag(raw);
    e.base_ev = v.belichtung_ev;
    e.contrast = v.kontrast;
    e.highlights = v.lichter;
    e.shadows = v.tiefen;
    e.whites = v.weiss;
    e.blacks = v.schwarz;
    e.vibrance = pipeline::xmp::dynamik_aus_xmp(v.dynamik);
    e.saturation = v.saettigung;
    // Rauschminderung ISO-abhaengig, nach EFFEKTIVER ISO: die Kamera-
    // Grundbelichtung (RAF-Kameras belichten das RAW 0,7–1,7 Blenden knapper)
    // und die Automatik sind ein digitaler Push — ISO 800 rauscht dann wie
    // ISO 1600+.
    let push = (v.belichtung_ev - raw.meta.belichtungsversatz).max(0.0);
    let eff_iso = raw.meta.iso * push.exp2();
    if eff_iso > 400.0 {
        let stufen = (eff_iso / 400.0).log2();
        e.color_noise = (22.0 + stufen * 6.5).clamp(25.0, 48.0);
    }
    if eff_iso > 800.0 {
        let stufen = (eff_iso / 800.0).log2();
        e.noise = (14.0 + stufen * 9.0).clamp(14.0, 45.0);
    }
    e
}

/// Grundstand eines Fotos ohne Fotowache-Begleitdatei: liegt die XMP-Begleitdatei
/// eines anderen Programms daneben, gelten deren XMP-Einstellungen (Ton, Farbe,
/// Weissabgleich, Details, Zuschnitt) — sonst die Automatik. Auch im Stapel: ein
/// Ordner aus einer anderen Anwendung wird so entwickelt, wie er dort
/// eingestellt ist.
fn grundstand(path: &Path, raw: &rawcore::RawImage) -> Edit {
    let auto = startwerte(raw);
    if !xmp_uebernehmen() {
        return auto;
    }
    let Some(xmp) = pipeline::xmp::lesen(path) else {
        return auto;
    };
    let profil = pipeline::dcp::fuer_meta(&raw.meta);
    let fs = profil.farbstand(&raw.meta.wb_mul, &raw.meta.wb_aufnahme, None);
    let mut e = xmp.als_edit(&auto, (fs.kelvin_aufnahme as f64, fs.toenung_aufnahme as f64));
    // Die Belichtung der XMP-Einstellungen ist die Grundhelligkeit (der Regler
    // zeigt die Summe).
    e.base_ev = e.exposure / 50.0;
    e.exposure = 0.0;
    if let Some(z) = xmp.zuschnitt() {
        e.crop = Some(z);
    }
    // Rauschminderung: der Wert der XMP-Einstellungen, aber nie unter der
    // ISO-Grundausstattung.
    e.noise = e.noise.max(auto.noise);
    e.color_noise = e.color_noise.max(auto.color_noise);
    e.bearbeitet = false;
    e
}

/// XMP-Einstellungen anderer Programme uebernehmen? Standard ja; abschaltbar mit
/// FOTOWACHE_OHNE_XMP=1 (oder in den Einstellungen).
fn xmp_uebernehmen() -> bool {
    std::env::var_os("FOTOWACHE_OHNE_XMP").is_none()
}

/// Hat jemand an einer Bearbeitung aus Prozess 1 wirklich etwas verstellt?
/// (Die damalige Automatik setzte Grundhelligkeit, Rauschen, Schwarzpunkt,
/// Lichter/Tiefen/Weiss — das zaehlt nicht.)
fn alt_bearbeitet(e: &Edit) -> bool {
    let mut start = presets::start_edit_v1();
    start.base_ev = e.base_ev;
    start.rotate = e.rotate;
    start.color_noise = e.color_noise;
    start.noise = e.noise;
    start.blackpoint = e.blackpoint;
    start.highlights = e.highlights;
    start.shadows = e.shadows;
    start.whites = e.whites;
    start.prozess = e.prozess;
    start.bearbeitet = e.bearbeitet;
    start.profil = e.profil.clone();
    e != &start
}

/// Vor der Umstellung einer alten Begleitdatei eine Sicherung anlegen
/// (`<name>.fotowache.v1.json`) — nie etwas ueberschreiben, ohne es aufzuheben.
/// Eine Sicherung aus der Zeit vor dem neuen Namen (`.fotoraw.v1.json`) zaehlt mit.
fn sichere_v1(raw: &Path) {
    let Some(alt) = vorhandene_begleitdatei(raw) else { return };
    let ziel = neben(raw, ".fotowache.v1.json");
    if !ziel.exists() && !neben(raw, ".fotoraw.v1.json").exists() {
        let _ = std::fs::copy(&alt, &ziel);
    }
}

/// Fehlende Grundausstattung nachziehen. `base_ev == 0` ist das Kennzeichen
/// einer Bearbeitung, die OHNE gemessene Startwerte entstand — der Klassiker
/// war das schnelle Weiterblaettern: der Vorgaenger wurde gesichert, bevor
/// sein Dekodieren fertig war (start_edit() statt startwerte()). Solche Fotos
/// lagen dauerhaft 2–3 Blenden zu dunkel, Hochformate zusaetzlich quer, und
/// weil die Begleitdatei Vorrang hat, kam keine Verbesserung je bei ihnen an.
/// Nutzerwerte bleiben unangetastet: es wird nur ergaenzt (max), nie gesenkt.
/// Gibt zurueck, ob etwas nachgezogen wurde.
fn startwerte_nachziehen(edit: &mut Edit, frisch: &Edit) -> bool {
    // Prozess 1 → 2: Ton- und Farbwerte des alten Rechenwegs bedeuten im
    // Rechenweg der Referenz-Entwicklung etwas anderes (Grundhelligkeit,
    // Dynamik 44 …). Neu aufsetzen auf die Grundausstattung,
    // Aufnahme-Entscheidungen behalten.
    // Die alte Datei sichert der Aufrufer vorher (`sichere_v1`).
    if edit.prozess < pipeline::params::PROZESS {
        let alt = edit.clone();
        let mut neu = frisch.clone();
        neu.crop = alt.crop;
        if alt.rotate != 0 {
            neu.rotate = alt.rotate;
        }
        neu.flip_h = alt.flip_h;
        neu.flip_v = alt.flip_v;
        neu.straighten = alt.straighten;
        neu.heal = alt.heal.clone();
        neu.heal_ki = alt.heal_ki.clone();
        neu.zusaetze = alt.zusaetze.clone();
        neu.lens_auto = alt.lens_auto;
        neu.lens_profil = alt.lens_profil.clone();
        neu.bw = alt.bw;
        neu.bearbeitet = alt_bearbeitet(&alt);
        neu.prozess = pipeline::params::PROZESS;
        *edit = neu;
        return true;
    }
    // Unbearbeitet = reine Grundausstattung (Automatik bzw. XMP-Einstellungen):
    // sie folgt IMMER dem aktuellen Grundstand. So kommen Verbesserungen an
    // Automatik und XMP-Uebernahme bei jedem Foto an — eine gespeicherte
    // Automatik darf nie eine bessere ueberstimmen (die alte Falle: Verbesserungen
    // kamen nie an). Bearbeitetes bleibt unangetastet.
    if !edit.bearbeitet && *edit != *frisch {
        *edit = frisch.clone();
        return true;
    }
    false
}

fn develop_with_ai(
    gpu: &Gpu,
    file: &Path,
    base: &Edit,
    settings: &export::ExportSettings,
    strength: ai::Strength,
) -> Result<PathBuf, export::ExportError> {
    let raw = rawcore::decode(file, &rawcore::DecodeOptions::default())?;
    let grund = grundstand(file, &raw);
    // Ohne eigene Bearbeitung ist der Grundstand (Automatik bzw. XMP-
    // Einstellungen) der Ausgangspunkt der KI.
    let mut edit = if *base == presets::start_edit() { grund.clone() } else { base.clone() };
    // Frisches Foto ohne Begleitdatei (oder eine vergiftete mit base_ev 0):
    // die kompletten Startwerte nachziehen — dieselbe EINE Funktion wie im
    // Editor, halbe Nachbauten dieser Stelle haben schon zweimal etwas
    // vergessen (Grundhelligkeit, ISO-Rauschminderung, Kameralage).
    startwerte_nachziehen(&mut edit, &grund);
    if let Ok(mut small) = GpuImage::upload(gpu, &raw, Some(1400)) {
        // Auch im Stapel: die KI muss das KORRIGIERTE Bild beurteilen —
        // dasselbe, das nachher exportiert wird.
        small.objektiv = pipeline::objektiv::erkennen(&raw);
        let rgba = small.render(gpu, &edit);
        let probe = |e: &Edit| Some((small.render(gpu, e), small.width, small.height));
        match ai::develop(&rgba, small.width, small.height, &edit, strength, Some(&probe)) {
            Ok(r) => {
                println!("{}", tf("  KI: {} · {} — {}", &[&r.scene_label, &r.look, &r.description]));
                edit = r.edit;
            }
            Err(e) => eprintln!("{}", tf("KI übersprungen für {}: {}", &[&name_of(file), &e])),
        }
    }
    // Die KI-Regler mitsichern, damit sie in der Oberflaeche sichtbar werden.
    let stars = load_sidecar(file).map(|(_, s)| s).unwrap_or(0);
    save_sidecar(file, &edit, stars);
    export::save_developed(gpu, &raw, file, &edit, settings)
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

/// Statuszeile ohne geoeffnete Fotos: bereit, mit Grafikkarte und Version.
fn grundstatus(gpu: Option<&Gpu>) -> String {
    match gpu {
        Some(g) => tf("Bereit · {} · v{}", &[&g.backend, &env!("CARGO_PKG_VERSION")]),
        None => t("Keine Grafikkarte gefunden").to_string(),
    }
}

/// Schnelle Miniatur direkt aus den linearen Rohdaten — ohne GPU, damit der
/// Filmstreifen auch waehrend einer laufenden Stapelverarbeitung fuellt.
///
/// ⚠️ Sie muss denselben GRUNDEINDRUCK treffen wie die grosse Vorschau, sonst
/// zeigt die Galerie lauter „kaputte" Fotos: Ohne die gemessene Grundhelligkeit
/// und die Grundgradation waren alle Miniaturen viel zu dunkel — dieselbe
/// Fehlerklasse wie damals bei Kommandozeile und KI-Stapel („eine Stelle hat die
/// Grundhelligkeit schlicht vergessen"), nur an der vierten Stelle.
fn quick_thumb(raw: &rawcore::RawImage, max: u32) -> (u32, u32, Vec<u8>) {
    // Dieselbe Grundausstattung wie beim Oeffnen: gemessene Grundhelligkeit,
    // Kameraprofil, Grundkurve — sonst zeigt die Galerie ein anderes Foto als
    // das, das sich hinter der Miniatur verbirgt.
    let (w, h, out) = pipeline::schnellbild(raw, max, pipeline::auto_exposure(raw));
    // Kameralage: eine Hochformat-Aufnahme soll auch in der Galerie hochkant
    // stehen. Die grosse Ansicht dreht ueber `rotate` — Miniaturen drehen hier.
    let dreh = drehung_aus_kamera(raw.meta.flip);
    if dreh != 0 {
        let (gedreht, nw, nh) = pipeline::ausrichten(&out, w, h, 4, dreh, false, false);
        return (nw, nh, gedreht);
    }
    (w, h, out)
}

/// Fertige RGBA-Pixel auf Miniaturgroesse bringen — Mittelung ueber Bloecke,
/// damit nichts flimmert oder Treppen wirft.
fn verkleinere_rgba(rgba: &[u8], w: u32, h: u32, max: u32) -> (u32, u32, Vec<u8>) {
    let step = (w.max(h) as f32 / max as f32).ceil().max(1.0) as u32;
    let dw = (w / step).max(1);
    let dh = (h / step).max(1);
    let mut out = vec![255u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0f32; 3];
            let mut n = 0f32;
            for sy in (y * step)..((y + 1) * step).min(h) {
                for sx in (x * step)..((x + 1) * step).min(w) {
                    let i = ((sy as usize) * (w as usize) + sx as usize) * 4;
                    acc[0] += rgba[i] as f32;
                    acc[1] += rgba[i + 1] as f32;
                    acc[2] += rgba[i + 2] as f32;
                    n += 1.0;
                }
            }
            let o = ((y * dw + x) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (acc[c] / n.max(1.0)) as u8;
            }
        }
    }
    (dw, dh, out)
}

/// Misst ausgefressene (ein Kanal am Anschlag) und abgesoffene (alle Kanaele
/// am Boden) Anteile; mit `markieren` faerbt es sie wie die J-Ansicht gaengiger
/// Raw-Entwickler:
/// Lichter rot, Tiefen blau.
fn clipping_markieren(rgba: &mut [u8], markieren: bool) -> (f32, f32) {
    let (mut hell, mut dunkel, mut n) = (0usize, 0usize, 0usize);
    for px in rgba.chunks_exact_mut(4) {
        n += 1;
        let mx = px[0].max(px[1]).max(px[2]);
        if mx >= 254 {
            hell += 1;
            if markieren {
                px[0] = 255;
                px[1] = 0;
                px[2] = 0;
            }
        } else if mx <= 2 {
            dunkel += 1;
            if markieren {
                px[0] = 0;
                px[1] = 90;
                px[2] = 255;
            }
        }
    }
    let n = n.max(1) as f32;
    (hell as f32 / n, dunkel as f32 / n)
}

fn histogram(rgba: &[u8]) -> [[f32; 64]; 3] {
    let mut h = [[0f32; 64]; 3];
    let mut n = 0f32;
    for px in rgba.chunks_exact(4).step_by(7) {
        for c in 0..3 {
            let b = (px[c] as usize * 63) / 255;
            h[c][b] += 1.0;
        }
        n += 1.0;
    }
    if n > 0.0 {
        let peak = h
            .iter()
            .flat_map(|r| r.iter())
            .cloned()
            .fold(1.0f32, f32::max);
        for r in h.iter_mut() {
            for v in r.iter_mut() {
                *v /= peak;
            }
        }
    }
    h
}

/// Ein Regler wie in der Web-Version: Beschriftung links, Wert rechts,
/// Doppelklick setzt zurueck.
fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, min: f32, max: f32, def: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let resp = ui.add_sized(
            [96.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(12.0))
                .sense(egui::Sense::click())
                .halign(egui::Align::LEFT),
        );
        if resp.double_clicked() {
            *value = def;
            changed = true;
        }
        resp.on_hover_text(t("Doppelklick setzt zurück"));
        changed |= ui
            .add(egui::Slider::new(value, min..=max).show_value(false))
            .changed();
        let txt = if (*value - def).abs() < 0.01 {
            // Am Grundwert dessen Zahl zeigen (sonst stuende bei -0,001 „-0").
            egui::RichText::new(format!("{:.0}", def)).weak()
        } else {
            egui::RichText::new(grundpanel::mit_vorzeichen(*value, 0)).strong()
        };
        ui.add_sized([30.0, 18.0], egui::Label::new(txt.size(11.0)));
    });
    changed
}

impl eframe::App for App {
    /// Beim Schliessen den KI-Dienst beenden — er belegt sonst Gigabytes.
    fn on_exit(&mut self) {
        ki_lokal::beenden();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.zeichnen(ctx);
    }
}

impl App {
    /// Ein Bild der Oberflaeche — ohne eframe-Rahmen, damit der
    /// Bildschirmfoto-Pruefstand dieselbe Oberflaeche ohne Fenster zeichnen kann.
    fn zeichnen(&mut self, ctx: &egui::Context) {
        if self.egui_ctx.is_none() {
            self.egui_ctx = Some(ctx.clone());
        }
        self.pump(ctx);
        self.keyboard(ctx);
        // ⚠️ `any_down()`, nicht `any_pressed()`: Wer „Vorher/Nachher" GEDRUECKT
        // HAELT, bewegt die Maus nicht — mit `any_pressed` (nur der Moment des
        // Drueckens) liefen die 2,5 s im Vollbild weiter, die Leiste blendete
        // unter dem Finger aus, der Knopf verschwand und der Vergleich sprang
        // sofort zurueck — Vorher/Nachher ging im Vollbild schlicht nicht.
        if ctx.input(|i| i.pointer.velocity().length() > 1.0 || i.pointer.any_down()) {
            self.maus_zuletzt = std::time::Instant::now();
        }
        // Begleitdatei verzoegert schreiben: waehrend des Regelns waere jede
        // Aenderung eine Datei — eine Sekunde Ruhe genuegt als Ausloeser.
        if self.dirty
            && self
                .dirty_at
                .map(|t| t.elapsed().as_millis() > 900)
                .unwrap_or(false)
        {
            self.save_current_sidecar();
        }
        // Reihenfolge zaehlt: erst die Arbeitskopie neu aufbauen (Retusche steckt
        // in den Pixeln), dann zeichnen. Andersherum saehe man einen Zug lang das
        // alte Bild.
        if self.needs_reupload {
            self.reupload();
        }
        if self.needs_render {
            self.render_preview(ctx);
        }
        // Wann muss von selbst neu gezeichnet werden?
        //
        // egui zeichnet nur auf Anlass — Mausbewegung, Tastendruck. Alles, was im
        // Hintergrund laeuft, braucht deshalb einen eigenen Anstoss. `ai_busy` und
        // `dirty` fehlten hier, und das hatte zwei Folgen, die nach ganz
        // verschiedenen Fehlern aussahen: das KI-Protokoll stand still, solange man
        // die Maus nicht bewegte, und die **Begleitdatei wurde nie geschrieben** —
        // sie wird eine Sekunde nach der letzten Aenderung gesichert, und dieser
        // Zeitpunkt kam ohne Neuzeichnen nie an. Damit galt auch kein Foto je als
        // „bearbeitet".
        if self.loading.is_some()
            || self.batch_running
            || self.ai_busy
            || self.dirty
            || self.reupload_laeuft
            || self.needs_reupload
            || !self.thumbs_pending.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Tastenkuerzel: G schaltet die Galerie um (wie in anderen Bildprogrammen).
        if ctx.input(|i| i.key_pressed(egui::Key::G)) && !self.crop_mode {
            self.gallery_view = !self.gallery_view;
        }
        if self.fullscreen {
            // Vollbild heisst **Bild**, nicht „grosses Fenster mit denselben
            // Leisten": Regler, Filmstreifen und Kopfleiste verschwinden, das Foto
            // bekommt den ganzen Bildschirm.
            //
            // Das loest zugleich ein Problem, das man nur auf einem der Systeme
            // sieht: eine Leiste ganz oben am Bildschirmrand liegt im Vollbild unter
            // der automatisch einblendenden Menueleiste des Systems. Faehrt man
            // dorthin, klappt die Menueleiste darueber und fängt die Klicks ab — die
            // Knoepfe wirken dann „tot". Ohne Leiste gibt es dort nichts mehr zu
            // treffen.
            self.center(ctx);
        } else {
            self.top_bar(ctx);
            self.status_bar(ctx);
            if self.gallery_view {
                self.gallery(ctx);
            } else {
                self.side_panel(ctx);
                self.filmstrip(ctx);
                self.center(ctx);
            }
        }
        if self.show_log {
            self.log_fenster(ctx);
        }
        if self.gen_fenster {
            self.generator_fenster(ctx);
        }
        if self.pano_fenster {
            self.panorama_fenster(ctx);
        }
        if self.heal_mode && self.hinzufuegen_modus && generator::installiert() {
            self.hinzufuegen_fenster(ctx);
        }
        if self.show_batch {
            self.batch_window(ctx);
        }
        if self.batch_running {
            self.batch_progress(ctx);
        }
    }
}

impl App {
    /// Tastenkuerzel. Sterne auf 1–5 (0 loescht), Pfeile blaettern, F = Vollbild.
    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return; // in einem Textfeld hat Tippen Vorrang
        }
        let keys: Vec<(egui::Key, u8)> = vec![
            (egui::Key::Num0, 0),
            (egui::Key::Num1, 1),
            (egui::Key::Num2, 2),
            (egui::Key::Num3, 3),
            (egui::Key::Num4, 4),
            (egui::Key::Num5, 5),
        ];
        let mut set: Option<u8> = None;
        let mut step: i32 = 0;
        let mut toggle_full = false;
        // Die gewohnten Kuerzel gaengiger Raw-Entwickler (⌘ oder Strg je nach
        // System — egui fasst beides als `command`).
        let (mut z, mut zz, mut kop, mut einf, mut auto, mut j, mut w, mut v, mut r, mut vorher, mut d) =
            (false, false, false, false, false, false, false, false, false, false, false);
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            z = cmd && !shift && i.key_pressed(egui::Key::Z);
            zz = cmd && shift && i.key_pressed(egui::Key::Z);
            kop = cmd && shift && i.key_pressed(egui::Key::C);
            einf = cmd && shift && i.key_pressed(egui::Key::V);
            auto = cmd && i.key_pressed(egui::Key::U);
            if !cmd {
                j = i.key_pressed(egui::Key::J);
                w = i.key_pressed(egui::Key::W);
                v = i.key_pressed(egui::Key::V);
                r = i.key_pressed(egui::Key::R);
                vorher = i.key_pressed(egui::Key::Backslash);
                d = i.key_pressed(egui::Key::D);
            }
        });
        if z {
            self.rueckgaengig();
        }
        if zz {
            self.wiederholen();
        }
        if kop {
            self.einstellungen_kopieren();
        }
        if einf {
            self.einstellungen_einfuegen();
        }
        if auto {
            self.auto_anwenden();
        }
        if j {
            self.clipping = !self.clipping;
            self.needs_render = true;
        }
        if w && self.current.is_some() {
            self.pipette = !self.pipette;
            self.heal_mode = false;
        }
        if v && self.current.is_some() {
            self.push_undo();
            let e = self.edit();
            e.bw = !e.bw;
            self.needs_render = true;
            self.mark_dirty();
        }
        if r && self.current.is_some() {
            self.crop_mode = !self.crop_mode;
            self.needs_render = true;
        }
        if vorher {
            self.show_before = !self.show_before;
            self.needs_render = true;
        }
        if d {
            self.gallery_view = false;
        }
        if z || zz || kop || einf || auto {
            return;
        }
        ctx.input(|i| {
            for (k, n) in &keys {
                if i.key_pressed(*k) {
                    set = Some(*n);
                }
            }
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::Space) {
                step = 1;
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                step = -1;
            }
            if i.key_pressed(egui::Key::F) {
                toggle_full = true;
            }
            if i.key_pressed(egui::Key::Escape) && self.fullscreen {
                toggle_full = true;
            }
        });
        if let Some(n) = set {
            self.set_stars(n);
        }
        if step != 0 {
            if let Some(c) = self.current {
                let next = (c as i32 + step).clamp(0, self.gallery.len() as i32 - 1) as usize;
                if next != c {
                    self.select(next);
                }
            }
        }
        if toggle_full {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("oben").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button(t("Ordner öffnen")).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.open_paths(vec![dir]);
                    }
                }
                if ui.button(t("Dateien öffnen")).clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter(t("RAW-Dateien"), rawcore::RAW_EXTENSIONS)
                        .pick_files()
                    {
                        self.open_paths(files);
                    }
                }
                ui.separator();
                let has = self.current.is_some();
                if ui
                    .add_enabled(has && !self.undo.is_empty(), egui::Button::new(t("Zurück")))
                    .on_hover_text(t("Rückgängig (⌘Z)"))
                    .clicked()
                {
                    self.rueckgaengig();
                }
                if ui
                    .add_enabled(has && !self.redo.is_empty(), egui::Button::new(t("Vor")))
                    .on_hover_text(t("Wiederherstellen (⇧⌘Z)"))
                    .clicked()
                {
                    self.wiederholen();
                }
                if ui
                    .add_enabled(has, egui::Button::new(t("Kopieren")))
                    .on_hover_text(t("Einstellungen kopieren (⇧⌘C)"))
                    .clicked()
                {
                    self.einstellungen_kopieren();
                }
                if ui
                    .add_enabled(has && self.zwischenablage.is_some(), egui::Button::new(t("Einfügen")))
                    .on_hover_text(t("Einstellungen einfügen (⇧⌘V) — auf alle markierten Fotos"))
                    .clicked()
                {
                    self.einstellungen_einfuegen();
                }
                ui.separator();
                // Lokale KI: ohne Installation ein Knopf zum Installieren (ein Klick,
                // einmalig), waehrend des Ladens der Fortschritt, danach die KI.
                if let Some((text, anteil)) = self.ki_laedt.clone() {
                    ui.add(egui::ProgressBar::new(anteil).desired_width(140.0).show_percentage());
                    ui.label(egui::RichText::new(text).size(11.0));
                    if ui.button(t("abbrechen")).clicked() {
                        self.ki_abbruch.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                } else if !self.ki_installiert {
                    if let Err(grund) = ki_lokal::verfuegbar() {
                        ui.label(egui::RichText::new(t("Lokale KI nicht verfügbar")).size(11.0).weak())
                            .on_hover_text(grund);
                    } else if ui
                            .button(tf("Lokale KI installieren ({} GB)", &[&format!("{:.1}", ki_lokal::paketgroesse() as f64 / 1e9)]))
                            .on_hover_text(t(
                                "Lädt einmalig ein Bildmodell und ein Retusche-Modell auf diesen \
                                 Rechner. Danach läuft die KI komplett lokal — kein Foto verlässt \
                                 den Rechner, kein Konto, keine Kosten.",
                            ))
                            .clicked()
                    {
                        self.ki_installieren();
                    }
                } else {
                    let magic = egui::Button::new(if self.ai_busy {
                        t("KI denkt…")
                    } else {
                        t("Magisch entwickeln")
                    });
                    if ui
                        .add_enabled(has && !self.ai_busy, magic)
                        .on_hover_text(t("Die lokale KI schaut sich das Foto an und entwickelt es passend zur Szene"))
                        .clicked()
                    {
                        self.start_ai();
                    }
                    egui::ComboBox::from_id_salt("ki_staerke")
                        .selected_text(self.ai_strength.label())
                        .width(80.0)
                        .show_ui(ui, |ui| {
                            for s in [ai::Strength::Sanft, ai::Strength::Normal, ai::Strength::Kraeftig] {
                                ui.selectable_value(&mut self.ai_strength, s, s.label());
                            }
                        });
                    ui.separator();
                    if ui
                        .selectable_label(self.show_log, t("KI-Protokoll"))
                        .on_hover_text(t("Was die KI gesehen und gemacht hat"))
                        .clicked()
                    {
                        self.show_log = !self.show_log;
                    }
                }
                if generator::verfuegbar().is_ok()
                    && ui
                        .selectable_label(self.gen_fenster, t("Bild erzeugen…"))
                        .on_hover_text(t("Ein Bild aus einer Beschreibung erzeugen — lokal, ohne Konto"))
                        .clicked()
                {
                    self.gen_fenster = !self.gen_fenster;
                }
                if ui.selectable_label(self.gallery_view, t("Galerie (G)")).clicked() {
                    self.gallery_view = !self.gallery_view;
                }
                ui.separator();
                let has = self.current.is_some();
                if ui
                    .add_enabled(has, egui::Button::new(t("Dieses Foto sichern")))
                    .clicked()
                {
                    self.export_current();
                }
                if ui
                    .add_enabled(!self.gallery.is_empty(), egui::Button::new(t("Stapel entwickeln…")))
                    .on_hover_text(t("Viele Fotos auf einmal entwickeln"))
                    .clicked()
                {
                    self.show_batch = true;
                }
                let markiert = self.selected.iter().filter(|s| **s).count();
                if ui
                    .add_enabled(markiert >= 2, egui::Button::new(t("Panorama…")))
                    .on_hover_text(t("Aus den markierten Fotos ein Panorama zusammensetzen (mindestens zwei markieren)"))
                    .clicked()
                {
                    self.pano_fenster = true;
                    self.pano_fuellen = pipeline::heal_ki::verfuegbar();
                }
            });
            ui.add_space(4.0);
        });
    }

    /// Fusszeile: Status, KI-Meldung, Rechenzeit. Bewusst UNTEN und nicht in der
    /// Kopfleiste — dort schob sich der Text bei laengeren Meldungen ueber die Knoepfe.
    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if self.ai_busy {
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                    ui.label(egui::RichText::new(&self.status).size(11.0).color(farbe::TEXT_LEISE));
                    if !self.ai_note.is_empty() {
                        ui.label(egui::RichText::new("·").color(farbe::RAND));
                        ui.label(
                            egui::RichText::new(&self.ai_note)
                                .size(11.0)
                                .color(farbe::AKZENT),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Ganz rechts die Sprachwahl — unten statt in der Kopfleiste,
                        // die mit allen Knoepfen schon die Fensterbreite fuellt.
                        self.sprachwahl(ui);
                        self.update_knopf(ui, ctx);
                        if let Some(m) = &self.image_meta {
                            // Manche DNGs nennen keine Kamera — dann ohne
                            // fuehrende Leerstelle und Trennpunkt.
                            let kamera = format!("{} {}", m.make, m.model);
                            let aufnahme = format!(
                                "ISO {:.0} · {} · f/{:.1}",
                                m.iso,
                                m.shutter_text(),
                                m.aperture
                            );
                            let text = match kamera.trim() {
                                "" => aufnahme,
                                k => format!("{k} · {aufnahme}"),
                            };
                            ui.label(egui::RichText::new(text).size(11.0).color(farbe::TEXT_LEISE));
                        }
                        if self.last_render_ms > 0.0 {
                            ui.label(
                                egui::RichText::new(format!("{:.0} ms", self.last_render_ms))
                                    .size(11.0)
                                    .color(farbe::RAND),
                            );
                        }
                        // Welcher Bildschirm erkannt wurde. Steht bewusst sichtbar
                        // da: sonst weiss man nie, ob die Farben gerade auf das
                        // Geraet gerechnet werden oder nur auf sRGB geraten wird.
                        if let Some(gpu) = &self.gpu {
                            ui.label(
                                egui::RichText::new(sprache::monitor_text(gpu))
                                    .size(11.0)
                                    .color(farbe::TEXT_LEISE),
                            )
                            .on_hover_text(t(
                                "Die Vorschau wird auf dieses Profil gerechnet. \
                                 Exportierte Dateien bleiben sRGB.",
                            ));
                        }
                    });
                });
            });
    }

    /// Auswahl „Deutsch / English". Wirkt sofort — alle Texte werden in jedem
    /// Bild neu geholt — und gilt auch beim naechsten Start (`sprache::setzen`).
    fn sprachwahl(&mut self, ui: &mut egui::Ui) {
        let jetzt = sprache::aktuell();
        let mut wahl = jetzt;
        ui.scope(|ui| {
            // Flacher als die uebrigen Knoepfe: die Fusszeile ist nur 26 px hoch.
            ui.spacing_mut().button_padding = egui::vec2(6.0, 1.0);
            egui::ComboBox::from_id_salt("sprache")
                .selected_text(egui::RichText::new(jetzt.name()).size(11.0))
                .width(78.0)
                .show_ui(ui, |ui| {
                    for s in [sprache::Sprache::De, sprache::Sprache::En] {
                        ui.selectable_value(&mut wahl, s, s.name());
                    }
                })
                .response
                // Bewusst zweisprachig: wer die Sprache nicht versteht, muss
                // diesen Knopf trotzdem finden.
                .on_hover_text("Sprache / Language");
        });
        if wahl != jetzt {
            sprache::setzen(wahl);
            self.status_neu_beschriften();
        }
    }

    /// Die Bearbeitung **so, wie sie gerade angezeigt wird**.
    ///
    /// Im Zuschneide-Werkzeug steht bewusst das ganze Bild auf dem Schirm — dann
    /// darf auch keine Umrechnung den Zuschnitt einrechnen. Genau daran ist es beim
    /// ersten Versuch gescheitert: der nachgeladene Ausschnitt wurde durch die
    /// Zuschnittgroesse geteilt und deckte ploetzlich das ganze Bild ab, sodass es
    /// waehrend des Zuschneidens hineingezoomt aussah.
    fn anzeige_edit(&self) -> Edit {
        let mut e = self.edit_ref();
        if self.crop_mode {
            e.crop = None;
        }
        e
    }

    /// Umrechnung der Pinselgroesse: Anteil der **angezeigten** Breite → Anteil der
    /// **Quell**breite (in der die Flecken gespeichert sind).
    ///
    /// Zwei Dinge greifen ineinander: ein Zuschnitt zeigt nur einen Teil, und eine
    /// Vierteldrehung vertauscht die Achsen — die waagerechte Bildschirmachse
    /// entspricht dann der senkrechten des Originals. Ohne diese Umrechnung ist der
    /// gesetzte Fleck bei gedrehten oder beschnittenen Fotos deutlich zu gross
    /// oder zu klein.
    fn pinsel_faktor(&self) -> f32 {
        let e = self.anzeige_edit();
        let [_, _, cw, ch] = e.crop.unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let quer = matches!(e.rotate.rem_euclid(360), 90 | 270);
        match (&self.raw_full, quer) {
            (Some(raw), true) => ch * raw.height as f32 / raw.width.max(1) as f32,
            _ => cw,
        }
    }

    /// Wie deutlich die schwebenden Leisten gerade zu sehen sind.
    ///
    /// Im Fenster immer voll. Im Vollbild blenden sie sich nach zweieinhalb
    /// Sekunden ohne Mausbewegung weich aus — sonst waere „Vollbild" wieder nur
    /// ein grosses Fenster mit Leisten drumherum. Die kleinste Mausbewegung holt
    /// sie zurueck.
    fn leisten_deckkraft(&self, ctx: &egui::Context) -> f32 {
        if !self.fullscreen {
            return 1.0;
        }
        let ruhe = self.maus_zuletzt.elapsed().as_secs_f32();
        // Auch das Warten auf den Ausblendzeitpunkt braucht einen eigenen Anstoss —
        // sonst steht die Uhr still, bis der Nutzer zufaellig etwas tut, und die
        // Leisten bleiben fuer immer stehen. Gleiche Falle wie beim Sichern der
        // Begleitdatei.
        if ruhe < 3.3 {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        if ruhe < 2.5 {
            1.0
        } else {
            let d = (1.0 - (ruhe - 2.5) / 0.7).clamp(0.0, 1.0);
            if d > 0.02 {
                ctx.request_repaint();
            }
            d
        }
    }

    /// Die schwebenden Leisten ueber dem Bild: Zoom und Vollbild oben rechts,
    /// Werkzeuge unten mittig, Bewertung und Dateiname unten links.
    fn bild_leisten(&mut self, ctx: &egui::Context, rect: egui::Rect, img_rect: egui::Rect) {
        let d = self.leisten_deckkraft(ctx);
        let has = self.current.is_some();
        // Am BILD ausrichten, nicht am Fenster — sonst schwebt die Bewertung neben
        // dem Foto in der leeren Flaeche. Auf den sichtbaren Bereich begrenzt,
        // damit bei starkem Zoom nichts aus dem Fenster wandert.
        let am_bild = img_rect.intersect(rect);
        let am_bild = if am_bild.width() > 80.0 && am_bild.height() > 80.0 {
            am_bild
        } else {
            rect
        };
        // Massstab in echten Bildpixeln: „100 %" muss heissen, dass ein Bildpunkt
        // ein Bildschirmpunkt ist — nicht, dass das Bild eingepasst gezeigt wird.
        let massstab = match &self.raw_full {
            Some(raw) if raw.width > 0 => img_rect.width() / raw.width as f32 * 100.0,
            _ => self.zoom * 100.0,
        };

        // --- oben rechts: Ansicht ------------------------------------------
        schwebeleiste(ctx, "leiste_ansicht", am_bild.right_top() + egui::vec2(-14.0, 14.0), egui::Align2::RIGHT_TOP, d, |ui| {
            ui.horizontal(|ui| {
                let eingepasst = self.zoom <= 1.001;
                if ui
                    .add_enabled(!eingepasst, egui::Button::new(t("Einpassen")))
                    .on_hover_text(t("Ganzes Bild zeigen"))
                    .clicked()
                {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                if ui.add_enabled(has, egui::Button::new("−")).clicked() {
                    self.zoom = (self.zoom / 1.4).max(1.0);
                    if self.zoom <= 1.001 {
                        self.pan = egui::Vec2::ZERO;
                    }
                }
                ui.label(
                    egui::RichText::new(format!("{massstab:.0} %"))
                        .size(11.0)
                        .color(farbe::TEXT_LEISE),
                )
                .on_hover_text(t("Abbildungsmaßstab: 100 % = ein Bildpunkt auf einen Bildschirmpunkt"));
                if ui
                    .add_enabled(has, egui::Button::new("1:1"))
                    .on_hover_text(t("Originalgröße zeigen"))
                    .clicked()
                {
                    // So weit zoomen, bis ein Bildpunkt auf einem Bildschirmpunkt
                    // sitzt — der Blick, mit dem man Schärfe wirklich beurteilt.
                    if massstab > 1.0 {
                        self.zoom = (self.zoom * 100.0 / massstab).clamp(1.0, 12.0);
                    }
                }
                if ui.add_enabled(has, egui::Button::new("+")).clicked() {
                    self.zoom = (self.zoom * 1.4).min(12.0);
                }
                ui.separator();
                if ui
                    .selectable_label(self.fullscreen, t("Vollbild"))
                    .on_hover_text(t("Taste F — Esc zurück"))
                    .clicked()
                {
                    self.fullscreen = !self.fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                }
            });
        });

        // --- unten mittig: Werkzeuge ----------------------------------------
        schwebeleiste(ctx, "leiste_werkzeug", am_bild.center_bottom() + egui::vec2(0.0, -16.0), egui::Align2::CENTER_BOTTOM, d, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(has, egui::Button::selectable(self.crop_mode, t("Zuschneiden")))
                    .clicked()
                {
                    self.crop_mode = !self.crop_mode;
                    self.crop_drag = None;
                    if self.crop_mode {
                        self.heal_mode = false;
                    }
                    self.needs_render = true;
                }
                if self.edit_ref().crop.is_some()
                    && ui.add_enabled(has, egui::Button::new(t("aufheben"))).clicked()
                {
                    self.push_undo();
                    self.edit().crop = None;
                    self.needs_render = true;
                    self.mark_dirty();
                }
                ui.separator();
                if ui
                    .add_enabled(has, egui::Button::selectable(self.heal_mode, t("Objekte entfernen")))
                    .on_hover_text(t("Auf die Stelle klicken — sie wird aus der Umgebung geschlossen"))
                    .clicked()
                {
                    self.heal_mode = !self.heal_mode;
                    if self.heal_mode {
                        self.crop_mode = false;
                    }
                }
                if self.heal_mode {
                    ui.add(
                        egui::Slider::new(&mut self.heal_radius, 0.003..=0.06)
                            .show_value(false)
                            .text(t("Größe")),
                    );
                    if generator::installiert() {
                        ui.checkbox(&mut self.hinzufuegen_modus, t("Hinzufügen"))
                            .on_hover_text(t(
                                "Bereich anmalen und beschreiben, was dort erscheinen soll — \
                                 z. B. Wolken am klaren Himmel (lokal, ~1,5 min).",
                            ));
                    }
                    if ki_lokal::retusche_modell().is_some() && !self.hinzufuegen_modus {
                        ui.checkbox(&mut self.heal_ki_modus, t("KI-Füllung"))
                            .on_hover_text(t(
                                "An: die lokale KI erfindet die Stelle aus der Umgebung neu \
                                 (auch große Objekte, ~2 s). Aus: eine passende Nachbarstelle \
                                 wird kopiert (schnell, für kleine Flecken).",
                            ));
                    }
                    let n = self.edit_ref().heal.len() + self.edit_ref().heal_ki.len();
                    if ui
                        .add_enabled(n > 0, egui::Button::new(tf("zurück ({})", &[&n])))
                        .clicked()
                    {
                        self.push_undo();
                        // Zuerst aus der Liste des gewaehlten Pinsels, sonst aus der anderen.
                        let ki_zuerst = self.heal_ki_modus;
                        let e = self.edit();
                        let (erste, zweite) = if ki_zuerst {
                            (&mut e.heal_ki, &mut e.heal)
                        } else {
                            (&mut e.heal, &mut e.heal_ki)
                        };
                        if erste.pop().is_none() {
                            zweite.pop();
                        }
                        self.needs_reupload = true;
                        self.mark_dirty();
                    }
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("⟲")).on_hover_text(t("90° nach links")).clicked() {
                    self.drehen(-90);
                }
                if ui.add_enabled(has, egui::Button::new("⟳")).on_hover_text(t("90° nach rechts")).clicked() {
                    self.drehen(90);
                }
                if ui.add_enabled(has, egui::Button::new("↔")).on_hover_text(t("Waagerecht spiegeln")).clicked() {
                    self.push_undo();
                    let v = !self.edit_ref().flip_h;
                    self.edit().flip_h = v;
                    self.needs_render = true;
                    self.mark_dirty();
                }
                ui.separator();
                let vh = ui.add_enabled(has, egui::Button::new(t("Vorher / Nachher")));
                if vh.is_pointer_button_down_on() != self.show_before {
                    self.show_before = vh.is_pointer_button_down_on();
                    self.needs_render = true;
                }
                vh.on_hover_text(t("gedrückt halten"));
            });
        });

        // --- oben links: Bewertung und Datei --------------------------------
        // Bewusst OBEN und nicht unten: unten sitzt die Werkzeugleiste mittig, und
        // bei hochformatigen oder zugeschnittenen Bildern schoben sich beide
        // ineinander. Oben links ist frei und liegt dem Zoom oben rechts gegenüber.
        if let Some(pfad) = self.current_path() {
            schwebeleiste(ctx, "leiste_stern", am_bild.left_top() + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, d, |ui| {
                ui.horizontal(|ui| {
                    let jetzt = self.stars.get(&pfad).copied().unwrap_or(0);
                    if let Some(n) = sterne_leiste(ui, jetzt) {
                        self.set_stars(n);
                    }
                    ui.label(
                        egui::RichText::new(name_of(&pfad))
                            .size(11.0)
                            .color(farbe::TEXT_LEISE),
                    );
                });
            });
        }
    }


    /// Dreht in 90er-Schritten. Der Zuschnitt wird dabei aufgehoben: er ist als
    /// Anteil des *gedrehten* Bildes gespeichert und saesse danach quer im Bild.
    /// Rueckgaengig (⌘Z) — ein Schritt je Reglerzug.
    fn rueckgaengig(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let now = self.edit_ref();
            self.redo.push(now);
            if let Some(p) = self.current_path() {
                self.edits.insert(p, prev);
            }
            self.needs_render = true;
            self.mark_dirty();
        }
    }

    /// Wiederherstellen (⇧⌘Z).
    fn wiederholen(&mut self) {
        if let Some(next) = self.redo.pop() {
            let now = self.edit_ref();
            self.undo.push(now);
            if let Some(p) = self.current_path() {
                self.edits.insert(p, next);
            }
            self.needs_render = true;
            self.mark_dirty();
        }
    }

    /// „Auto" (⌘U): Ton und Farbe neu aus der Automatik — wie „Auto" in gaengigen
    /// Raw-Entwicklern.
    fn auto_anwenden(&mut self) {
        let Some(raw) = self.raw_full.clone() else { return };
        self.push_undo();
        let v = pipeline::auto::vorschlag(&raw);
        let e = self.edit();
        e.base_ev = v.belichtung_ev;
        e.exposure = 0.0;
        e.contrast = v.kontrast;
        e.highlights = v.lichter;
        e.shadows = v.tiefen;
        e.whites = v.weiss;
        e.blacks = v.schwarz;
        e.vibrance = pipeline::xmp::dynamik_aus_xmp(v.dynamik);
        e.saturation = v.saettigung;
        self.needs_render = true;
        self.mark_dirty();
    }

    /// Zurueck zur Grundausstattung des Fotos (Automatik bzw. XMP-Einstellungen).
    fn zuruecksetzen(&mut self) {
        let Some(p) = self.current_path() else { return };
        let Some(grund) = self.grundstaende.get(&p).cloned() else { return };
        self.push_undo();
        self.edits.insert(p, grund);
        self.needs_render = true;
        self.mark_dirty();
    }

    /// Weissabgleich aus einer Flaeche des Fotos (Anteile im Sensorbild):
    /// die Flaeche soll neutral werden. `None` = ganzes Bild (Automatik).
    fn weissabgleich_aus(&mut self, flaeche: Option<(f32, f32)>) {
        let (Some(raw), Some(img)) = (self.raw_full.clone(), self.image.as_ref()) else { return };
        let meta = &raw.meta;
        let (w, h) = (raw.width as usize, raw.height as usize);
        let mut summe = [0f64; 3];
        let mut n = 0f64;
        let mut nimm = |x: usize, y: usize, gewicht: f64| {
            let i = (y * w + x) * 3;
            let c = [raw.pixels[i] as f64, raw.pixels[i + 1] as f64, raw.pixels[i + 2] as f64];
            let mx = c[0].max(c[1]).max(c[2]) / 65535.0;
            // Ausgefressenes und Rauschen am Boden sagen nichts ueber das Licht.
            // Gesaettigt ist ein Kanal, sobald er nahe an Sensorwert 1 mal
            // angewandtem Faktor liegt.
            let gesaettigt = (0..3).any(|k| c[k] / 65535.0 > 0.95 * meta.wb_mul[k] as f64);
            if gesaettigt || mx < 0.003 {
                return;
            }
            for k in 0..3 {
                summe[k] += c[k] * gewicht;
            }
            n += gewicht;
        };
        match flaeche {
            Some((fx, fy)) => {
                let (cx, cy) = ((fx * w as f32) as usize, (fy * h as f32) as usize);
                let r = (w.max(h) / 400).max(3);
                for y in cy.saturating_sub(r)..(cy + r + 1).min(h) {
                    for x in cx.saturating_sub(r)..(cx + r + 1).min(w) {
                        nimm(x, y, 1.0);
                    }
                }
            }
            None => {
                // Automatik: nahezu neutrale Mitten zaehlen (unter dem
                // Aufnahme-Weissabgleich schon fast grau) — bunte Flaechen
                // wuerden das Ergebnis sonst einfaerben.
                let schritt = ((w * h / 120_000).max(1) as f64).sqrt().ceil() as usize;
                // Auf den Aufnahme-Weissabgleich bringen — die Pixel tragen ihn
                // nicht immer (Daten ohne Mosaik gleicht LibRaw nicht ab).
                let ausgleich: [f64; 3] =
                    std::array::from_fn(|k| meta.wb_aufnahme[k].max(1e-6) as f64 / meta.wb_mul[k].max(1e-6) as f64);
                let mut y = 0;
                while y < h {
                    let mut x = 0;
                    while x < w {
                        let i = (y * w + x) * 3;
                        let c = [
                            raw.pixels[i] as f64 * ausgleich[0],
                            raw.pixels[i + 1] as f64 * ausgleich[1],
                            raw.pixels[i + 2] as f64 * ausgleich[2],
                        ];
                        let mx = c[0].max(c[1]).max(c[2]);
                        let mn = c[0].min(c[1]).min(c[2]);
                        if mx > 0.0 && (mx - mn) / mx < 0.25 {
                            nimm(x, y, 1.0);
                        }
                        x += schritt;
                    }
                    y += schritt;
                }
            }
        }
        if n < 1.0 {
            self.status = t("Weißabgleich: keine brauchbare Stelle gefunden").into();
            return;
        }
        // Pixel = Kamera · Aufnahme-Multiplikator → Kamera-Neutral dieses Lichts.
        let mut neutral = [0f64; 3];
        for k in 0..3 {
            neutral[k] = summe[k] / n / (meta.wb_mul[k].max(1e-6) as f64);
        }
        let mx = neutral.iter().cloned().fold(0.0f64, f64::max).max(1e-12);
        for v in neutral.iter_mut() {
            *v /= mx;
        }
        let edit = self.edit_ref();
        let profil = img.profil_fuer(&edit);
        let xy = profil.neutral_zu_xy(&neutral);
        let (k, toenung) = pipeline::dcp::xy_zu_temp(xy);
        let (k0, t0) = img.weissabgleich_aufnahme();
        let (temp, tint) =
            pipeline::wb_regler(k0 as f64, t0 as f64, k.clamp(2000.0, 50000.0), toenung.clamp(-150.0, 150.0));
        self.push_undo();
        let e = self.edit();
        e.temp = temp.clamp(-300.0, 300.0);
        e.tint = tint.clamp(-150.0, 150.0);
        self.status = tf(
            "Weißabgleich: {} K, Tönung {}",
            &[&format!("{:.0}", k), &grundpanel::mit_vorzeichen(toenung as f32, 0)],
        );
        self.needs_render = true;
        self.mark_dirty();
    }

    fn weissabgleich_automatisch(&mut self) {
        self.weissabgleich_aus(None);
    }

    /// Einstellungen kopieren (⇧⌘C).
    fn einstellungen_kopieren(&mut self) {
        if self.current.is_some() {
            self.zwischenablage = Some(self.edit_ref());
            self.status = t("Einstellungen kopiert — ⇧⌘V fügt sie bei anderen Fotos ein").into();
        }
    }

    /// Einstellungen einfuegen (⇧⌘V): auf alle markierten Fotos, sonst auf das
    /// aktuelle. Wie in gaengigen Raw-Entwicklern: Ton, Farbe, Details, Effekte,
    /// Profil und Objektiv — Zuschnitt, Drehung und Retusche bleiben je Foto.
    fn einstellungen_einfuegen(&mut self) {
        let Some(quelle) = self.zwischenablage.clone() else { return };
        let ziele: Vec<PathBuf> = {
            let markiert: Vec<PathBuf> = self
                .gallery
                .iter()
                .zip(self.selected.iter())
                .filter(|(_, s)| **s)
                .map(|(p, _)| p.clone())
                .collect();
            if markiert.is_empty() {
                self.current_path().into_iter().collect()
            } else {
                markiert
            }
        };
        let aktuell = self.current_path();
        if aktuell.as_ref().is_some_and(|p| ziele.contains(p)) {
            self.push_undo();
        }
        for p in &ziele {
            let alt = self
                .edits
                .get(p)
                .cloned()
                .or_else(|| self.grundstaende.get(p).cloned())
                .unwrap_or_else(presets::start_edit);
            let mut neu = quelle.clone();
            neu.crop = alt.crop;
            neu.rotate = alt.rotate;
            neu.flip_h = alt.flip_h;
            neu.flip_v = alt.flip_v;
            neu.straighten = alt.straighten;
            neu.heal = alt.heal.clone();
            neu.heal_ki = alt.heal_ki.clone();
            neu.zusaetze = alt.zusaetze.clone();
            neu.bearbeitet = true;
            self.edits.insert(p.clone(), neu.clone());
            if Some(p) != aktuell.as_ref() {
                save_sidecar(p, &neu, self.stars.get(p).copied().unwrap_or(0));
                self.hat_sidecar.insert(p.clone());
            }
        }
        self.status = tf("Einstellungen auf {} Foto(s) übertragen", &[&ziele.len()]);
        self.needs_render = true;
        self.mark_dirty();
    }

    fn drehen(&mut self, grad: i32) {
        self.push_undo();
        let neu = (self.edit_ref().rotate + grad).rem_euclid(360);
        {
            let e = self.edit();
            e.rotate = neu;
            e.crop = None;
        }
        self.needs_render = true;
        self.mark_dirty();
    }

    fn export_current(&mut self) {
        let (Some(gpu), Some(path)) = (self.gpu.clone(), self.current_path()) else {
            return;
        };
        let edit = self.edit_ref();
        let settings = self.batch.clone();
        let target = settings.target_for(&path);
        let Some(chosen) = rfd::FileDialog::new()
            .set_file_name(name_of(&target))
            .set_directory(target.parent().unwrap_or(Path::new(".")))
            .save_file()
        else {
            return;
        };
        let mut settings = settings;
        settings.target_dir = chosen.parent().map(|p| p.to_path_buf());
        settings.suffix = String::new();
        // Der gewaehlte Dateiname gewinnt: wir bauen ihn ueber das Suffix nach.
        let stem = chosen
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let orig_stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        settings.suffix = stem.strip_prefix(&orig_stem).unwrap_or("").to_string();
        self.status = t("sichere…").into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let msg = match export::export_one(&gpu, &path, &edit, &settings) {
                Ok(ziel) => export::BatchMsg::Done {
                    file: path,
                    target: ziel,
                },
                Err(e) => export::BatchMsg::Failed {
                    file: path,
                    error: e.to_string(),
                },
            };
            let _ = tx.send(Msg::Batch(msg));
            let _ = tx.send(Msg::Batch(export::BatchMsg::Finished { ok: 1, failed: 0 }));
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("regler")
            .default_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.histogram_ui(ui);
                    ui.separator();
                    self.presets_ui(ui);
                    ui.separator();
                    if self.current.is_none() {
                        ui.label(t("Kein Foto geöffnet."));
                        return;
                    }
                    self.sliders_ui(ui);
                });
            });
    }

    fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 90.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 4.0, egui::Color32::from_rgb(10, 10, 12));
        let cols = [
            egui::Color32::from_rgba_unmultiplied(255, 80, 80, 150),
            egui::Color32::from_rgba_unmultiplied(80, 255, 120, 150),
            egui::Color32::from_rgba_unmultiplied(90, 140, 255, 150),
        ];
        // Clipping-Warnung wie in gaengigen Raw-Entwicklern: Dreiecke oben links
        // (Tiefen) und oben rechts (Lichter) leuchten, sobald etwas abschneidet;
        // ein Klick schaltet die Anzeige im Bild um (J).
        let (hell, dunkel) = self.clip_anteil;
        let dreieck = |p: &egui::Painter, rechts: bool, aktiv: bool, farbe: egui::Color32| {
            let x = if rechts { rect.right() - 14.0 } else { rect.left() + 4.0 };
            let y = rect.top() + 4.0;
            let pts = if rechts {
                vec![egui::pos2(x, y), egui::pos2(x + 10.0, y), egui::pos2(x + 10.0, y + 10.0)]
            } else {
                vec![egui::pos2(x, y), egui::pos2(x + 10.0, y), egui::pos2(x, y + 10.0)]
            };
            let fuell = if aktiv { farbe } else { egui::Color32::from_gray(60) };
            p.add(egui::Shape::convex_polygon(pts, fuell, egui::Stroke::NONE));
        };
        let schalter = ui.interact(rect, ui.id().with("histogramm"), egui::Sense::click());
        if schalter.clicked() {
            self.clipping = !self.clipping;
            self.needs_render = true;
        }
        schalter.on_hover_text(tf(
            "Lichter abgeschnitten: {} % · Tiefen abgeschnitten: {} % — Klick oder J zeigt sie im Bild",
            &[&format!("{:.2}", hell * 100.0), &format!("{:.2}", dunkel * 100.0)],
        ));
        let p = ui.painter();
        let n = self.histogram[0].len() as f32;
        dreieck(p, false, dunkel > 0.0005 || self.clipping, egui::Color32::from_rgb(70, 130, 255));
        dreieck(p, true, hell > 0.0005 || self.clipping, egui::Color32::from_rgb(255, 80, 80));
        for (c, chan) in self.histogram.iter().enumerate() {
            for (i, v) in chan.iter().enumerate() {
                let x = rect.left() + rect.width() * (i as f32 / n);
                let w = rect.width() / n;
                let h = rect.height() * v.min(1.0);
                p.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.bottom() - h),
                        egui::vec2(w, h),
                    ),
                    0.0,
                    cols[c],
                );
            }
        }
    }

    /// Eine Vorlage anwenden: zurueck auf den Startzustand, dann die Vorlage.
    /// Erhalten bleibt, was KEINE Optik ist: Grundhelligkeit, Zuschnitt,
    /// Drehung/Spiegelung und die Retuschestellen — eine Vorlage soll den Look
    /// wechseln, nicht die Arbeit am Bild wegwerfen.
    fn preset_anwenden(&mut self, apply: fn(&mut Edit)) {
        if self.current.is_none() {
            return;
        }
        self.push_undo();
        let alt = self.edit_ref();
        let basis = self
            .current_path()
            .and_then(|p| self.grundstaende.get(&p).cloned())
            .unwrap_or_else(presets::start_edit);
        let e = self.edit();
        *e = basis;
        e.bearbeitet = true;
        e.profil = alt.profil.clone();
        e.base_ev = alt.base_ev;
        e.crop = alt.crop;
        e.rotate = alt.rotate;
        e.flip_h = alt.flip_h;
        e.flip_v = alt.flip_v;
        e.straighten = alt.straighten;
        e.heal = alt.heal;
        e.heal_ki = alt.heal_ki;
        e.zusaetze = alt.zusaetze;
        // Die Objektiv-Profilwahl gehoert zum Foto (welches Glas war drauf),
        // nicht zum Look — eine Vorlage darf sie nicht zuruecksetzen.
        e.lens_auto = alt.lens_auto;
        e.lens_profil = alt.lens_profil.clone();
        apply(e);
        self.needs_render = true;
        self.mark_dirty();
    }

    fn presets_ui(&mut self, ui: &mut egui::Ui) {
        // Die Filmlooks zuerst — nur bei RAF-Kameras, deren eingebaute Bildmodi
        // sie nachempfinden: wer so eine Kamera hat, denkt in diesen Looks.
        // „Ausgewogen" ist der Ausgangspunkt, auf den die Grundwerte
        // kalibriert sind.
        let hat_filmmodi = self
            .image_meta
            .as_ref()
            .map(|m| m.make.to_ascii_lowercase().contains("fuji")) // Kennung: Herstellerkennung
            .unwrap_or(false);
        if hat_filmmodi {
        ui.label(egui::RichText::new(t("Filmlooks")).strong());
        ui.horizontal_wrapped(|ui| {
            let mut anwenden: Option<fn(&mut Edit)> = None;
            for p in presets::FILMMODI {
                // Name und Hinweis stehen deutsch in `presets` und werden hier uebersetzt.
                if ui.small_button(t(p.name)).on_hover_text(t(p.hinweis)).clicked() {
                    anwenden = Some(p.apply);
                }
            }
            if let Some(f) = anwenden {
                self.preset_anwenden(f);
            }
        });
        ui.add_space(4.0);
        }
        ui.label(egui::RichText::new(t("Vorlagen")).strong());
        ui.horizontal_wrapped(|ui| {
            let mut anwenden: Option<fn(&mut Edit)> = None;
            for p in presets::PRESETS {
                if ui.small_button(t(p.name)).clicked() {
                    anwenden = Some(p.apply);
                }
            }
            if let Some(f) = anwenden {
                self.preset_anwenden(f);
            }
        });
    }

    fn sliders_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut started = false;
        let mut e = self.edit_ref();

        // Die Kennung haelt den Auf-/Zu-Zustand fest; sie bleibt deutsch, damit
        // ein Sprachwechsel keine Gruppe auf- oder zuklappt.
        macro_rules! group {
            ($kennung:literal, $title:expr, $body:expr) => {
                egui::CollapsingHeader::new($title)
                    .id_salt($kennung)
                    .default_open(true)
                    .show(ui, $body);
            };
        }

        let info = grundpanel::FotoInfo {
            wb_aufnahme: self.image.as_ref().map(|b| b.weissabgleich_aufnahme()).unwrap_or((5500.0, 0.0)),
            profile: self.image.as_ref().map(|b| b.profile_zur_wahl()).unwrap_or_default(),
            profil_name: self
                .image
                .as_ref()
                .map(|b| b.profil.name.clone())
                .unwrap_or_else(|| "Fotowache Standard".into()),
        };
        let mut aktionen = grundpanel::Aktionen::default();
        let pipette = self.pipette;
        group!("Grundeinstellungen", t("Grundeinstellungen"), |ui: &mut egui::Ui| {
            aktionen = grundpanel::grundeinstellungen(ui, &mut e, &info, pipette);
        });
        changed |= aktionen.geaendert;
        // Farbkanaele: drei Regler Rot/Gruen/Blau, jeweils 0–100 %. 50 % laesst die
        // Farbe unveraendert, darunter wird sie blasser, darueber kraeftiger.
        //
        // Jeder Regler deckt ein breites 120°-Segment des Farbkreises ab (mehrere der
        // intern acht Farbbaender auf einmal). Genau daran krankte die alte 8-Knopf-
        // Oberflaeche: „Gruen" lag dort auf 135°, echtes Wiesengruen liegt aber bei
        // ~95° (technisch das „Gelb"-Band). Wer „Gruen" drehte, traf das Gras nie —
        // es sah aus, als haette der Regler keine Wirkung. Die breiten Gruppen
        // fangen das jetzt sicher.
        //
        // Die KI schreibt weiterhin fein aufgeloest in alle acht Baender; diese
        // Regler zeigen den staerksten Wert ihrer Gruppe und setzen beim Ziehen die
        // ganze Gruppe gemeinsam.
        group!("Farbkanäle", t("Farbkanäle"), |ui: &mut egui::Ui| {
            // Name → interne Bandindizes (0=Rot,1=Orange,2=Gelb,3=Grün,4=Cyan,
            // 5=Blau,6=Violett,7=Magenta). Zusammen decken die drei Gruppen den
            // ganzen Farbkreis lückenlos ab.
            let gruppen: [(&str, &[usize]); 3] = [
                (t("Rot"), &[0, 1, 7]),
                (t("Grün"), &[2, 3]),
                (t("Blau"), &[4, 5, 6]),
            ];
            ui.label(
                egui::RichText::new(t("50 % = unverändert · darunter blasser, darüber kräftiger"))
                    .size(10.0)
                    .color(farbe::TEXT_LEISE),
            );
            for (name, baender) in gruppen {
                // Reglerstand aus dem betragsgroessten Band der Gruppe ableiten.
                let ist = baender
                    .iter()
                    .map(|&b| e.hsl_s[b])
                    .fold(0.0, |a, v| if v.abs() > f32::abs(a) { v } else { a });
                let mut wert = ist / 2.0 + 50.0; // −100..100  →  0..100
                if slider(ui, name, &mut wert, 0.0, 100.0, 50.0) {
                    let s = (wert - 50.0) * 2.0; // 0..100  →  −100..100
                    for &b in baender {
                        e.hsl_s[b] = s;
                    }
                    changed = true;
                }
            }
        });
        group!("Farbverlauf (Split-Toning)", t("Farbverlauf (Split-Toning)"), |ui: &mut egui::Ui| {
            changed |= slider(ui, t("Tiefen · Ton"), &mut e.grade_shadow_hue, 0.0, 360.0, 0.0);
            changed |= slider(ui, t("Tiefen · Sätt."), &mut e.grade_shadow_sat, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Mitten · Ton"), &mut e.grade_mid_hue, 0.0, 360.0, 0.0);
            changed |= slider(ui, t("Mitten · Sätt."), &mut e.grade_mid_sat, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Lichter · Ton"), &mut e.grade_high_hue, 0.0, 360.0, 0.0);
            changed |= slider(ui, t("Lichter · Sätt."), &mut e.grade_high_sat, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Balance"), &mut e.grade_balance, -100.0, 100.0, 0.0);
        });
        group!("Effekte", t("Effekte"), |ui: &mut egui::Ui| {
            changed |= slider(ui, t("Vignette"), &mut e.vignette, -100.0, 100.0, 0.0);
            changed |= slider(ui, t("Körnung"), &mut e.grain, 0.0, 100.0, 0.0);
        });
        group!("Details", t("Details"), |ui: &mut egui::Ui| {
            changed |= slider(ui, t("Schärfen"), &mut e.sharpen, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Radius"), &mut e.sharpen_radius, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Maskierung"), &mut e.sharpen_mask, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Rauschen · Helligkeit"), &mut e.noise, 0.0, 100.0, 0.0);
            changed |= slider(ui, t("Rauschen · Farbe"), &mut e.color_noise, 0.0, 100.0, 0.0);
        });
        group!("Objektiv & Geometrie", t("Objektiv & Geometrie"), |ui: &mut egui::Ui| {
            // Profilkorrektur: Verzeichnung aus dem Objektivprofil plus die am
            // Bild gemessene Randabdunklung — an, solange niemand sie abschaltet
            // (wie die Referenz-Entwicklung bei RAF-Kameras). Die Regler darunter
            // sind Feinschliff OBENDRAUF, kein Ersatz.
            let vorher = e.lens_auto;
            ui.checkbox(&mut e.lens_auto, t("Profilkorrektur"));
            changed |= e.lens_auto != vorher;
            // Profil-Auswahl: automatisch (DNG-Daten → Profil → Schätzung)
            // oder eine feste Wahl — für Dateien, bei denen die Erkennung
            // nichts findet (z. B. Telefon-Fotos ohne eingebettete Korrektur).
            // Die Schluessel ("b061", `WAHL_TELEFON_*` …) sind intern und werden
            // so gespeichert (auch in aelteren Begleitdateien, deshalb nie
            // umbenennen); uebersetzt wird nur die Anzeige.
            const WAHL_TELEFON_HAUPT: &str = "iphone_haupt"; // Kennung: gespeicherter Schluessel
            const WAHL_TELEFON_UW: &str = "iphone_uw"; // Kennung: gespeicherter Schluessel
            const WAHL_TELEFON_TELE: &str = "iphone_tele"; // Kennung: gespeicherter Schluessel
            let profil_vorher = e.lens_profil.clone();
            let wahl_text = |w: &str| match w {
                "" => t("Automatisch"),
                "b061" => t("Reisezoom 18–300 mm (B061)"),
                WAHL_TELEFON_HAUPT => t("Telefon: Hauptkamera"),
                WAHL_TELEFON_UW => t("Telefon: Ultraweitwinkel"),
                WAHL_TELEFON_TELE => t("Telefon: Tele"),
                "schaetzung" => t("Schätzung aus Brennweite"),
                _ => t("Automatisch"),
            };
            egui::ComboBox::from_id_salt("lens_profil")
                .width(180.0)
                .selected_text(wahl_text(&e.lens_profil))
                .show_ui(ui, |ui| {
                    for w in [
                        "",
                        "b061",
                        WAHL_TELEFON_HAUPT,
                        WAHL_TELEFON_UW,
                        WAHL_TELEFON_TELE,
                        "schaetzung",
                    ] {
                        ui.selectable_value(&mut e.lens_profil, w.to_string(), wahl_text(w));
                    }
                })
                .response
                .on_hover_text(t(
                    "Welches Objektivprofil die Korrektur nutzt.\n\
                     Automatisch: Telefon-DNGs tragen ihre exakte Korrektur in der \
                     Datei (jedes Modell), sonst greift das hinterlegte Profil oder \
                     die Brennweiten-Schätzung.",
                ));
            changed |= e.lens_profil != profil_vorher;
            if let Some(img) = &self.image {
                let o = img.objektiv_wirksam(&e);
                // Der Quelltext kommt fest deutsch aus der Farbkette (`pipeline`)
                // und wird hier uebersetzt.
                let text = if o.aktiv() {
                    tf(
                        "{} · Vignette {}",
                        &[&t(o.quelle_text()), &format!("{:.0}", o.devignette)],
                    )
                } else {
                    t(o.quelle_text()).to_string()
                };
                ui.label(egui::RichText::new(text).size(10.0).color(farbe::TEXT_LEISE));
            }
            changed |= slider(ui, t("Vignette entfernen"), &mut e.lens_devignette, 0.0, 100.0, 0.0);
            // Positiv = Tonne beheben (gerade Linien woelben sich nach aussen),
            // negativ = Kissen beheben. Wirkt im zweiten Durchgang beim Abtasten.
            changed |= slider(ui, t("Verzeichnung"), &mut e.lens_distort, -100.0, 100.0, 0.0);
            changed |= slider(ui, t("Geraderichten"), &mut e.straighten, -15.0, 15.0, 0.0);
            changed |= slider(ui, t("Grauschleier"), &mut e.blackpoint, 0.0, 100.0, 0.0);
        });

        // Wurde irgendein Regler bewegt? egui meldet das ueber den Kontext.
        let gedrueckt = ui.ctx().input(|i| i.pointer.any_down());
        if gedrueckt {
            started = true;
        }
        let now = self.edit_ref();
        if now != e {
            e.bearbeitet = true;
            // EIN Rueckgaengig-Schritt je Reglerzug (vorher: einer je Bild des
            // Zugs — „Zurueck" ging dann nur ein paar Pixel zurueck).
            if !self.zug_laeuft && self.undo.last() != Some(&now) {
                self.undo.push(now.clone());
                if self.undo.len() > 100 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            self.zug_laeuft = started;
            if let Some(p) = self.current_path() {
                self.edits.insert(p, e);
            }
            self.needs_render = true;
        } else if changed {
            if let Some(p) = self.current_path() {
                self.edits.insert(p, e);
            }
            self.needs_render = true;
        }
        if !gedrueckt {
            self.zug_laeuft = false;
        }
        if aktionen.geaendert {
            self.mark_dirty();
        }
        if aktionen.pipette_umschalten {
            self.pipette = !self.pipette;
            self.heal_mode = false;
            self.crop_mode = false;
        }
        if aktionen.auto_wb {
            self.weissabgleich_automatisch();
        }
        if aktionen.auto_ton {
            self.auto_anwenden();
        }
        if aktionen.zuruecksetzen {
            self.zuruecksetzen();
        }
    }

    fn filmstrip(&mut self, ctx: &egui::Context) {
        if self.gallery.is_empty() {
            return;
        }
        egui::TopBottomPanel::bottom("filmstreifen")
            .exact_height(132.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let picked = self.selected.iter().filter(|s| **s).count();
                    let zeile = if picked > 0 {
                        tf("{} Fotos · {} ausgewählt", &[&self.gallery.len(), &picked])
                    } else {
                        tf("{} Fotos", &[&self.gallery.len()])
                    };
                    ui.label(egui::RichText::new(zeile).size(11.0).weak());
                    if ui.small_button(t("alle")).clicked() {
                        self.selected.iter_mut().for_each(|s| *s = true);
                    }
                    if ui.small_button(t("keine")).clicked() {
                        self.selected.iter_mut().for_each(|s| *s = false);
                    }
                });
                // Wechselt das aktuelle Foto (Galerie-Auswahl, Pfeiltasten),
                // springt der Streifen dorthin — sonst zeigt er irgendwo
                // hin, nur nicht auf das Bild, das man gerade sieht.
                let springen = self.current != self.film_zuletzt;
                self.film_zuletzt = self.current;
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let paths: Vec<PathBuf> = self.gallery.clone();
                        let clip = ui.clip_rect();
                        for (i, path) in paths.iter().enumerate() {
                            // Nur zeichnen, was im Blickfeld liegt (plus etwas
                            // Vorlauf): Bei hunderten Fotos wuerden sonst in jedem
                            // Frame hunderte Kacheln aufgebaut — und jede fehlende
                            // Miniatur angefordert, bevor man sie je sieht.
                            let breite = self
                                .thumbs
                                .get(path)
                                .map(|t| {
                                    let s = t.size_vec2();
                                    s.x * (84.0 / s.y.max(1.0))
                                })
                                .unwrap_or(110.0);
                            let x = ui.next_widget_position().x;
                            let is_current = self.current == Some(i);
                            if springen && is_current {
                                // Auch fuer weggelassene (gekuerzte) Kacheln:
                                // das Ziel-Rechteck reicht zum Hinrollen.
                                let ziel = egui::Rect::from_min_size(
                                    egui::pos2(x, ui.min_rect().top()),
                                    egui::vec2(breite, 100.0),
                                );
                                ui.scroll_to_rect(ziel, Some(egui::Align::Center));
                            }
                            if x + breite < clip.left() - 400.0 || x > clip.right() + 400.0 {
                                ui.add_space(breite);
                                continue;
                            }
                            self.request_thumb(path);
                            let bearbeitet = self.is_edited(path);
                            ui.vertical(|ui| {
                                let resp = if let Some(tex) = self.thumbs.get(path) {
                                    let size = tex.size_vec2();
                                    let scale = 84.0 / size.y.max(1.0);
                                    ui.add(
                                        egui::Image::new(tex)
                                            .fit_to_exact_size(size * scale)
                                            .sense(egui::Sense::click()),
                                    )
                                } else {
                                    ui.add_sized(
                                        [110.0, 84.0],
                                        egui::Button::new("…").sense(egui::Sense::click()),
                                    )
                                };
                                if resp.clicked() {
                                    self.select(i);
                                }
                                // Gruener Rahmen = fertig bearbeitet (innen), blauer
                                // Rahmen = gerade geoeffnet (aussen) — beide gleichzeitig
                                // sichtbar, sie ueberdecken sich nicht.
                                if bearbeitet {
                                    ui.painter().rect_stroke(
                                        resp.rect,
                                        2.0,
                                        egui::Stroke::new(2.0_f32, farbe::BEARBEITET),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if is_current {
                                    ui.painter().rect_stroke(
                                        resp.rect,
                                        2.0,
                                        egui::Stroke::new(2.0_f32, egui::Color32::LIGHT_BLUE),
                                        egui::StrokeKind::Outside,
                                    );
                                }
                                ui.horizontal(|ui| {
                                    if let Some(sel) = self.selected.get_mut(i) {
                                        ui.checkbox(sel, "");
                                    }
                                    let st = self.stars.get(path).copied().unwrap_or(0);
                                    if st > 0 {
                                        ui.label(
                                            egui::RichText::new("★".repeat(st as usize))
                                                .size(10.0)
                                                .color(egui::Color32::from_rgb(250, 200, 80)),
                                        );
                                    }
                                    ui.label(
                                        egui::RichText::new(name_of(path))
                                            .size(9.0)
                                            .weak(),
                                    );
                                });
                            });
                        }
                    });
                });
            });
    }

    fn gallery(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for f in [
                    Filter::Alle,
                    Filter::Bearbeitet,
                    Filter::Unbearbeitet,
                    Filter::MitSternen,
                ] {
                    let n = {
                        let save = self.filter;
                        self.filter = f;
                        let n = self.filtered().len();
                        self.filter = save;
                        n
                    };
                    if ui
                        .selectable_label(self.filter == f, format!("{}  {n}", f.label()))
                        .clicked()
                    {
                        self.filter = f;
                    }
                }
            });
            ui.add_space(8.0);

            let idx = self.filtered();
            if idx.is_empty() {
                ui.centered_and_justified(|ui| ui.label(t("Nichts in dieser Auswahl.")));
                return;
            }
            let breite = ui.available_width();
            let kachel = 220.0;
            let spalten = ((breite / (kachel + 12.0)).floor() as usize).max(1);
            let mut oeffnen: Option<usize> = None;

            // Zeilenweise mit `show_rows`: gezeichnet (und angefordert) wird nur,
            // was im Blickfeld liegt. Vorher baute jeder Frame ALLE Kacheln auf —
            // bei grossen Ordnern wurde die Galerie selbst damit zaeh.
            let zeilen_hoehe = kachel * 0.75 + 44.0;
            let zeilen = idx.len().div_ceil(spalten);
            egui::ScrollArea::vertical().show_rows(ui, zeilen_hoehe, zeilen, |ui, bereich| {
                for zeile in bereich {
                    ui.horizontal(|ui| {
                        for s in 0..spalten {
                            let Some(i) = idx.get(zeile * spalten + s) else {
                                break;
                            };
                            let path = self.gallery[*i].clone();
                            self.request_thumb(&path);
                            let bearbeitet = self.is_edited(&path);
                            let sterne = self.stars.get(&path).copied().unwrap_or(0);
                            ui.vertical(|ui| {
                                ui.set_width(kachel);
                                let resp = if let Some(tex) = self.thumbs.get(&path) {
                                    let sz = tex.size_vec2();
                                    let sc = (kachel / sz.x).min(kachel * 0.75 / sz.y);
                                    ui.add(
                                        egui::Image::new(tex)
                                            .fit_to_exact_size(sz * sc)
                                            .corner_radius(6)
                                            .sense(egui::Sense::click()),
                                    )
                                } else {
                                    ui.add_sized([kachel, kachel * 0.7], egui::Spinner::new())
                                };
                                if resp.double_clicked() || resp.clicked() {
                                    oeffnen = Some(*i);
                                }
                                // Gruener Rahmen = fertig bearbeitet — wie im
                                // Filmstreifen, auf einen Blick ohne Text.
                                if bearbeitet {
                                    ui.painter().rect_stroke(
                                        resp.rect,
                                        6.0,
                                        egui::Stroke::new(2.0_f32, farbe::BEARBEITET),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                ui.horizontal(|ui| {
                                    // Gruener Punkt = fertig entwickelt. Auf einen Blick
                                    // erkennbar, ohne Text zu lesen.
                                    let (farbe, text) = if bearbeitet {
                                        (farbe::BEARBEITET, t("bearbeitet"))
                                    } else {
                                        (farbe::TEXT_LEISE, t("offen"))
                                    };
                                    // Punkt selbst zeichnen: die Standardschrift von
                                    // egui hat kein „●" und zeigte nur ein leeres Kaestchen.
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(9.0, 9.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(rect.center(), 4.0, farbe);
                                    ui.label(egui::RichText::new(text).size(10.0).color(farbe));
                                    if sterne > 0 {
                                        ui.label(
                                            egui::RichText::new("★".repeat(sterne as usize))
                                                .size(10.0)
                                                .color(farbe::AKZENT),
                                        );
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(name_of(&path)).size(10.0).weak(),
                                );
                            });
                        }
                    });
                }
            });
            if let Some(i) = oeffnen {
                self.gallery_view = false;
                self.select(i);
            }
        });
    }

    fn center(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(err) = &self.gpu_error {
                ui.centered_and_justified(|ui| {
                    ui.label(tf("Keine Grafikkarte nutzbar: {}", &[err]));
                });
                return;
            }
            if self.loading.is_some() {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            let Some(tex) = self.texture.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label(t("Öffne einen Ordner mit RAW-Dateien."));
                });
                return;
            };

            let avail = ui.available_size();
            let tex_size = tex.size_vec2();
            let fit = (avail.x / tex_size.x).min(avail.y / tex_size.y);
            let shown = tex_size * fit * self.zoom;
            let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());

            // Zoomen mit dem Mausrad/Trackpad, Verschieben mit gedrueckter Maus.
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if resp.hovered() && scroll.abs() > 0.1 && !self.crop_mode {
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(1.0, 12.0);
                if self.zoom <= 1.001 {
                    self.pan = egui::Vec2::ZERO;
                }
            }
            if resp.dragged() && !self.crop_mode && !self.heal_mode && self.zoom > 1.001 {
                self.pan += resp.drag_delta();
            }

            let center = rect.center() + self.pan;
            let img_rect = egui::Rect::from_center_size(center, shown);

            // --- Zuschnitt ziehen ------------------------------------------
            if self.crop_mode {
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.crop_drag = Some((p, p));
                    }
                }
                if resp.dragged() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        if let Some(d) = &mut self.crop_drag {
                            d.1 = p;
                        }
                    }
                }
                if resp.drag_stopped() {
                    if let Some((a, b)) = self.crop_drag.take() {
                        let r = egui::Rect::from_two_pos(a, b).intersect(img_rect);
                        if r.width() > 12.0 && r.height() > 12.0 {
                            let x = (r.left() - img_rect.left()) / img_rect.width();
                            let y = (r.top() - img_rect.top()) / img_rect.height();
                            let w = r.width() / img_rect.width();
                            let h = r.height() / img_rect.height();
                            self.push_undo();
                            self.edit().crop = Some([x, y, w, h]);
                            self.needs_render = true;
                            self.mark_dirty();
                        }
                    }
                }
            }

            // --- Weissabgleich-Pipette (W) ----------------------------------
            if self.pipette && resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if img_rect.contains(p) {
                        let x = (p.x - img_rect.left()) / img_rect.width();
                        let y = (p.y - img_rect.top()) / img_rect.height();
                        let (x, y) = anzeige_zu_quelle(x, y, &self.anzeige_edit());
                        self.weissabgleich_aus(Some((x, y)));
                        self.pipette = false;
                    }
                }
            }

            // --- Objekte entfernen ------------------------------------------
            // Ein Klick setzt eine Stelle, Ziehen malt eine Spur davon. Gespeichert
            // wird als Anteil des Bildes, damit Vorschau und Export dieselbe Stelle
            // treffen — unabhaengig von der Aufloesung.
            if self.heal_mode && (resp.clicked() || resp.dragged()) {
                if let Some(p) = resp.interact_pointer_pos() {
                    if img_rect.contains(p) {
                        let x = (p.x - img_rect.left()) / img_rect.width();
                        let y = (p.y - img_rect.top()) / img_rect.height();
                        // Ueber `anzeige_zu_quelle`, nicht nur ueber die Drehung:
                        // ist ein Zuschnitt aktiv, zeigt der Bildschirm nur einen
                        // Ausschnitt — der Klick muss auch darum verrechnet werden.
                        let (x, y) = anzeige_zu_quelle(x, y, &self.anzeige_edit());
                        // Der Regler meint, was man auf dem Bildschirm sieht;
                        // gespeichert wird in Quellmassen.
                        let r = self.heal_radius * self.pinsel_faktor();
                        let ki = self.heal_ki_modus && ki_lokal::retusche_modell().is_some();
                        let hinzu = self.hinzufuegen_modus && generator::installiert();
                        let e = self.edit_ref();
                        let neu = if hinzu { &self.zusatz_flecken } else if ki { &e.heal_ki } else { &e.heal }
                            .last()
                            // Beim Ziehen nicht jeden Mauspunkt einzeln nehmen —
                            // sonst stapeln sich hundert Flecken uebereinander und
                            // der Neuaufbau dauert ewig.
                            .map(|l| {
                                let d = ((l[0] - x).powi(2) + (l[1] - y).powi(2)).sqrt();
                                d > r * 0.8
                            })
                            .unwrap_or(true);
                        if neu && hinzu {
                            // Nur markieren — eingesetzt wird erst auf Knopfdruck.
                            self.zusatz_flecken.push([x, y, r]);
                        } else if neu {
                            if resp.clicked() {
                                self.push_undo();
                            }
                            if ki {
                                self.edit().heal_ki.push([x, y, r]);
                            } else {
                                self.edit().heal.push([x, y, r]);
                            }
                            self.needs_reupload = true;
                            self.mark_dirty();
                        }
                    }
                }
            }

            // --- Zeichnen ---------------------------------------------------
            let painter = ui.painter_at(rect);
            let uv_ganz = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), img_rect, uv_ganz, egui::Color32::WHITE);
            // Nachgeladener Ausschnitt in echter Aufloesung — an seiner Stelle,
            // nicht ueber das ganze Bild gezogen.
            if let Some(dt) = self.detail_tex.clone() {
                if let Some(dr) = self.detail_rect_am_schirm(img_rect) {
                    painter.image(dt.id(), dr, uv_ganz, egui::Color32::WHITE);
                }
            }

            // --- Zuschnitt anzeigen -----------------------------------------
            if self.crop_mode {
                // Der Rahmen, der gerade gezogen wird — oder der bestehende.
                let rahmen = self.crop_drag.map(|(a, b)| egui::Rect::from_two_pos(a, b)).or_else(
                    || {
                        self.edit_ref().crop.map(|[x, y, w, h]| {
                            egui::Rect::from_min_size(
                                img_rect.left_top()
                                    + egui::vec2(x * img_rect.width(), y * img_rect.height()),
                                egui::vec2(w * img_rect.width(), h * img_rect.height()),
                            )
                        })
                    },
                );
                if let Some(r) = rahmen {
                    // Alles ausserhalb abdunkeln — so sieht man sofort, was bleibt.
                    let dunkel = egui::Color32::from_black_alpha(140);
                    let o = img_rect;
                    for teil in [
                        egui::Rect::from_min_max(o.left_top(), egui::pos2(o.right(), r.top())),
                        egui::Rect::from_min_max(egui::pos2(o.left(), r.bottom()), o.right_bottom()),
                        egui::Rect::from_min_max(
                            egui::pos2(o.left(), r.top()),
                            egui::pos2(r.left(), r.bottom()),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(r.right(), r.top()),
                            egui::pos2(o.right(), r.bottom()),
                        ),
                    ] {
                        if teil.width() > 0.0 && teil.height() > 0.0 {
                            painter.rect_filled(teil, 0.0, dunkel);
                        }
                    }
                    painter.rect_stroke(
                        r,
                        0.0,
                        egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                    // Drittel-Linien — die uebliche Hilfe beim Bildaufbau.
                    for i in 1..3 {
                        let f = i as f32 / 3.0;
                        let hell = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(70));
                        painter.line_segment(
                            [
                                egui::pos2(r.left() + r.width() * f, r.top()),
                                egui::pos2(r.left() + r.width() * f, r.bottom()),
                            ],
                            hell,
                        );
                        painter.line_segment(
                            [
                                egui::pos2(r.left(), r.top() + r.height() * f),
                                egui::pos2(r.right(), r.top() + r.height() * f),
                            ],
                            hell,
                        );
                    }
                }
            }
            if self.heal_mode {
                // Bereits geschlossene Stellen markieren, damit man sieht, was man
                // schon gemacht hat — die Retusche selbst ist ja unsichtbar.
                let e = self.edit_ref();
                for [hx, hy, hr] in e.heal.iter().chain(e.heal_ki.iter()) {
                    let (dx, dy) = quelle_zu_anzeige(*hx, *hy, &self.anzeige_edit());
                    let c = img_rect.left_top()
                        + egui::vec2(dx * img_rect.width(), dy * img_rect.height());
                    painter.circle_stroke(
                        c,
                        hr / self.pinsel_faktor().max(1e-6) * img_rect.width(),
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90)),
                    );
                }
                // Der Bereich fuer „Hinzufuegen" — gefuellt, damit man sieht,
                // wohin etwas kommt.
                for [hx, hy, hr] in self.zusatz_flecken.iter() {
                    let (dx, dy) = quelle_zu_anzeige(*hx, *hy, &self.anzeige_edit());
                    let c = img_rect.left_top() + egui::vec2(dx * img_rect.width(), dy * img_rect.height());
                    painter.circle_filled(
                        c,
                        hr / self.pinsel_faktor().max(1e-6) * img_rect.width(),
                        egui::Color32::from_rgba_unmultiplied(230, 170, 60, 60),
                    );
                }
                if let Some(p) = resp.hover_pos() {
                    painter.circle_stroke(
                        p,
                        self.heal_radius * img_rect.width(),
                        egui::Stroke::new(1.5_f32, farbe::AKZENT),
                    );
                }
            }
            if self.zoom > 1.001 {
                painter.text(
                    rect.left_bottom() + egui::vec2(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{:.0} %", self.zoom * fit * 100.0),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_white_alpha(160),
                );
            }

            // --- Echte Aufloesung nachladen ---------------------------------
            // Ab etwa 1:1 zeigt das Aufblasen der Vorschau nur noch Matsch. Dann
            // holen wir den sichtbaren Ausschnitt aus den Originalpixeln nach.
            self.maybe_load_detail(ctx, img_rect, rect);

            // Zum Schluss: die schwebenden Leisten liegen ueber allem.
            self.bild_leisten(ctx, rect, img_rect);
        });
    }

    /// Laedt bei starkem Zoom den sichtbaren Ausschnitt in voller Aufloesung nach.
    fn maybe_load_detail(&mut self, ctx: &egui::Context, img_rect: egui::Rect, view: egui::Rect) {
        let (Some(gpu), Some(raw), Some(prev)) = (&self.gpu, &self.raw_full, &self.image) else {
            return;
        };
        let edit = self.anzeige_edit();

        // Sichtbaren Bereich in **Anzeige**-Anteilen, dann in Quell-Anteile
        // uebersetzen. Der Umweg ist noetig, weil das Angezeigte gedreht und
        // beschnitten ist, die Quelle aber nicht.
        let vis = view.intersect(img_rect);
        if vis.width() < 8.0 || vis.height() < 8.0 {
            return;
        }
        let d0 = (
            (vis.left() - img_rect.left()) / img_rect.width(),
            (vis.top() - img_rect.top()) / img_rect.height(),
        );
        let d1 = (
            (vis.right() - img_rect.left()) / img_rect.width(),
            (vis.bottom() - img_rect.top()) / img_rect.height(),
        );
        let (sx0, sy0, sx1, sy1) = ecken_bbox(d0, d1, &edit, anzeige_zu_quelle);

        // Lohnt es sich ueberhaupt? Kriterium: zeigt der Bildschirm gerade mehr
        // Pixel je Quellpixel, als die verkleinerte Arbeitskopie hergibt?
        let quell_breite = ((sx1 - sx0) * raw.width as f32).max(1.0);
        let am_schirm = vis.width() / quell_breite;
        let in_vorschau = prev.width as f32 / raw.width as f32;
        if am_schirm <= in_vorschau * 1.05 {
            if self.detail.is_some() {
                self.detail = None;
                self.detail_tex = None;
                self.detail_rect = None;
            }
            return;
        }

        // Rand fuer die Entzerrung: ein Ausgabepixel holt seine Quelle bis zu
        // dieser Strecke entfernt (18 mm Tonne: ~460 px; Geraderichten mehr).
        let o = prev.objektiv_wirksam(&self.edit_ref());
        let winkel = self.edit_ref().straighten.to_radians().abs();
        let halbdiag = (raw.width as f32).hypot(raw.height as f32) / 2.0;
        let pad = 128.0
            + o.max_verschiebung(raw.width, raw.height)
            + 2.0 * (winkel / 2.0).sin() * halbdiag;
        let x0 = ((sx0 * raw.width as f32) - pad).max(0.0) as u32;
        let y0 = ((sy0 * raw.height as f32) - pad).max(0.0) as u32;
        let x1 = ((sx1 * raw.width as f32) + pad).min(raw.width as f32) as u32;
        let y1 = ((sy1 * raw.height as f32) + pad).min(raw.height as f32) as u32;
        let want = (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0));
        if want.2 < 8 || want.3 < 8 {
            return;
        }
        // Nur neu laden, wenn sich der Ausschnitt spuerbar verschoben hat.
        if let Some(have) = self.detail_rect {
            let moved = (have.0 as i64 - want.0 as i64).abs()
                + (have.1 as i64 - want.1 as i64).abs();
            let resized = (have.2 as i64 - want.2 as i64).abs()
                + (have.3 as i64 - want.3 as i64).abs();
            if moved < 64 && resized < 64 {
                return;
            }
        }
        // Retusche auch im Zoom: sonst waere ausgerechnet beim genauen Hinsehen
        // die Stelle wieder da, die man weggemacht hat.
        let lokal = |liste: &[[f32; 3]]| -> Vec<[f32; 3]> {
            liste
                .iter()
                .map(|[hx, hy, hr]| {
                    [
                        (hx * raw.width as f32 - want.0 as f32) / want.2 as f32,
                        (hy * raw.height as f32 - want.1 as f32) / want.3 as f32,
                        hr * raw.width as f32 / want.2 as f32,
                    ]
                })
                .collect()
        };
        let (heal_lokal, heal_ki_lokal) = (lokal(&edit.heal), lokal(&edit.heal_ki));
        let stuecke_lokal: Vec<pipeline::zusatz::Stueck> = self
            .current_path()
            .map(|p| zusatz::stuecke(&p, &edit.zusaetze))
            .unwrap_or_default()
            .iter()
            .map(|z| z.im_ausschnitt(raw.width, raw.height, want.0, want.1, want.2, want.3))
            .collect();
        let retusche = pipeline::Retusche { heal: &heal_lokal, heal_ki: &heal_ki_lokal, zusaetze: &stuecke_lokal };
        if let Ok(mut region) = GpuImage::upload_region_mit(gpu, raw, want.0, want.1, want.2, want.3, &retusche)
        {
            // Der Ausschnitt muss im VOLLEN Bildbezug entzerren — kachel-lokal
            // gerechnet spraenge er gegenueber der Vorschau.
            region.als_ausschnitt(want.0, want.1, raw.width, raw.height, prev.objektiv);
            // Dieselbe Basisebene wie die Vorschau — aus dem Ausschnitt allein
            // liesse sich die Umgebungshelligkeit am Rand nicht bestimmen.
            region.basis_von(prev);
            let voll = self.edit_ref();
            let rgba = region.render_screen(gpu, &voll);
            // Gleiche Drehung wie die Vorschau — sonst liegt der Ausschnitt quer
            // im ansonsten aufrechten Bild.
            let (rgba, rw, rh) = pipeline::ausrichten(
                &rgba,
                region.width,
                region.height,
                4,
                edit.rotate,
                edit.flip_h,
                edit.flip_v,
            );
            let color =
                egui::ColorImage::from_rgba_unmultiplied([rw as usize, rh as usize], &rgba);
            self.detail_tex = Some(ctx.load_texture("detail", color, egui::TextureOptions::LINEAR));
            self.detail_rect = Some(want);
            self.detail = Some(region);
        }
    }

    /// Wo der nachgeladene Ausschnitt auf dem Bildschirm liegt.
    ///
    /// Frueher wurde er einfach ueber das **ganze** Bildrechteck gemalt — bei
    /// starkem Zoom ist das um ein Vielfaches zu gross, der Ausschnitt sass also
    /// voellig falsch. Jetzt wird seine Lage zurueckgerechnet.
    fn detail_rect_am_schirm(&self, img_rect: egui::Rect) -> Option<egui::Rect> {
        let (raw, (x0, y0, w, h)) = (self.raw_full.as_ref()?, self.detail_rect?);
        let e = self.anzeige_edit();
        let a = (x0 as f32 / raw.width as f32, y0 as f32 / raw.height as f32);
        let b = (
            (x0 + w) as f32 / raw.width as f32,
            (y0 + h) as f32 / raw.height as f32,
        );
        let (u0, v0, u1, v1) = ecken_bbox(a, b, &e, quelle_zu_anzeige);
        Some(egui::Rect::from_min_max(
            img_rect.left_top() + egui::vec2(u0 * img_rect.width(), v0 * img_rect.height()),
            img_rect.left_top() + egui::vec2(u1 * img_rect.width(), v1 * img_rect.height()),
        ))
    }

    /// Das KI-Protokoll — Vorbild ist das Live-Log der Web-Version: Zeitstempel,
    /// kurze Meldung, Details zum Aufklappen.
    ///
    /// Ohne diese Ansicht war nicht zu erkennen, ob die Nachkontrolle ueberhaupt
    /// gelaufen ist: sie brach im Fehlerfall stumm ab, und aussen sah alles gleich
    /// aus wie bei einem einzelnen Durchgang.
    fn log_fenster(&mut self, ctx: &egui::Context) {
        let mut offen = self.show_log;
        egui::Window::new(t("KI-Protokoll"))
            // Feste Kennung: der Titel wechselt mit der Sprache, das Fenster nicht.
            .id(egui::Id::new("KI-Protokoll"))
            .open(&mut offen)
            .default_width(520.0)
            .default_height(340.0)
            // Unten links statt mittig: sonst deckt es beim Oeffnen genau das Foto
            // ab, dessen Entwicklung man beurteilen will.
            .default_pos(egui::pos2(24.0, ctx.content_rect().height() - 400.0))
            .resizable(true)
            .show(ctx, |ui| {
                if self.ai_log.is_empty() {
                    ui.label(
                        egui::RichText::new(t(
                            "Noch nichts. „Magisch entwickeln\" starten — hier steht dann jeder Schritt.",
                        ))
                        .color(farbe::TEXT_LEISE),
                    );
                    return;
                }
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, z) in self.ai_log.iter().enumerate() {
                            ui.horizontal_top(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("+{:>5.1} s", z.bei))
                                        .monospace()
                                        .size(11.0)
                                        .color(farbe::RAND),
                                );
                                ui.label(egui::RichText::new(&z.text).color(z.art.farbe()));
                            });
                            if !z.detail.trim().is_empty() {
                                egui::CollapsingHeader::new(t("Einzelheiten"))
                                    .id_salt(i)
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(&z.detail)
                                                .monospace()
                                                .size(11.0)
                                                .color(farbe::TEXT_LEISE),
                                        );
                                    });
                            }
                            ui.add_space(2.0);
                        }
                    });
            });
        self.show_log = offen;
    }

    fn batch_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_batch;
        egui::Window::new(t("Stapelverarbeitung"))
            .id(egui::Id::new("Stapelverarbeitung"))
            .open(&mut open)
            .resizable(false)
            // Mittig und eine Ebene hoeher als die schwebenden Leisten am
            // Bild — oben links lag sonst die Sterne-Leiste ueber der Titelzeile.
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let picked = self.selected.iter().filter(|s| **s).count();
                let n = if picked > 0 { picked } else { self.gallery.len() };
                ui.label(if picked > 0 {
                    tf("{} Fotos werden entwickelt (Auswahl)", &[&n])
                } else {
                    tf("{} Fotos werden entwickelt (alle)", &[&n])
                });
                ui.add_space(4.0);
                ui.radio_value(
                    &mut self.batch_auto,
                    true,
                    t("Jedes Foto automatisch entwickeln"),
                );
                ui.radio_value(
                    &mut self.batch_auto,
                    false,
                    t("Meine aktuelle Einstellung auf alle anwenden"),
                );
                ui.label(
                    egui::RichText::new(if self.batch_auto {
                        t("Jedes Foto bekommt seine eigenen automatischen Werte (Belichtung, \
                           Lichter, Schatten, Schwarz, Rauschen) — wie beim Öffnen eines Fotos, \
                           nur für alle auf einmal. Fotos, die du von Hand bearbeitet hast, \
                           behalten ihre Regler.")
                    } else {
                        t("Alle Fotos bekommen die Regler des gerade offenen Bildes. Von Hand \
                           bearbeitete Fotos behalten ihre eigenen.")
                    })
                    .size(11.0)
                    .weak(),
                );
                ui.separator();

                // Kennung „Format" wie bisher, die Beschriftung in der Sprache.
                egui::ComboBox::new("Format", t("Format"))
                    .selected_text(self.batch.format.label())
                    .show_ui(ui, |ui| {
                        for f in [
                            export::Format::Jpeg,
                            export::Format::Png,
                            export::Format::Tiff16,
                        ] {
                            ui.selectable_value(&mut self.batch.format, f, f.label());
                        }
                    });
                if self.batch.format == export::Format::Jpeg {
                    ui.add(egui::Slider::new(&mut self.batch.quality, 60..=100).text(t("Qualität")));
                }

                let mut limit = self.batch.max_dim.is_some();
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut limit, t("verkleinern auf")).changed() {
                        self.batch.max_dim = if limit { Some(2560) } else { None };
                    }
                    if let Some(m) = &mut self.batch.max_dim {
                        ui.add(egui::DragValue::new(m).range(600..=12000).suffix(" px"));
                    } else {
                        ui.label(egui::RichText::new(t("volle Auflösung")).weak());
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(t("Zielordner:"));
                    let txt = self
                        .batch
                        .target_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| t("neben den Originalen").into());
                    ui.label(egui::RichText::new(txt).size(11.0).weak());
                    if ui.small_button(t("wählen…")).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.batch.target_dir = Some(d);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("Namenszusatz:"));
                    ui.text_edit_singleline(&mut self.batch.suffix);
                });
                ui.label(
                    egui::RichText::new(t("Originale werden nie überschrieben."))
                        .size(11.0)
                        .weak(),
                );

                // Ohne lokale KI keine KI-Wahl — der Stapel laeuft dann ohne.
                if self.ki_installiert {
                    ui.separator();
                    ui.checkbox(&mut self.batch_ai, t("jedes Foto von der KI entwickeln lassen"));
                    if self.batch_ai {
                        ui.label(
                            egui::RichText::new(t(
                                "Die lokale KI sieht sich jedes Foto einzeln an — passend zur \
                                 Szene, aber langsamer (~10 s pro Foto). Klappt es bei einem \
                                 Foto nicht, wird es trotzdem entwickelt.",
                            ))
                            .size(11.0)
                            .weak(),
                        );
                    }
                }
                ui.separator();
                if ui.button(t("Stapel starten")).clicked() {
                    self.start_batch();
                }
            });
        if !self.show_batch {
            open = false;
        }
        self.show_batch = open;
    }

    fn batch_progress(&mut self, ctx: &egui::Context) {
        egui::Window::new(t("Stapel läuft"))
            .id(egui::Id::new("Stapel läuft"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let frac = if self.batch_total > 0 {
                    self.batch_done as f32 / self.batch_total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .text(format!("{} / {}", self.batch_done, self.batch_total)),
                );
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.batch_log.iter().rev().take(80).collect::<Vec<_>>().iter().rev() {
                            ui.label(egui::RichText::new(*line).size(11.0));
                        }
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Das Blaetter-Leck: eine Begleitdatei ohne gemessene Startwerte
    /// (base_ev 0) muss beim naechsten Laden geheilt werden — Grundhelligkeit,
    /// Schwarzpunkt, ISO-Rauschminderung und Kameralage nachziehen, ohne
    /// Nutzerwerte anzutasten.
    #[test]
    fn begleitdatei_ohne_startwerte_wird_geheilt() {
        let mut frisch = presets::start_edit();
        frisch.base_ev = 0.4;
        frisch.highlights = -60.0;
        frisch.shadows = 45.0;
        frisch.vibrance = 15.0;
        frisch.rotate = 90;

        // Alte Begleitdatei (Prozess 1, nur Automatik): neu aufgesetzt,
        // Aufnahme-Entscheidungen bleiben, gilt als unbearbeitet.
        let mut alt = presets::start_edit_v1();
        alt.base_ev = 1.9;
        alt.highlights = -40.0;
        assert!(startwerte_nachziehen(&mut alt, &frisch));
        assert_eq!(alt.prozess, pipeline::params::PROZESS);
        assert_eq!(alt.base_ev, 0.4);
        assert_eq!(alt.highlights, -60.0);
        assert_eq!(alt.rotate, 90, "Kameralage aus dem Grundstand");
        assert!(!alt.bearbeitet, "nur Automatik war es nicht bearbeitet");

        // Ein Zuschnitt ist Handarbeit: bleibt erhalten und zaehlt als bearbeitet.
        let mut zu = presets::start_edit_v1();
        zu.base_ev = 1.9;
        zu.crop = Some([0.1, 0.1, 0.8, 0.8]);
        assert!(startwerte_nachziehen(&mut zu, &frisch));
        assert_eq!(zu.crop, Some([0.1, 0.1, 0.8, 0.8]), "Zuschnitt bleibt");
        assert!(zu.bearbeitet);

        // Alte Begleitdatei MIT Handarbeit bleibt als bearbeitet erkennbar.
        let mut hand = presets::start_edit_v1();
        hand.base_ev = 1.9;
        hand.exposure = 30.0;
        assert!(startwerte_nachziehen(&mut hand, &frisch));
        assert!(hand.bearbeitet);

        // Unbearbeitet (Prozess 2): folgt dem aktuellen Grundstand.
        let mut auto = presets::start_edit();
        auto.base_ev = 0.1;
        assert!(startwerte_nachziehen(&mut auto, &frisch));
        assert_eq!(auto, frisch);

        // Bearbeitet (Prozess 2): bleibt bis aufs Bit unangetastet.
        let mut gesund = presets::start_edit();
        gesund.base_ev = 2.0;
        gesund.exposure = 33.0;
        gesund.bearbeitet = true;
        let kopie = gesund.clone();
        assert!(!startwerte_nachziehen(&mut gesund, &frisch));
        assert_eq!(gesund, kopie);
    }


    /// Ein dunkelgraues Testbild (linear, wie vom Sensor) muss in der Miniatur
    /// HELL herauskommen: Grundhelligkeit zieht die Flaeche auf Zielwert 0,20,
    /// dann sRGB-Kennlinie und Grundgradation. Ohne diese Schritte kam hier
    /// ~66 heraus — die Galerie zeigte alle Fotos viel zu dunkel.
    #[test]
    fn miniatur_ist_so_hell_wie_die_vorschau() {
        let raw = rawcore::RawImage {
            width: 64,
            height: 64,
            pixels: vec![(0.05f32 * 65535.0) as u16; 64 * 64 * 3],
            meta: rawcore::RawMeta {
                make: "Test".into(),
                model: "Grau".into(),
                muster_6x6: false,
                // Einheitsmatrix: Kamerafarbraum == XYZ, damit die Rechnung
                // von Hand nachpruefbar bleibt.
                cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                cam_mul: [1.0; 4],
                pre_mul: [1.0; 4],
                black: 0,
                cblack: [0; 4],
                maximum: 65535,
                iso: 100.0,
                shutter: 0.01,
                aperture: 2.8,
                focal_len: 35.0,
                flip: 0,
                timestamp: 0,
                lens: String::new(),
                artist: String::new(),
                dng_warp: None,
                wb_mul: [1.0; 4],
                wb_aufnahme: [1.0; 4],
                weiss_pegel: 1.0,
                belichtungsversatz: 0.0,
                dynamikbereich: [0; 6],
                grundbelichtung_dng: 0.0,
                datei_objektiv: None,
                brennweite_kb: 0.0,
                fokus_m: 0.0,
                dng_profil: None,
            },
        };
        let (w, h, rgba) = quick_thumb(&raw, 32);
        assert!(w >= 16 && h >= 16);
        // Erwartung: auto_exposure hebt 0,05 auf ~0,20 → nach Kennlinie und
        // Grundgradation liegt der Gruenkanal deutlich ueber 100 (vorher: ~66).
        let mitte = ((h / 2 * w + w / 2) * 4) as usize;
        let g = rgba[mitte + 1];
        assert!(
            (100..=190).contains(&g),
            "Miniatur-Helligkeit {g} — Grundhelligkeit oder Gradation fehlt"
        );
    }

    /// Die Farbrauschminderung waechst mit der ISO — die feste 22 liess
    /// High-ISO-Fotos voller Farbrauschen. Helligkeitsrauschen bleibt bewusst 0:
    /// das ist Korn, kein Fehler.
    #[test]
    fn rauschminderung_waechst_mit_der_iso() {
        let mut raw = rawcore::RawImage {
            width: 16,
            height: 16,
            pixels: vec![20000; 16 * 16 * 3],
            meta: rawcore::RawMeta {
                make: "Test".into(),
                model: "ISO".into(),
                muster_6x6: true,
                cam_to_xyz: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                cam_mul: [1.0; 4],
                pre_mul: [1.0; 4],
                black: 0,
                cblack: [0; 4],
                maximum: 65535,
                iso: 125.0,
                shutter: 0.01,
                aperture: 2.8,
                focal_len: 35.0,
                flip: 0,
                timestamp: 0,
                lens: String::new(),
                artist: String::new(),
                dng_warp: None,
                wb_mul: [1.0; 4],
                wb_aufnahme: [1.0; 4],
                weiss_pegel: 1.0,
                belichtungsversatz: 0.0,
                dynamikbereich: [0; 6],
                grundbelichtung_dng: 0.0,
                datei_objektiv: None,
                brennweite_kb: 0.0,
                fokus_m: 0.0,
                dng_profil: None,
            },
        };
        // Basis-ISO: Farbrauschen 25 wie im Standard der Referenz-Entwicklung,
        // Helligkeitsrauschen bleibt (Korn).
        assert_eq!(startwerte(&raw).color_noise, 25.0);
        assert_eq!(startwerte(&raw).noise, 0.0, "Basis-ISO: Korn ist kein Fehler");
        raw.meta.iso = 1600.0;
        let mittel = startwerte(&raw).color_noise;
        assert!(mittel > 30.0 && mittel < 40.0, "ISO 1600: {mittel}");
        // Ab ISO 800 greift auch die Helligkeits-Rauschminderung (NLM
        // verwaescht keine Kanten mehr — deshalb darf sie jetzt automatisch an).
        let hell = startwerte(&raw).noise;
        assert!(hell > 14.0 && hell < 30.0, "ISO 1600 Helligkeit: {hell}");
        raw.meta.iso = 12800.0;
        assert_eq!(startwerte(&raw).color_noise, 48.0, "Deckel bei 48");
        assert_eq!(startwerte(&raw).noise, 45.0, "Deckel bei 45");
    }

    /// Wachhund am Shader-Quelltext: die Rauschminderung muss FLECKEN
    /// vergleichen (NLM) und den Rausch-Sockel abziehen — faellt jemand auf
    /// den alten Pixel-Vergleich zurueck, verwaescht High-ISO wieder.
    #[test]
    fn rauschminderung_bleibt_fleckenbasiert() {
        let src = include_str!("../../pipeline/src/output.wgsl");
        assert!(src.contains("Fleckenabstand"), "NLM-Fleckenvergleich fehlt");
        assert!(src.contains("d2 - sockel"), "Rausch-Sockel-Abzug fehlt");
        // Die Filterbreite MUSS am gemessenen Pegel haengen — mit fester
        // Breite war die Korrektur auf Originalpixeln (1:1-Zoom, Export)
        // wirkungslos, weil dort das volle Sensorrauschen liegt.
        assert!(src.contains("sigma * ("), "Filterbreite haengt nicht am Rauschpegel");
        assert!(src.contains("u.detail2.w"), "gemessener Rauschpegel kommt nicht an");
    }

    /// Die Umrechnung Anzeige ↔ gespeichertes Bild muss sich exakt aufheben.
    ///
    /// Das ist die Stelle, an der die Retusche bei gedrehten Fotos daneben landet —
    /// und der Fehler faellt erst beim Export auf, wenn man ihn nicht hier faengt.
    #[test]
    fn heal_punkt_hin_und_zurueck() {
        let punkte = [(0.10f32, 0.20f32), (0.5, 0.5), (0.83, 0.07)];
        for rotate in [0, 90, 180, 270] {
            for flip_h in [false, true] {
                for flip_v in [false, true] {
                    let e = Edit {
                        rotate,
                        flip_h,
                        flip_v,
                        ..Default::default()
                    };
                    for (x, y) in punkte {
                        let (dx, dy) = heal_punkt_drehen(x, y, &e);
                        let (rx, ry) = heal_punkt_zurueckdrehen(dx, dy, &e);
                        assert!(
                            (rx - x).abs() < 1e-5 && (ry - y).abs() < 1e-5,
                            "rotate={rotate} h={flip_h} v={flip_v}: ({x},{y}) → ({dx},{dy}) → ({rx},{ry})"
                        );
                    }
                }
            }
        }
    }

    /// Ein Punkt oben links muss sich nach einer Vierteldrehung nach rechts oben
    /// bewegen — sonst stimmt zwar die Umkehrung, aber die Richtung ist verkehrt.
    #[test]
    fn drehung_geht_in_die_richtige_richtung() {
        let e = Edit {
            rotate: 90,
            ..Default::default()
        };
        let (x, y) = heal_punkt_drehen(0.1, 0.1, &e);
        assert!(x > 0.8 && y < 0.2, "erwartet rechts oben, ist ({x}, {y})");
    }

    /// Anzeige ↔ Quelle muss sich auch **mit Zuschnitt** exakt aufheben.
    ///
    /// Das ist die Kombination, die beim Zuschneiden schiefging: das Bild wird
    /// gedreht UND beschnitten gezeigt, die Retusche und der Zoom rechnen aber in
    /// Quellkoordinaten. Jede der beiden Umrechnungen fuer sich war richtig.
    #[test]
    fn anzeige_und_quelle_heben_sich_auf() {
        for crop in [None, Some([0.2f32, 0.1, 0.5, 0.4]), Some([0.0, 0.0, 1.0, 1.0])] {
            for rotate in [0, 90, 180, 270] {
                for flip_h in [false, true] {
                    let e = Edit { rotate, flip_h, crop, ..Default::default() };
                    for (dx, dy) in [(0.0f32, 0.0f32), (0.25, 0.75), (1.0, 1.0)] {
                        let (sx, sy) = anzeige_zu_quelle(dx, dy, &e);
                        let (rx, ry) = quelle_zu_anzeige(sx, sy, &e);
                        assert!(
                            (rx - dx).abs() < 1e-4 && (ry - dy).abs() < 1e-4,
                            "crop={crop:?} rot={rotate} h={flip_h}: ({dx},{dy}) → ({sx},{sy}) → ({rx},{ry})"
                        );
                    }
                }
            }
        }
    }

    /// Und die Ecken muessen im Quellbild wieder ein Rechteck aufspannen, das
    /// den Ausschnitt umschliesst — darauf beruht das Nachladen beim Zoomen.
    #[test]
    fn eckenkasten_umschliesst_den_ausschnitt() {
        let e = Edit {
            rotate: 90,
            crop: Some([0.25, 0.25, 0.5, 0.5]),
            ..Default::default()
        };
        let (x0, y0, x1, y1) = ecken_bbox((0.0, 0.0), (1.0, 1.0), &e, anzeige_zu_quelle);
        assert!(x0 >= -1e-4 && y0 >= -1e-4 && x1 <= 1.0001 && y1 <= 1.0001, "{x0} {y0} {x1} {y1}");
        // Ein halb so grosser Ausschnitt bleibt auch nach der Drehung halb so gross.
        assert!(((x1 - x0) - 0.5).abs() < 1e-4, "Breite {}", x1 - x0);
        assert!(((y1 - y0) - 0.5).abs() < 1e-4, "Hoehe {}", y1 - y0);
    }

    /// Zeichen, die die mitgelieferte Schrift **nicht** hat, duerfen nicht in die
    /// Oberflaeche geraten — sie erscheinen als leeres Kaestchen.
    ///
    /// Das ist hier schon dreimal passiert (Aufzaehlungspunkt, Blenden-Symbol vor
    /// „Magisch entwickeln", Pfeile im Vollbild-Hinweis) und jedes Mal erst auf
    /// einem Bildschirmfoto aufgefallen. Diese Liste ist nachgeprueft: was hier
    /// steht, wurde am laufenden Programm als leeres Kaestchen gesehen; Sterne und
    /// die Dreh-Pfeile fehlen bewusst, die werden nachweislich gezeichnet.
    ///
    /// Geprueft werden alle Dateien der Oberflaeche und die englische Tabelle —
    /// eine Uebersetzung kann das Kaestchen genauso hereinholen wie das Original.
    #[test]
    fn keine_fehlenden_schriftzeichen_in_der_oberflaeche() {
        const FEHLEN: &[char] = &['✦', '●', '→', '←', '✓', '✗', '⟵', '⟶'];
        let mut treffer = Vec::new();
        for (datei, quelle) in sprache::pruefung::QUELLEN {
            // ai.rs nur ueber seine t()-Texte (unten): der Rest dort ist die
            // Anweisung an den KI-Server und erscheint nie auf dem Schirm.
            if *datei != "ai.rs" {
                // Kommentare und Testmodule zaehlen nicht — dort wird nichts
                // gezeigt, Testmeldungen landen im Terminal. Sonst gilt die Regel
                // ohne Ausnahme, auch fuer Terminal-Ausgaben: eine Ausnahmeliste
                // waere genau die Stelle, an der spaeter wieder etwas durchrutscht.
                let programm = sprache::pruefung::nur_programm(quelle);
                for (nr, zeile) in programm.lines().enumerate() {
                    for c in FEHLEN {
                        if zeile.contains(*c) && zeile.contains('"') {
                            treffer.push(format!("{datei}:{}: {c} in {}", nr + 1, zeile.trim()));
                        }
                    }
                }
            }
            // Jeder Text in t()/tf() — auch ueber mehrere Zeilen fortgesetzte.
            for f in sprache::pruefung::texte(quelle) {
                for c in FEHLEN {
                    if f.text.contains(*c) {
                        treffer.push(format!("{datei}:{}: {c} in {:?}", f.zeile, f.text));
                    }
                }
            }
        }
        for (de, en) in sprache::pruefung::tabelle_en() {
            for c in FEHLEN {
                if de.contains(*c) || en.contains(*c) {
                    treffer.push(format!("Tabelle: {c} in {de:?} / {en:?}"));
                }
            }
        }
        assert!(treffer.is_empty(), "fehlende Zeichen in Texten:\n{}", treffer.join("\n"));
    }

    /// Die Lageangabe der Kamera muss in die uebliche Zaehlweise uebersetzt werden.
    #[test]
    fn kameralage_wird_uebersetzt() {
        assert_eq!(drehung_aus_kamera(0), 0);
        assert_eq!(drehung_aus_kamera(3), 180);
        assert_eq!(drehung_aus_kamera(5), 270);
        assert_eq!(drehung_aus_kamera(6), 90);
        // Unbekannte Werte duerfen nicht raten.
        assert_eq!(drehung_aus_kamera(99), 0);
    }
}

// ---------------------------------------------------------------------------
// Schwebende Bedienelemente ueber dem Bild
//
// Warum ueberhaupt: Zuschneiden, Zoom, Drehen, Sterne und Vollbild gehoeren zum
// BILD, nicht zur Dateiverwaltung. In einer Kopfleiste ganz oben sind sie weit
// weg von dem, worauf sie wirken — und im Vollbild waren sie gar nicht mehr
// erreichbar. Als schwebende Leisten am Bildrand sind sie immer da, wo man
// hinschaut, und im Vollbild genauso benutzbar wie sonst.
// ---------------------------------------------------------------------------

/// Eine schwebende Leiste an einem Ankerpunkt.
fn schwebeleiste(
    ctx: &egui::Context,
    id: &str,
    punkt: egui::Pos2,
    pivot: egui::Align2,
    deckkraft: f32,
    inhalt: impl FnOnce(&mut egui::Ui),
) {
    if deckkraft <= 0.02 {
        return;
    }
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(punkt)
        .pivot(pivot)
        .show(ctx, |ui| {
            ui.set_opacity(deckkraft);
            egui::Frame::new()
                .fill(farbe::SCHWEBE)
                .corner_radius(12.0)
                .inner_margin(egui::Margin::symmetric(10, 7))
                .stroke(egui::Stroke::new(1.0_f32, farbe::RAND))
                .show(ui, inhalt);
        });
}

/// Ein fuenfzackiger Stern, gezeichnet.
///
/// Gezeichnet und nicht geschrieben: als Text waere er von der Schriftgroesse
/// abhaengig und liesse sich nicht sauber anklicken. Ausserdem soll ein halb
/// vergebener Stern erkennbar sein, ohne dass man die Zahl daneben liest.
fn stern_malen(painter: &egui::Painter, mitte: egui::Pos2, r: f32, gefuellt: bool) {
    let mut ecken = Vec::with_capacity(10);
    for i in 0..10 {
        let winkel = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let rr = if i % 2 == 0 { r } else { r * 0.42 };
        ecken.push(mitte + egui::vec2(winkel.cos() * rr, winkel.sin() * rr));
    }
    let (fuellung, strich) = if gefuellt {
        (farbe::AKZENT, egui::Stroke::NONE)
    } else {
        (
            Color32::TRANSPARENT,
            egui::Stroke::new(1.2_f32, farbe::TEXT_LEISE),
        )
    };
    painter.add(egui::Shape::convex_polygon(ecken, fuellung, strich));
}

/// Fuenf anklickbare Sterne. Gibt die neue Bewertung zurueck, wenn geklickt wurde.
/// Ein Klick auf den bereits gesetzten Stern loescht die Bewertung — so kommt man
/// ohne Zusatzknopf wieder auf null.
fn sterne_leiste(ui: &mut egui::Ui, aktuell: u8) -> Option<u8> {
    let r = 9.0;
    let abstand = r * 2.4;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(abstand * 5.0, r * 2.4),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let unter_maus = resp.hover_pos().map(|p| {
        (((p.x - rect.left()) / abstand).floor() as i32 + 1).clamp(1, 5) as u8
    });
    let zeigen = unter_maus.unwrap_or(aktuell);
    for i in 0..5u8 {
        let mitte = egui::pos2(rect.left() + abstand * (i as f32 + 0.5), rect.center().y);
        stern_malen(&painter, mitte, r, i < zeigen);
    }
    if resp.clicked() {
        if let Some(n) = unter_maus {
            return Some(if n == aktuell { 0 } else { n });
        }
    }
    None
}

