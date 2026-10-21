//! Lineare DNG schreiben — fuer zusammengesetzte Bilder (Panorama).
//!
//! Das Ergebnis eines Panoramas ist kein fertiges Foto, sondern wieder ein
//! Rohbild: entmosaikte, lineare Sensorwerte im Kamerafarbraum. Als lineare DNG
//! (PhotometricInterpretation „LinearRaw") gespeichert, liest es der Dekoder
//! wie jede andere Rohdatei — Weissabgleich, alle Regler, KI, Retusche und
//! Zuschnitt wirken darauf, und die Datei taucht in der Galerie auf.
//!
//! Geschrieben wird ein schlichtes TIFF (Little Endian, unkomprimiert, 16 Bit,
//! drei Kanaele, Streifen) mit den DNG-Feldern, die ein Leser fuer die Farbe
//! braucht: Farbmatrix (XYZ -> Kamera) unter D65, neutraler Weissabgleich der
//! Aufnahme, Schwarz 0, Weiss 65535.

use std::io::Write;
use std::path::Path;

/// Was ueber die Herkunft bekannt ist.
#[derive(Debug, Clone)]
pub struct Herkunft {
    pub make: String,
    pub model: String,
    /// Kamera-RGB -> XYZ (D65), wie `RawMeta::cam_to_xyz`.
    pub cam_to_xyz: [[f32; 3]; 3],
    /// Weissabgleich der Aufnahme als Faktoren (R, G, B), wie `wb_aufnahme`.
    pub wb: [f32; 3],
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub timestamp: i64,
}

/// Schreibt eine lineare DNG. `rgb` sind Sensorwerte (0..1 = 0..Saettigung),
/// verschraenkt RGB, `w * h * 3` Werte.
pub fn schreiben(pfad: &Path, w: u32, h: u32, rgb: &[f32], herkunft: &Herkunft) -> std::io::Result<()> {
    assert_eq!(rgb.len(), (w as usize) * (h as usize) * 3);
    let mut b = Baum::default();
    let zeilen_je_streifen: u32 = (65536 / (w.max(1) * 6)).clamp(1, h.max(1));
    let streifen = h.div_ceil(zeilen_je_streifen);
    let unique = format!("{} {}", herkunft.make, herkunft.model).trim().to_string();
    let farbmatrix = invertieren(herkunft.cam_to_xyz).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    // DNG erwartet die Matrix so normiert, dass D65-Weiss (XYZ) im hellsten
    // Kamerakanal 1 ergibt.
    let d65 = [0.9505f32, 1.0, 1.089];
    let kamera_weiss: Vec<f32> = (0..3).map(|i| (0..3).map(|j| farbmatrix[i][j] * d65[j]).sum()).collect();
    let norm = kamera_weiss.iter().cloned().fold(f32::MIN, f32::max).max(1e-6);
    let g = herkunft.wb[1].max(1e-6);
    let neutral = [g / herkunft.wb[0].max(1e-6), 1.0, g / herkunft.wb[2].max(1e-6)];

    b.kurz(254, &[0]); // NewSubFileType: Hauptbild
    b.lang(256, &[w]);
    b.lang(257, &[h]);
    b.kurz(258, &[16, 16, 16]);
    b.kurz(259, &[1]); // unkomprimiert
    b.kurz(262, &[34892]); // LinearRaw
    b.text(271, &herkunft.make);
    b.text(272, &herkunft.model);
    b.lang(273, &vec![0; streifen as usize]); // StripOffsets, unten gesetzt
    b.kurz(274, &[1]);
    b.kurz(277, &[3]);
    b.lang(278, &[zeilen_je_streifen]);
    b.lang(279, &(0..streifen).map(|i| zeilen_je_streifen.min(h - i * zeilen_je_streifen) * w * 6).collect::<Vec<_>>());
    b.kurz(284, &[1]); // verschraenkt
    b.text(305, "Fotowache");
    let zeit = exif_zeit(herkunft.timestamp);
    if let Some(z) = &zeit {
        b.text(306, z); // DateTime
    }
    b.bytes(50706, &[1, 4, 0, 0]); // DNGVersion
    b.bytes(50707, &[1, 1, 0, 0]); // DNGBackwardVersion
    b.text(50708, if unique.is_empty() { "Fotowache" } else { &unique });
    b.lang(50714, &[0]); // BlackLevel
    b.lang(50717, &[65535]); // WhiteLevel
    b.srational(50721, &farbmatrix.iter().flatten().map(|v| v / norm).collect::<Vec<_>>()); // ColorMatrix1
    b.rational(50728, &neutral); // AsShotNeutral
    b.kurz(50778, &[21]); // CalibrationIlluminant1 = D65
    // EXIF-Werte (eigenes Unterverzeichnis — im Hauptverzeichnis liest der
    // Dekoder sie nicht, gemessen), damit Statuszeile und Rauschminderung (ISO)
    // sie kennen.
    let mut exif = Baum::default();
    if herkunft.shutter > 0.0 {
        exif.rational(33434, &[herkunft.shutter]);
    }
    if herkunft.aperture > 0.0 {
        exif.rational(33437, &[herkunft.aperture]);
    }
    if herkunft.iso > 0.0 {
        exif.kurz(34855, &[herkunft.iso.round().clamp(1.0, 65535.0) as u16]);
    }
    if herkunft.focal_len > 0.0 {
        exif.rational(37386, &[herkunft.focal_len]);
    }
    if let Some(z) = &zeit {
        exif.text(36867, z); // DateTimeOriginal
        exif.text(36868, z); // DateTimeDigitized
    }
    b.lang(34665, &[0]); // ExifIFD, unten gesetzt

    let (mut kopf, streifen_feld, verweise) = b.bauen(0);
    let exif_start = kopf.len();
    let (exif_daten, _, _) = exif.bauen(exif_start);
    let exif_feld = verweise.iter().find(|(t, _)| *t == 34665).map(|(_, p)| *p).expect("ExifIFD-Feld");
    kopf[exif_feld..exif_feld + 4].copy_from_slice(&(exif_start as u32).to_le_bytes());
    kopf.extend_from_slice(&exif_daten);
    let mut f = std::io::BufWriter::new(std::fs::File::create(pfad)?);
    let daten_start = kopf.len() as u32;
    // StripOffsets eintragen.
    let mut ofs = daten_start;
    for i in 0..streifen {
        let p = streifen_feld + (i as usize) * 4;
        kopf[p..p + 4].copy_from_slice(&ofs.to_le_bytes());
        ofs += zeilen_je_streifen.min(h - i * zeilen_je_streifen) * w * 6;
    }
    f.write_all(&kopf)?;
    let mut zeile = Vec::with_capacity(w as usize * 6);
    for y in 0..h as usize {
        zeile.clear();
        for v in &rgb[y * w as usize * 3..(y + 1) * w as usize * 3] {
            zeile.extend_from_slice(&((v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_le_bytes());
        }
        f.write_all(&zeile)?;
    }
    f.flush()
}

fn invertieren(m: [[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if d.abs() < 1e-9 {
        return None;
    }
    let k = |a: usize, b: usize, c: usize, e: usize| m[a][b] * m[c][e] - m[a][e] * m[c][b];
    Some([
        [k(1, 1, 2, 2) / d, -k(0, 1, 2, 2) / d, k(0, 1, 1, 2) / d],
        [-k(1, 0, 2, 2) / d, k(0, 0, 2, 2) / d, -k(0, 0, 1, 2) / d],
        [k(1, 0, 2, 1) / d, -k(0, 0, 2, 1) / d, k(0, 0, 1, 1) / d],
    ])
}

/// Ein kleiner TIFF-IFD-Bauer (Little Endian).
#[derive(Default)]
struct Baum {
    eintraege: Vec<(u16, u16, u32, Vec<u8>)>, // Tag, Typ, Anzahl, Rohdaten
}

/// Aufnahmezeit im EXIF-Format „JJJJ:MM:TT hh:mm:ss". Der Dekoder liest dieses
/// Feld als Ortszeit (`mktime` der C-Laufzeit) — deshalb hier mit derselben
/// Ortszeit zurueckwandeln, sonst verschiebt die Rundreise um die Zeitzone.
fn exif_zeit(ts: i64) -> Option<String> {
    if ts <= 0 {
        return None;
    }
    let t = ortszeit(ts)?;
    Some(format!("{:04}:{:02}:{:02} {:02}:{:02}:{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec))
}

/// `struct tm` der C-Laufzeit; nur die neun gemeinsamen Felder werden gelesen,
/// der Rest haelt Platz fuer die systemeigenen Zusatzfelder.
#[repr(C)]
#[derive(Default)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    _rest: [u64; 4],
}

#[cfg(unix)]
fn ortszeit(ts: i64) -> Option<Tm> {
    extern "C" {
        fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm::default();
    // SAFETY: beide Zeiger zeigen auf gueltige, lange genug lebende Werte;
    // `Tm` ist groesser als jedes `struct tm` der unterstuetzten Systeme.
    let r = unsafe { localtime_r(&ts, &mut tm) };
    (!r.is_null()).then_some(tm)
}

#[cfg(windows)]
fn ortszeit(ts: i64) -> Option<Tm> {
    extern "C" {
        fn _localtime64_s(tm: *mut Tm, t: *const i64) -> i32;
    }
    let mut tm = Tm::default();
    // SAFETY: wie oben; die Laufzeit schreibt nur die neun Felder.
    let r = unsafe { _localtime64_s(&mut tm, &ts) };
    (r == 0).then_some(tm)
}

impl Baum {
    fn kurz(&mut self, tag: u16, v: &[u16]) {
        self.eintraege.push((tag, 3, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    }
    fn lang(&mut self, tag: u16, v: &[u32]) {
        self.eintraege.push((tag, 4, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect()));
    }
    fn bytes(&mut self, tag: u16, v: &[u8]) {
        self.eintraege.push((tag, 1, v.len() as u32, v.to_vec()));
    }
    fn text(&mut self, tag: u16, s: &str) {
        let mut v: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
        v.push(0);
        self.eintraege.push((tag, 2, v.len() as u32, v));
    }
    fn rational(&mut self, tag: u16, v: &[f32]) {
        let d = 1_000_000u32;
        let daten = v.iter().flat_map(|x| {
            let n = (x.max(0.0) * d as f32).round() as u32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        });
        self.eintraege.push((tag, 5, v.len() as u32, daten.collect()));
    }
    fn srational(&mut self, tag: u16, v: &[f32]) {
        let d = 1_000_000i32;
        let daten = v.iter().flat_map(|x| {
            let n = (x * d as f32).round() as i32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        });
        self.eintraege.push((tag, 10, v.len() as u32, daten.collect()));
    }

    /// Kopf + IFD + ausgelagerte Werte. `start` = 0 fuer das Hauptverzeichnis
    /// (mit Dateikopf), sonst die Dateiposition eines Unterverzeichnisses. Gibt
    /// ausserdem die Position der StripOffsets-Werte und je Tag die Position
    /// seines Wertefelds zurueck (zum spaeteren Eintragen).
    fn bauen(mut self, start: usize) -> (Vec<u8>, usize, Vec<(u16, usize)>) {
        self.eintraege.sort_by_key(|e| e.0);
        let n = self.eintraege.len();
        let ifd_start = if start == 0 { 8usize } else { start };
        let extra_start = ifd_start + 2 + n * 12 + 4;
        let mut kopf = Vec::new();
        if start == 0 {
            kopf.extend_from_slice(b"II");
            kopf.extend_from_slice(&42u16.to_le_bytes());
            kopf.extend_from_slice(&(ifd_start as u32).to_le_bytes());
        }
        let mut verweise = Vec::new();
        let mut ifd = Vec::new();
        ifd.extend_from_slice(&(n as u16).to_le_bytes());
        let mut extra: Vec<u8> = Vec::new();
        let mut streifen_feld = 0usize;
        for (tag, typ, anzahl, daten) in &self.eintraege {
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&typ.to_le_bytes());
            ifd.extend_from_slice(&anzahl.to_le_bytes());
            if daten.len() <= 4 {
                let mut v = daten.clone();
                v.resize(4, 0);
                if *tag == 273 {
                    streifen_feld = ifd_start + ifd.len();
                }
                verweise.push((*tag, ifd_start + ifd.len()));
                ifd.extend_from_slice(&v);
            } else {
                if extra.len() % 2 == 1 {
                    extra.push(0);
                }
                let ofs = extra_start + extra.len();
                if *tag == 273 {
                    streifen_feld = ofs;
                }
                ifd.extend_from_slice(&(ofs as u32).to_le_bytes());
                extra.extend_from_slice(daten);
            }
        }
        ifd.extend_from_slice(&0u32.to_le_bytes()); // kein weiteres IFD
        kopf.extend_from_slice(&ifd);
        kopf.extend_from_slice(&extra);
        if kopf.len() % 2 == 1 {
            kopf.push(0);
        }
        (kopf, streifen_feld, verweise)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_umkehren() {
        let m = [[0.6, 0.3, 0.1], [0.2, 0.7, 0.1], [0.0, 0.1, 0.9]];
        let i = invertieren(m).unwrap();
        for a in 0..3 {
            for b in 0..3 {
                let s: f32 = (0..3).map(|k| m[a][k] * i[k][b]).sum();
                assert!((s - if a == b { 1.0 } else { 0.0 }).abs() < 1e-5);
            }
        }
    }

    /// Rundreise: schreiben und mit dem eigenen Dekoder (LibRaw) wieder lesen.
    #[test]
    fn eine_geschriebene_dng_wird_wieder_gelesen() {
        let (w, h) = (96u32, 64u32);
        let mut rgb = vec![0f32; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                // Unterhalb der Saettigung nach dem Weissabgleich (R x2, B x1,5),
                // sonst rekonstruiert der Dekoder ausgefressene Lichter.
                rgb[i] = 0.05 + 0.3 * x as f32 / w as f32;
                rgb[i + 1] = 0.3;
                rgb[i + 2] = 0.05 + 0.3 * y as f32 / h as f32;
            }
        }
        let herkunft = Herkunft {
            make: "Testhersteller".into(),
            model: "Testkamera".into(),
            cam_to_xyz: [[0.6, 0.3, 0.1], [0.25, 0.7, 0.05], [0.02, 0.1, 0.95]],
            wb: [2.0, 1.0, 1.5],
            iso: 200.0,
            shutter: 0.01,
            aperture: 8.0,
            focal_len: 35.0,
            timestamp: 1_658_746_867,
        };
        let pfad = std::env::temp_dir().join(format!("fotowache_dng_rund_{}.dng", std::process::id()));
        schreiben(&pfad, w, h, &rgb, &herkunft).unwrap();
        let bild = crate::decode(&pfad, &crate::DecodeOptions::default());
        let _ = std::fs::remove_file(&pfad);
        let bild = bild.expect("eigene DNG lesbar");
        assert_eq!((bild.width, bild.height), (w, h));
        assert!((bild.meta.iso - 200.0).abs() < 1.0, "ISO {}", bild.meta.iso);
        assert_eq!(bild.meta.timestamp, 1_658_746_867, "Aufnahmezeit");
        // Weissabgleich der Aufnahme kommt als Verhaeltnis zurueck.
        let wb = bild.meta.wb_aufnahme;
        assert!(((wb[0] / wb[1]) - 2.0).abs() < 0.05 && ((wb[2] / wb[1]) - 1.5).abs() < 0.05, "{wb:?}");
        // Pixel: nach Abzug des angewandten Weissabgleichs die geschriebenen
        // Sensorwerte (Verhaeltnis R/G an zwei Stellen).
        let px = |x: u32, y: u32, c: usize| bild.pixels[((y * w + x) * 3) as usize + c] as f32 / bild.meta.wb_mul[c];
        let links = px(4, 32, 0) / px(4, 32, 1);
        let rechts = px(90, 32, 0) / px(90, 32, 1);
        let soll_l = rgb[((32 * w + 4) * 3) as usize] / 0.3;
        let soll_r = rgb[((32 * w + 90) * 3) as usize] / 0.3;
        assert!((links / soll_l - 1.0).abs() < 0.05, "{links} {soll_l}");
        assert!((rechts / soll_r - 1.0).abs() < 0.05, "{rechts} {soll_r}");
    }
}
