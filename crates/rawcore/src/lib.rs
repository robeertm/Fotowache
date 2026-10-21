//! `rawcore` — RAW-Dateien einlesen, entwickeln bis zum linearen Kamerabild.
//!
//! Das ist die Wurzel des Qualitaetsunterschieds zur Web-Version. Dort kam das Bild
//! als 8-Bit-JPEG-Vorschau (oder bestenfalls als WASM-Demosaic) in eine Canvas — hier
//! bekommen wir **16 Bit pro Kanal, linear, im Farbraum der Kamera**, dazu die Werte,
//! die man fuer echtes Farbmanagement braucht: die Matrix Kamera→CIE XYZ und den
//! Weissabgleich, wie die Kamera ihn aufgenommen hat.
//!
//! ## Warum der Weissabgleich schon hier passiert und die Farbmatrix nicht
//! Demosaicing rechnet zwischen benachbarten Sensorzellen. Sind die Kanaele noch
//! unbalanciert (Rot typisch halb so hell wie Gruen), rateert jeder Interpolator
//! schlechter — deshalb legt LibRaw den Weissabgleich der Kamera **vor** das
//! Demosaicing, so wie es auch andere Raw-Entwickler tun.
//! Die Farbmatrix dagegen bleibt bewusst liegen: sie wandert in die GPU. Nur so
//! kann man Temperatur/Tint spaeter noch anfassen, ohne die Datei neu zu dekodieren
//! (40 MP mit 6x6-Farbmuster = ein bis zwei Sekunden — als Regler unbrauchbar).

use std::ffi::{c_char, c_float, c_int, c_uint, c_void, CStr, CString};
use std::path::Path;

pub mod dng;
pub mod dng_schreiben;
pub mod raf;

mod ffi;
use ffi::*;

#[derive(Debug, thiserror::Error)]
pub enum RawError {
    #[error("Datei nicht lesbar: {0}")]
    Path(String),
    #[error("LibRaw konnte die Datei nicht oeffnen: {0}")]
    Open(String),
    #[error("LibRaw konnte die Rohdaten nicht entpacken: {0}")]
    Unpack(String),
    #[error("LibRaw konnte das Bild nicht entwickeln: {0}")]
    Process(String),
    #[error("LibRaw lieferte kein Bild zurueck (Fehler {0})")]
    NoImage(i32),
    #[error("unerwartetes Bildformat von LibRaw: {bits} Bit, {colors} Kanaele")]
    Unexpected { bits: u16, colors: u16 },
    #[error("LibRaw ist ohne Speicher")]
    OutOfMemory,
}

/// Wie stark demosaict wird. Fuer das 6x6-Farbmuster der RAF-Kameras waehlt LibRaw
/// ab `Hoch` automatisch Markesteijn (3 Durchgaenge) — das ist der Algorithmus, an dem
/// sich jeder Raw-Entwickler fuer dieses Muster messen lassen muss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemosaicQuality {
    /// Schnell, fuer Vorschauen/Stapel: bilinear.
    Schnell,
    /// Guter Kompromiss (PPG / 6x6-Muster 1-Pass).
    Mittel,
    /// Voll: DHT (Bayer) bzw. Markesteijn 3-Pass (6x6-Muster). Der Normalfall.
    Hoch,
}

impl DemosaicQuality {
    fn libraw_code(self) -> c_int {
        match self {
            DemosaicQuality::Schnell => 0,
            DemosaicQuality::Mittel => 2,
            // 11 = DHT fuer Bayer-Sensoren (die meisten Hersteller): sauberer als
            // das alte AHD (keine Labyrinth-Muster) und sogar schneller —
            // gemessen am Muster einer 20-MP-Bayer-Kamera 130 statt 153 ms. Das
            // 6x6-Muster prueft LibRaw VOR den Bayer-Verfahren: dort bleibt es
            // bei Markesteijn 3-Pass.
            DemosaicQuality::Hoch => 11,
        }
    }
}

/// Umgang mit ausgebrannten Lichtern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlights {
    /// Hart abschneiden — ehrlich, aber Lichter werden flach.
    Abschneiden,
    /// Kanaele mischen: rettet Zeichnung, ohne Farbstiche zu erfinden.
    Mischen,
    /// Rekonstruieren: holt am meisten zurueck, kann in Extremfaellen raten.
    Rekonstruieren,
}

impl Highlights {
    fn libraw_code(self) -> c_int {
        match self {
            Highlights::Abschneiden => 0,
            Highlights::Mischen => 2,
            Highlights::Rekonstruieren => 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodeOptions {
    pub demosaic: DemosaicQuality,
    pub highlights: Highlights,
    /// Halbe Aufloesung — fuer Filmstreifen-Vorschauen. Spart ~4x Zeit.
    pub half_size: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            demosaic: DemosaicQuality::Hoch,
            highlights: Highlights::Mischen,
            half_size: false,
        }
    }
}

/// Alles, was die GPU spaeter fuer korrekte Farben braucht.
#[derive(Debug, Clone)]
pub struct RawMeta {
    pub make: String,
    pub model: String,
    pub muster_6x6: bool,
    /// Kamera-RGB → CIE XYZ, 3x3 zeilenweise (aus LibRaws 4x3, vierte Zeile ist
    /// nur bei 4-Kanal-Sensoren belegt).
    pub cam_to_xyz: [[f32; 3]; 3],
    /// Weissabgleich, wie aufgenommen (R,G1,B,G2).
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub black: u32,
    pub cblack: [u32; 4],
    pub maximum: u32,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub flip: i32,
    /// Aufnahmezeitpunkt in Unix-Sekunden (0 = nicht hinterlegt).
    pub timestamp: i64,
    /// Objektiv, wie die Kamera es notiert hat.
    pub lens: String,
    /// Fotograf, falls in der Kamera hinterlegt.
    pub artist: String,
    /// Objektivkorrektur, die der Hersteller in die DNG geschrieben hat
    /// (Telefone & Co.) — exakt fuer diese Aufnahme, siehe `dng.rs`.
    pub dng_warp: Option<dng::DngWarp>,
    /// Die Weissabgleich-Faktoren, mit denen LibRaw die Pixel TATSAECHLICH
    /// multipliziert hat (R, G1, B, G2), in LibRaws Normierung: Pixel =
    /// Sensorwert (Saettigung 1) · Faktor. Nach dem Entwickeln nachgelesen.
    pub wb_mul: [f32; 4],
    /// Der Weissabgleich der Aufnahme (Faktoren aus der Datei) — legt
    /// „Wie Aufnahme" fest. Meist gleich `wb_mul` bis auf die Normierung;
    /// verschieden bei Daten ohne Mosaik, die LibRaw nicht abgleicht.
    pub wb_aufnahme: [f32; 4],
    /// Auf welchem Pixelwert neutrales Sensor-Weiss liegt. LibRaw normiert
    /// beim Mischen der Lichter auf den GROESSTEN Faktor — ein ausgefressenes
    /// Weiss landet dann nicht bei 1,0, sondern bei min/max der Faktoren
    /// (bei einer RAF im Tageslicht ~0,53). Die Farbkette skaliert darauf zurueck.
    pub weiss_pegel: f32,
    /// Belichtungs-Versatz der RAF-Kameras (ExpoMidPointShift, EV) und
    /// Dynamikbereich (100/200/400) — DR200/DR400 belichten das RAW 1–2 Blenden
    /// knapper.
    pub belichtungsversatz: f32,
    pub dynamikbereich: [u16; 6],
    /// BaselineExposure einer DNG (EV), sonst 0.
    pub grundbelichtung_dng: f32,
    /// RAF-Objektivkorrektur der Kamera, in die Datei eingebettet (Verzeichnung,
    /// Farbsaum, Vignettierung) — siehe `raf.rs`.
    pub datei_objektiv: Option<raf::RafObjektiv>,
    /// Brennweite in Kleinbild-mm laut EXIF (0 = unbekannt) — ergibt den
    /// Formatfaktor fuer Objektivprofile anderer Kameras.
    pub brennweite_kb: f32,
    /// Fokusabstand in Metern laut Makernote (0 = unbekannt).
    pub fokus_m: f32,
    /// Das Kameraprofil, das eine DNG selbst mitbringt (Telefone, Kameras mit
    /// DNG als Rohformat …) — als kleine DCP-artige TIFF-Datei, siehe
    /// `dng::lese_profil`.
    pub dng_profil: Option<Vec<u8>>,
}

impl RawMeta {
    /// Belichtungszeit lesbar: „1/250 s" statt „0.004".
    pub fn shutter_text(&self) -> String {
        if self.shutter <= 0.0 {
            return "—".into();
        }
        if self.shutter >= 1.0 {
            format!("{:.1} s", self.shutter)
        } else {
            format!("1/{} s", (1.0 / self.shutter).round() as i64)
        }
    }
}

/// Ein entwickeltes, aber **unbearbeitetes** Bild: linear, 16 Bit, Kamerafarbraum,
/// Weissabgleich der Kamera bereits angewandt.
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    /// RGB, verschraenkt (r,g,b,r,g,b …), 16 Bit, linear.
    pub pixels: Vec<u16>,
    pub meta: RawMeta,
}

impl RawImage {
    pub fn pixel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
    pub fn megapixel(&self) -> f32 {
        self.pixel_count() as f32 / 1_000_000.0
    }
}

fn strerror(code: c_int) -> String {
    unsafe {
        let p = libraw_strerror(code);
        if p.is_null() {
            format!("Fehler {code}")
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// Haelt die OpenMP-Laufzeit von LibRaw fuer die ganze Programmdauer wach.
/// Einmal im Hauptfaden aufrufen, bevor dekodiert wird.
///
/// Die LLVM-Laufzeit faehrt herunter, sobald der letzte Faden endet, der sie
/// benutzt hat — dekodiert wird aber in kurzlebigen Faeden. Danach zeigt die
/// zwischengespeicherte Sperre in LibRaws Speicherverwaltung
/// (`#pragma omp critical`) ins Leere, und der naechste Dekodier-Faden stuerzt
/// ab (gemessen: zweiter Faden, SIGSEGV in `__kmpc_critical_with_hint`).
/// Der Hauptfaden endet erst mit dem Programm; meldet er sich einmal an,
/// bleibt die Laufzeit stehen (nachgemessen: acht Lade-Faeden nacheinander).
pub fn openmp_festhalten() {
    #[cfg(mit_openmp)]
    unsafe {
        unsafe extern "C" {
            fn omp_get_max_threads() -> std::os::raw::c_int;
        }
        omp_get_max_threads();
    }
}

/// Liest eine RAW-Datei und entwickelt sie bis zum linearen Kamerabild.
pub fn decode(path: impl AsRef<Path>, opts: &DecodeOptions) -> Result<RawImage, RawError> {
    let path = path.as_ref();
    let c_path = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| RawError::Path(path.display().to_string()))?;

    unsafe {
        let lr = libraw_init(0);
        if lr.is_null() {
            return Err(RawError::OutOfMemory);
        }
        // Ab hier sorgt der Waechter dafuer, dass LibRaw auf jedem Rueckweg
        // aufgeraeumt wird — auch bei jedem `?`.
        let _guard = Guard(lr);

        let rc = libraw_open_file(lr, c_path.as_ptr());
        if rc != 0 {
            return Err(RawError::Open(strerror(rc)));
        }
        let rc = libraw_unpack(lr);
        if rc != 0 {
            return Err(RawError::Unpack(strerror(rc)));
        }

        // --- Entwicklungs-Vorgaben: so roh wie moeglich, so gut wie noetig --------------
        libraw_set_output_bps(lr, 16);
        // Linear bleiben (Gamma 1/1). Jede Tonwertkurve macht spaeter die GPU in float —
        // hier wuerde sie nur Praezision kosten.
        libraw_set_gamma(lr, 0, 1.0);
        libraw_set_gamma(lr, 1, 1.0);
        // Keine automatische Aufhellung: die Belichtung gehoert dem Nutzer, nicht LibRaw.
        libraw_set_no_auto_bright(lr, 1);
        // Farbraum 0 = Kamera. Die Matrix wenden wir selbst auf der GPU an.
        libraw_set_output_color(lr, 0);
        libraw_set_highlight(lr, opts.highlights.libraw_code());
        let mut qualitaet = opts.demosaic.libraw_code();
        if let Some(q) = QUALITAET_ERZWINGEN.with(|q| q.get()) {
            qualitaet = q as c_int;
        }
        libraw_set_demosaic(lr, qualitaet);
        // Gemessen und VERWORFEN: LibRaws Median-Paesse (-m 3, fr_set_med_passes)
        // gegen Farbpixel des 6x6-Musters an Kanten kosteten 3,7 s -> 13,7 s je
        // 40-MP-Foto und brachten sichtbar nichts — die Fleckenbasierte
        // Farbrauschminderung auf der GPU (output.wgsl) raeumt dieselben
        // Pixel bereits weg. Wer es je wieder erwaegt: erst 100-%-Ausschnitte
        // vergleichen, der Shim-Setter existiert noch.
        // LibRaw senkt den Weisspunkt sonst heuristisch (Standard 0.75) — wir wollen
        // vorhersagbares Verhalten und regeln Lichter selbst.
        libraw_set_adjust_maximum_thr(lr, 0.0);
        libraw_set_half_size(lr, if opts.half_size { 1 } else { 0 });
        // Drehung gehoert UNS: LibRaw wuerde die Kameralage sonst selbst
        // anwenden UND sie in sizes.flip weitermelden — Miniatur und Editor
        // drehten dann ein zweites Mal (Hochformat lag wieder quer).
        // ⚠️ Die Kameralage VOR dem Entwickeln sichern: user_flip = 0
        // ueberschreibt sizes.flip beim Verarbeiten.
        let flip_kamera = libraw_get_flip(lr);
        libraw_set_user_flip(lr, 0);

        // Weissabgleich der Kamera explizit setzen (statt use_camera_wb zu erraten):
        // vier Multiplikatoren, die LibRaw beim Entpacken aus der Datei gelesen hat.
        let mut mul = [0f32; 4];
        for (i, m) in mul.iter_mut().enumerate() {
            *m = libraw_get_cam_mul(lr, i as c_int);
        }
        // Manche Dateien liefern keinen brauchbaren Wert — dann nimmt LibRaw die
        // Tageslicht-Referenz der Kamera, und wir merken es wenigstens.
        if !(mul[0] > 0.0 && mul[1] > 0.0) {
            for (i, m) in mul.iter_mut().enumerate() {
                *m = libraw_get_pre_mul(lr, i as c_int);
            }
        }
        if mul[3] == 0.0 {
            mul[3] = mul[1]; // zweites Gruen wie das erste
        }
        // Den vierten Faktor nur bei echten Vier-Farben-Sensoren setzen. Bei
        // RGB-Daten ohne Mosaik (verkleinerte S/M-RAWs und sRAWs mancher
        // Hersteller, lineare DNGs) uebernimmt LibRaw fuer R/G/B nicht unsere
        // Faktoren, sondern 1 — den vierten aber schon. Mit „Lichter mischen"
        // normiert LibRaw dann auf diesen einen grossen Faktor, und das ganze
        // Bild landet bei 72 von 65535 Stufen (gefunden an einem S-RAW-Testmuster).
        let farben = libraw_get_colors(lr);
        let ohne_mosaik = libraw_get_filters(lr) == 0;
        for (i, m) in mul.iter().enumerate() {
            let wert = if i == 3 && farben != 4 { 0.0 } else { *m };
            libraw_set_user_mul(lr, i as c_int, wert as c_float);
        }

        let rc = libraw_dcraw_process(lr);
        if rc != 0 {
            return Err(RawError::Process(strerror(rc)));
        }

        // Welche Faktoren hat LibRaw WIRKLICH angewandt? Nach dem Entwickeln
        // stehen sie normiert in pre_mul: Pixel = Sensorwert (Saettigung 1) ·
        // Faktor. Nicht annehmen, nachlesen — bei Daten ohne Mosaik weichen sie
        // von unseren ab (siehe oben), und die Farbkette muss genau diese kennen.
        let mut angewandt = [0f32; 4];
        for (i, a) in angewandt.iter_mut().enumerate() {
            *a = libraw_get_pre_mul(lr, i as c_int);
        }
        if farben != 4 {
            angewandt[3] = angewandt[1];
        }
        // Daten ohne Mosaik, bei denen LibRaw alle Faktoren auf 1 laesst
        // (S/M-RAW bzw. sRAW mancher Hersteller): die Kamera hat sie schon mit
        // dem Aufnahme-Weissabgleich multipliziert. Gemessen am S-RAW-Testmuster:
        // als „unabgeglichen" behandelt, wurde das Bild knallig magenta.
        let gleich = |a: f32, b: f32| (a - b).abs() <= 1e-4 * b.abs().max(1e-6);
        let aufnahme_neutral = gleich(mul[0], mul[1]) && gleich(mul[2], mul[1]);
        if ohne_mosaik
            && gleich(angewandt[0], angewandt[1])
            && gleich(angewandt[2], angewandt[1])
            && !aufnahme_neutral
        {
            let m_min = mul[..3].iter().copied().fold(f32::MAX, f32::min).max(1e-6);
            for i in 0..4 {
                angewandt[i] = mul[i] / m_min;
            }
        }
        if !angewandt[..3].iter().all(|a| a.is_finite() && *a > 1e-6) {
            // Rueckfall: LibRaws Normierung nachrechnen (Abschneiden = auf den
            // kleinsten, sonst auf den groessten Faktor).
            let m_min = mul[..3].iter().copied().fold(f32::MAX, f32::min).max(1e-6);
            let m_max = mul[..3].iter().copied().fold(0.0f32, f32::max).max(1e-6);
            let bezug = if opts.highlights == Highlights::Abschneiden { m_min } else { m_max };
            for i in 0..4 {
                angewandt[i] = mul[i] / bezug;
            }
        }
        // Pegel des neutralen Sensor-Weiss in Pixeln: Weiss der Aufnahme
        // (Neutral = kleinster Faktor / Faktor) mal angewandtem Faktor; der
        // groesste Kanal zaehlt. Bei normalen Bayer-Dateien ist das der
        // bekannte Wert kleinster/groesster Faktor (beim Abschneiden 1).
        let m_min = mul[..3].iter().copied().filter(|m| *m > 0.0).fold(f32::MAX, f32::min);
        let weiss_pegel = (0..3)
            .map(|c| angewandt[c] * m_min / mul[c].max(1e-6))
            .fold(0.0f32, f32::max)
            .clamp(0.05, 1.0);

        let mut err: c_int = 0;
        let img = libraw_dcraw_make_mem_image(lr, &mut err);
        if img.is_null() {
            return Err(RawError::NoImage(err));
        }
        let _img_guard = ImgGuard(img);

        let ip = &*img;
        if ip.bits != 16 || ip.colors != 3 {
            return Err(RawError::Unexpected {
                bits: ip.bits,
                colors: ip.colors,
            });
        }

        let n_u16 = (ip.data_size as usize) / 2;
        let src = ip.data.as_ptr() as *const u16;
        let mut pixels = vec![0u16; n_u16];
        std::ptr::copy_nonoverlapping(src, pixels.as_mut_ptr(), n_u16);

        // Metadaten ueber den C-Anschluss holen (Feld-Layout kennt nur der C-Compiler).
        let mut raw_meta = FrMeta::default();
        fr_fill_meta(lr, &mut raw_meta as *mut FrMeta);

        let mut cam_to_xyz = [[0f32; 3]; 3];
        for (r, row) in cam_to_xyz.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = raw_meta.cam_xyz[r * 3 + c];
            }
        }

        let meta = RawMeta {
            make: cstr_field(&raw_meta.make),
            model: cstr_field(&raw_meta.model),
            muster_6x6: raw_meta.muster_6x6 != 0,
            cam_to_xyz,
            cam_mul: raw_meta.cam_mul,
            pre_mul: raw_meta.pre_mul,
            black: raw_meta.black,
            cblack: raw_meta.cblack,
            maximum: raw_meta.maximum,
            iso: raw_meta.iso,
            shutter: raw_meta.shutter,
            aperture: raw_meta.aperture,
            focal_len: raw_meta.focal_len,
            flip: flip_kamera,
            timestamp: raw_meta.timestamp,
            lens: cstr_field(&raw_meta.lens),
            artist: cstr_field(&raw_meta.artist),
            // Nur bei DNGs suchen — andere RAW-Formate tragen keine Opcodes,
            // und der TIFF-Leser soll gar nicht erst raten muessen.
            dng_warp: {
                let p: &Path = path.as_ref();
                if p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
                {
                    dng::lese_opcodes(p)
                } else {
                    None
                }
            },
            dng_profil: {
                let p: &Path = path.as_ref();
                if p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
                {
                    dng::lese_profil(p)
                } else {
                    None
                }
            },
            datei_objektiv: {
                let p: &Path = path.as_ref();
                if p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("raf"))
                {
                    raf::lesen(p)
                } else {
                    None
                }
            },
            brennweite_kb: if raw_meta.focal_35mm.is_finite() { raw_meta.focal_35mm.max(0.0) } else { 0.0 },
            fokus_m: if raw_meta.focus_distance.is_finite() { raw_meta.focus_distance.max(0.0) } else { 0.0 },
            wb_mul: angewandt,
            wb_aufnahme: mul,
            weiss_pegel,
            belichtungsversatz: if raw_meta.expo_versatz.is_finite() && raw_meta.expo_versatz.abs() < 10.0 {
                raw_meta.expo_versatz
            } else {
                0.0
            },
            dynamikbereich: raw_meta.dynamikbereich,
            // LibRaw meldet „nicht gesetzt" als −999 — nur echte Werte zaehlen.
            grundbelichtung_dng: if raw_meta.dng_baseline_exposure.is_finite()
                && raw_meta.dng_baseline_exposure.abs() < 20.0
            {
                raw_meta.dng_baseline_exposure
            } else {
                0.0
            },
        };

        Ok(RawImage {
            width: ip.width as u32,
            height: ip.height as u32,
            pixels,
            meta,
        })
    }
}

/// Nur fuer Messsonden: mit einem bestimmten LibRaw-Demosaic-Code dekodieren.
pub fn decode_mit_qualitaet(path: impl AsRef<Path>, code: i32) -> Result<RawImage, RawError> {
    QUALITAET_ERZWINGEN.with(|q| q.set(Some(code)));
    let r = decode(path, &DecodeOptions::default());
    QUALITAET_ERZWINGEN.with(|q| q.set(None));
    r
}

thread_local! {
    static QUALITAET_ERZWINGEN: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// Die eingebettete Kamera-Vorschau (RAF: ein grosses JPEG) roh herausholen —
/// ohne das RAW zu entpacken oder zu demosaicen. Fuer die Galerie: eine Miniatur so
/// zu gewinnen kostet Millisekunden statt ein bis zwei Sekunden pro Foto.
///
/// Der Look ist der Kamera-Look (Filmsimulation), nicht die Fotowache-Entwicklung —
/// fuer eine Browsing-Vorschau genau richtig, und sobald ein Foto geoeffnet/entwickelt
/// wird, ersetzt die grosse Ansicht die Miniatur ohnehin durch den Fotowache-Look.
///
/// `flip` ist die Kameralage (sizes.flip), damit der Aufrufer die Miniatur genauso
/// dreht wie die grosse Ansicht (`drehung_aus_kamera`). Gibt `None` zurueck, wenn es
/// keine JPEG-Vorschau gibt (dann bleibt nur der volle Dekodier-Weg).
pub struct EmbeddedThumb {
    pub jpeg: Vec<u8>,
    pub flip: i32,
}

pub fn embedded_thumb(path: impl AsRef<Path>) -> Option<EmbeddedThumb> {
    let path = path.as_ref();
    let c_path = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    unsafe {
        let lr = libraw_init(0);
        if lr.is_null() {
            return None;
        }
        let _guard = Guard(lr);
        if libraw_open_file(lr, c_path.as_ptr()) != 0 {
            return None;
        }
        let mut format: c_int = 0;
        let mut length: u32 = 0;
        let mut tw: c_int = 0;
        let mut th: c_int = 0;
        let mut flip: c_int = 0;
        if fr_thumb_unpack(lr, &mut format, &mut length, &mut tw, &mut th, &mut flip) != 0 {
            return None;
        }
        // 1 = LIBRAW_THUMBNAIL_JPEG. Nur das nehmen wir — Bitmap-Vorschauen sind
        // selten und muessten anders dekodiert werden; dann lieber der volle Weg.
        if format != 1 || length == 0 {
            return None;
        }
        let mut jpeg = vec![0u8; length as usize];
        fr_thumb_copy(lr, jpeg.as_mut_ptr());
        Some(EmbeddedThumb { jpeg, flip })
    }
}

fn cstr_field(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

struct Guard(*mut c_void);
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { libraw_close(self.0) }
    }
}
struct ImgGuard(*mut LibrawProcessedImage);
impl Drop for ImgGuard {
    fn drop(&mut self) {
        unsafe { libraw_dcraw_clear_mem(self.0) }
    }
}

/// Version der eingebundenen LibRaw — gehoert in jeden Fehlerbericht.
pub fn libraw_version() -> String {
    unsafe {
        let p = ffi::libraw_version();
        if p.is_null() {
            "unbekannt".into()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// Liste der Endungen, die wir als RAW behandeln.
pub const RAW_EXTENSIONS: &[&str] = &[
    "raf", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "dng", "orf", "rw2", "raw",
    "pef", "iiq", "3fr", "fff", "erf", "mos", "mrw", "x3f", "gpr",
];

pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[allow(dead_code)]
fn _assert_meta_layout() {
    // Sicherung gegen ein LibRaw-Update, das die Shim-Struktur veraendert:
    // beide Seiten muessen dieselbe Groesse sehen.
    unsafe {
        debug_assert_eq!(fr_meta_size(), std::mem::size_of::<FrMeta>() as c_uint as usize);
    }
}
