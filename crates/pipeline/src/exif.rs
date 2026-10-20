//! Aufnahmedaten und Farbprofil in die Exportdateien schreiben.
//!
//! ## Warum nicht einfach durchreichen
//! Ein RAW traegt seine EXIF-Daten in einer herstellereigenen Struktur — bei jedem
//! Hersteller anders aufgebaut. Man kann sie nicht als Block
//! herauskopieren und in ein JPEG legen: die Verweise darin zeigen auf Positionen
//! in der RAW-Datei und waeren danach falsch. Deshalb werden hier die Felder, die
//! zaehlen, **neu geschrieben**. Das Ergebnis ist regelkonform und enthaelt keine
//! Angaben, die nach der Entwicklung ohnehin nicht mehr stimmen (Sensorgroesse,
//! Weissabgleich-Rohwerte, Vorschaubilder).
//!
//! ## Was mitkommt
//! Kamera, Objektiv, Aufnahmezeitpunkt, Belichtungszeit, Blende, ISO, Brennweite,
//! Fotograf — plus der Hinweis, womit entwickelt wurde. Dazu in **jeder** Datei das
//! sRGB-Profil, damit kein Programm mehr raten muss.
//!
//! ## Warum eigener TIFF-Schreiber
//! Die uebliche Bildbibliothek schreibt 16-Bit-TIFF, kann aber weder Farbprofil noch
//! EXIF hineinlegen — und genau dafuer nimmt man ein 16-Bit-TIFF. Ein einfaches,
//! unkomprimiertes TIFF ist ueberschaubar genug, um es selbst zu schreiben und dabei
//! die volle Kontrolle zu behalten.

/// Alles, was aus der Aufnahme in die Datei soll.
#[derive(Debug, Clone, Default)]
pub struct Aufnahme {
    pub hersteller: String,
    pub modell: String,
    pub objektiv: String,
    pub fotograf: String,
    /// Unix-Sekunden, 0 = unbekannt.
    pub zeitpunkt: i64,
    /// Belichtungszeit in Sekunden.
    pub belichtung: f32,
    pub blende: f32,
    pub iso: f32,
    pub brennweite: f32,
}

impl Aufnahme {
    pub fn aus_meta(m: &rawcore::RawMeta) -> Self {
        Self {
            hersteller: m.make.clone(),
            modell: m.model.clone(),
            objektiv: m.lens.clone(),
            fotograf: m.artist.clone(),
            zeitpunkt: m.timestamp,
            belichtung: m.shutter,
            blende: m.aperture,
            iso: m.iso,
            brennweite: m.focal_len,
        }
    }
}

pub const SOFTWARE: &str = "Fotowache";

// ---------------------------------------------------------------------------
// TIFF-Bausteine (EXIF ist innen drin ein TIFF)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Wert {
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    Undefined(Vec<u8>),
}

impl Wert {
    fn typ(&self) -> u16 {
        match self {
            Wert::Ascii(_) => 2,
            Wert::Short(_) => 3,
            Wert::Long(_) => 4,
            Wert::Rational(_) => 5,
            Wert::Undefined(_) => 7,
        }
    }
    fn anzahl(&self) -> u32 {
        match self {
            Wert::Ascii(s) => s.len() as u32 + 1, // abschliessende Null zaehlt mit
            Wert::Short(v) => v.len() as u32,
            Wert::Long(v) => v.len() as u32,
            Wert::Rational(v) => v.len() as u32,
            Wert::Undefined(v) => v.len() as u32,
        }
    }
    fn bytes(&self) -> Vec<u8> {
        match self {
            Wert::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Wert::Short(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Wert::Long(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Wert::Rational(v) => v
                .iter()
                .flat_map(|(a, b)| {
                    let mut o = a.to_le_bytes().to_vec();
                    o.extend_from_slice(&b.to_le_bytes());
                    o
                })
                .collect(),
            Wert::Undefined(v) => v.clone(),
        }
    }
}

struct Ifd {
    eintraege: Vec<(u16, Wert)>,
}

impl Ifd {
    fn neu() -> Self {
        Self {
            eintraege: Vec::new(),
        }
    }
    fn setz(&mut self, tag: u16, w: Wert) {
        self.eintraege.push((tag, w));
    }
    /// Laenge des Verzeichnisses selbst (ohne die ausgelagerten Werte).
    fn kopf_len(&self) -> usize {
        2 + self.eintraege.len() * 12 + 4
    }
    /// Laenge der Werte, die nicht in die 4 Byte des Eintrags passen.
    fn daten_len(&self) -> usize {
        self.eintraege
            .iter()
            .map(|(_, w)| {
                let n = w.bytes().len();
                if n <= 4 {
                    0
                } else {
                    n + n % 2 // auf gerade Adressen auffuellen
                }
            })
            .sum()
    }
    /// Schreibt Verzeichnis und Werte. `basis` ist der Versatz, ab dem die
    /// TIFF-internen Zeiger zaehlen (Anfang des TIFF-Kopfes).
    fn schreib(&mut self, eigener_versatz: usize, basis: usize, naechstes: u32) -> Vec<u8> {
        // TIFF verlangt aufsteigend sortierte Tags. Wer das ignoriert, bekommt
        // Dateien, die viele Leseprogramme klaglos falsch interpretieren.
        self.eintraege.sort_by_key(|(t, _)| *t);
        let mut kopf = Vec::with_capacity(self.kopf_len());
        let mut daten = Vec::with_capacity(self.daten_len());
        let daten_start = eigener_versatz + self.kopf_len();

        kopf.extend_from_slice(&(self.eintraege.len() as u16).to_le_bytes());
        for (tag, w) in &self.eintraege {
            kopf.extend_from_slice(&tag.to_le_bytes());
            kopf.extend_from_slice(&w.typ().to_le_bytes());
            kopf.extend_from_slice(&w.anzahl().to_le_bytes());
            let b = w.bytes();
            if b.len() <= 4 {
                let mut feld = [0u8; 4];
                feld[..b.len()].copy_from_slice(&b);
                kopf.extend_from_slice(&feld);
            } else {
                let ziel = (daten_start + daten.len() - basis) as u32;
                kopf.extend_from_slice(&ziel.to_le_bytes());
                daten.extend_from_slice(&b);
                if b.len() % 2 == 1 {
                    daten.push(0);
                }
            }
        }
        kopf.extend_from_slice(&naechstes.to_le_bytes());
        kopf.extend_from_slice(&daten);
        kopf
    }
}

fn rational(v: f32) -> (u32, u32) {
    if !v.is_finite() || v <= 0.0 {
        return (0, 1);
    }
    if v >= 1.0 {
        ((v * 1000.0).round() as u32, 1000)
    } else {
        // Kurze Zeiten als „1/x" — so, wie sie auch auf der Kamera stehen.
        (1, (1.0 / v).round().max(1.0) as u32)
    }
}

/// Unix-Sekunden als „YYYY:MM:DD HH:MM:SS" (die Schreibweise, die EXIF vorschreibt).
///
/// Bewusst selbst gerechnet statt ueber eine Zeitbibliothek: EXIF kennt keine
/// Zeitzone, es will die **Ortszeit der Aufnahme**. Die steckt bei LibRaw bereits
/// als Sekundenwert drin, und eine Bibliothek wuerde ihn nur erneut umrechnen.
fn zeit_text(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    let tage = t.div_euclid(86_400);
    let rest = t.rem_euclid(86_400);
    let (h, mi, s) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    // Zivilkalender aus Tagen seit 1970 — Verfahren nach Howard Hinnant.
    let z = tage + 719_468;
    let aera = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + aera * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}:{m:02}:{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Baut den EXIF-Block, wie er in ein JPEG gehoert (ohne die „Exif\0\0"-Kennung).
pub fn exif_tiff(a: &Aufnahme, breite: u32, hoehe: u32) -> Vec<u8> {
    let zeit = zeit_text(a.zeitpunkt);

    let mut exif = Ifd::neu();
    if a.belichtung > 0.0 {
        exif.setz(0x829A, Wert::Rational(vec![rational(a.belichtung)]));
    }
    if a.blende > 0.0 {
        exif.setz(0x829D, Wert::Rational(vec![rational(a.blende)]));
    }
    if a.iso > 0.0 {
        exif.setz(0x8827, Wert::Short(vec![a.iso.min(65535.0) as u16]));
    }
    if !zeit.is_empty() {
        exif.setz(0x9003, Wert::Ascii(zeit.clone())); // DateTimeOriginal
        exif.setz(0x9004, Wert::Ascii(zeit.clone())); // DateTimeDigitized
    }
    if a.brennweite > 0.0 {
        exif.setz(0x920A, Wert::Rational(vec![rational(a.brennweite)]));
    }
    // ColorSpace = 1 (sRGB). Das ist die Angabe, auf die sich Kameras und Browser
    // stuetzen, wenn kein ICC-Profil da ist — wir liefern beides.
    exif.setz(0xA001, Wert::Short(vec![1]));
    exif.setz(0xA002, Wert::Long(vec![breite]));
    exif.setz(0xA003, Wert::Long(vec![hoehe]));
    if !a.objektiv.is_empty() {
        exif.setz(0xA434, Wert::Ascii(a.objektiv.clone()));
    }
    // Version des EXIF-Teils: „0230". Ohne sie halten manche Leser den Block fuer defekt.
    exif.setz(0x9000, Wert::Undefined(b"0230".to_vec()));

    let mut null = Ifd::neu();
    if !a.hersteller.is_empty() {
        null.setz(0x010F, Wert::Ascii(a.hersteller.clone()));
    }
    if !a.modell.is_empty() {
        null.setz(0x0110, Wert::Ascii(a.modell.clone()));
    }
    // Ausrichtung 1: Das Bild liegt bereits richtig herum — Drehung und Zuschnitt
    // sind beim Entwickeln in die Pixel eingerechnet worden. Eine Ausrichtungs-
    // angabe zusaetzlich waere doppelt gedreht.
    null.setz(0x0112, Wert::Short(vec![1]));
    null.setz(0x0131, Wert::Ascii(SOFTWARE.to_string()));
    if !zeit.is_empty() {
        null.setz(0x0132, Wert::Ascii(zeit));
    }
    if !a.fotograf.is_empty() {
        null.setz(0x013B, Wert::Ascii(a.fotograf.clone()));
    }
    null.setz(0x011A, Wert::Rational(vec![(72, 1)]));
    null.setz(0x011B, Wert::Rational(vec![(72, 1)]));
    null.setz(0x0128, Wert::Short(vec![2]));
    // Platzhalter fuer den Zeiger aufs Exif-Verzeichnis — wird gleich ersetzt.
    null.setz(0x8769, Wert::Long(vec![0]));

    // Aufbau: Kopf(8) | IFD0 | IFD0-Daten | Exif-IFD | Exif-Daten
    let ifd0_off = 8usize;
    let ifd0_gesamt = null.kopf_len() + null.daten_len();
    let exif_off = ifd0_off + ifd0_gesamt;

    // Zeiger richtigstellen und neu bauen.
    for (tag, w) in null.eintraege.iter_mut() {
        if *tag == 0x8769 {
            *w = Wert::Long(vec![exif_off as u32]);
        }
    }

    let mut out = Vec::with_capacity(exif_off + exif.kopf_len() + exif.daten_len());
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd0_off as u32).to_le_bytes());
    out.extend_from_slice(&null.schreib(ifd0_off, 0, 0));
    out.extend_from_slice(&exif.schreib(exif_off, 0, 0));
    out
}

// ---------------------------------------------------------------------------
// JPEG
// ---------------------------------------------------------------------------

/// Legt EXIF (APP1) und Farbprofil (APP2) in ein fertiges JPEG.
///
/// Beide Bloecke muessen unmittelbar hinter der Startmarke stehen und in dieser
/// Reihenfolge — steht APP1 hinter APP2, findet ein Teil der Leseprogramme die
/// Aufnahmedaten nicht mehr.
pub fn jpeg_mit_metadaten(jpeg: &[u8], exif: &[u8], icc: &[u8]) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut out = Vec::with_capacity(jpeg.len() + exif.len() + icc.len() + 64);
    out.extend_from_slice(&jpeg[..2]);

    if !exif.is_empty() {
        let inhalt_len = 6 + exif.len();
        if inhalt_len + 2 <= 0xFFFF {
            out.extend_from_slice(&[0xFF, 0xE1]);
            out.extend_from_slice(&((inhalt_len + 2) as u16).to_be_bytes());
            out.extend_from_slice(b"Exif\0\0");
            out.extend_from_slice(exif);
        }
    }

    if !icc.is_empty() {
        // Ein Profil darf laenger sein als ein JPEG-Segment (64 KB). Dafuer sieht
        // der Standard eine Stueckelung vor: jedes Stueck traegt seine Nummer und
        // die Gesamtzahl.
        let nutz = 0xFFFF - 2 - 12;
        let stuecke = icc.len().div_ceil(nutz).max(1);
        for (i, teil) in icc.chunks(nutz).enumerate() {
            out.extend_from_slice(&[0xFF, 0xE2]);
            out.extend_from_slice(&((teil.len() + 16) as u16).to_be_bytes());
            out.extend_from_slice(b"ICC_PROFILE\0");
            out.push((i + 1) as u8);
            out.push(stuecke as u8);
            out.extend_from_slice(teil);
        }
    }

    out.extend_from_slice(&jpeg[2..]);
    out
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

/// Haengt einem fertigen PNG das Farbprofil an (`iCCP`-Block).
///
/// Der Block muss **vor** den Bilddaten stehen; er wird deshalb direkt hinter den
/// Kopfblock gesetzt.
pub fn png_mit_icc(png: &[u8], icc: &[u8]) -> Vec<u8> {
    const SIGNATUR: usize = 8;
    if png.len() < SIGNATUR + 12 || icc.is_empty() {
        return png.to_vec();
    }
    // Laenge des IHDR-Blocks lesen und dahinter einfuegen.
    let ihdr_len = u32::from_be_bytes(png[SIGNATUR..SIGNATUR + 4].try_into().unwrap()) as usize;
    let einfuege_ab = SIGNATUR + 12 + ihdr_len;
    if einfuege_ab > png.len() {
        return png.to_vec();
    }

    let mut inhalt = Vec::with_capacity(icc.len() / 2 + 32);
    inhalt.extend_from_slice(b"sRGB"); // Profilname
    inhalt.push(0); // Nullbyte
    inhalt.push(0); // Kompressionsverfahren 0 = zlib
    inhalt.extend_from_slice(&zlib_pack(icc));

    let mut block = Vec::with_capacity(inhalt.len() + 12);
    block.extend_from_slice(&(inhalt.len() as u32).to_be_bytes());
    block.extend_from_slice(b"iCCP");
    block.extend_from_slice(&inhalt);
    let mut pruef = Vec::with_capacity(4 + inhalt.len());
    pruef.extend_from_slice(b"iCCP");
    pruef.extend_from_slice(&inhalt);
    block.extend_from_slice(&crc32(&pruef).to_be_bytes());

    let mut out = Vec::with_capacity(png.len() + block.len());
    out.extend_from_slice(&png[..einfuege_ab]);
    out.extend_from_slice(&block);
    out.extend_from_slice(&png[einfuege_ab..]);
    out
}

/// zlib-Rahmen um unkomprimierte („stored") Deflate-Bloecke.
///
/// Warum nicht wirklich komprimieren: das Profil ist rund 2 KB und wird genau
/// einmal je Datei geschrieben. Eine Kompressionsbibliothek nur dafuer ins
/// Programm zu holen, waere ein Abhaengigkeitsrisiko fuer eingesparte 1,5 KB.
/// „Stored" ist regelkonformes zlib und wird von jedem Leser verstanden.
fn zlib_pack(daten: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // zlib-Kopf, geringste Kompressionsstufe
    for (i, block) in daten.chunks(65_535).enumerate() {
        let letzter = (i + 1) * 65_535 >= daten.len();
        out.push(if letzter { 1 } else { 0 });
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(daten).to_be_bytes());
    out
}

fn adler32(d: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for x in d {
        a = (a + *x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for x in d {
        c ^= *x as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

// ---------------------------------------------------------------------------
// TIFF, 16 Bit
// ---------------------------------------------------------------------------

/// Schreibt ein unkomprimiertes 16-Bit-RGB-TIFF **mit** Farbprofil und
/// Aufnahmedaten. `px` ist verschraenkt (r,g,b,r,g,b …).
pub fn tiff16(px: &[u16], breite: u32, hoehe: u32, a: &Aufnahme, icc: &[u8]) -> Vec<u8> {
    let zeit = zeit_text(a.zeitpunkt);
    let bild_bytes = (breite as usize) * (hoehe as usize) * 6;

    let mut exif = Ifd::neu();
    if a.belichtung > 0.0 {
        exif.setz(0x829A, Wert::Rational(vec![rational(a.belichtung)]));
    }
    if a.blende > 0.0 {
        exif.setz(0x829D, Wert::Rational(vec![rational(a.blende)]));
    }
    if a.iso > 0.0 {
        exif.setz(0x8827, Wert::Short(vec![a.iso.min(65535.0) as u16]));
    }
    if !zeit.is_empty() {
        exif.setz(0x9003, Wert::Ascii(zeit.clone()));
    }
    if a.brennweite > 0.0 {
        exif.setz(0x920A, Wert::Rational(vec![rational(a.brennweite)]));
    }
    if !a.objektiv.is_empty() {
        exif.setz(0xA434, Wert::Ascii(a.objektiv.clone()));
    }
    exif.setz(0x9000, Wert::Undefined(b"0230".to_vec()));
    exif.setz(0xA001, Wert::Short(vec![1]));

    let mut null = Ifd::neu();
    null.setz(0x0100, Wert::Long(vec![breite]));
    null.setz(0x0101, Wert::Long(vec![hoehe]));
    null.setz(0x0102, Wert::Short(vec![16, 16, 16]));
    null.setz(0x0103, Wert::Short(vec![1])); // ohne Kompression
    null.setz(0x0106, Wert::Short(vec![2])); // RGB
    null.setz(0x0111, Wert::Long(vec![0])); // Platzhalter Bilddaten-Versatz
    null.setz(0x0115, Wert::Short(vec![3]));
    null.setz(0x0116, Wert::Long(vec![hoehe])); // alles in einem Streifen
    null.setz(0x0117, Wert::Long(vec![bild_bytes as u32]));
    null.setz(0x011A, Wert::Rational(vec![(72, 1)]));
    null.setz(0x011B, Wert::Rational(vec![(72, 1)]));
    null.setz(0x0128, Wert::Short(vec![2]));
    null.setz(0x0112, Wert::Short(vec![1]));
    null.setz(0x0131, Wert::Ascii(SOFTWARE.to_string()));
    null.setz(0x0153, Wert::Short(vec![1, 1, 1])); // Ganzzahlen ohne Vorzeichen
    if !a.hersteller.is_empty() {
        null.setz(0x010F, Wert::Ascii(a.hersteller.clone()));
    }
    if !a.modell.is_empty() {
        null.setz(0x0110, Wert::Ascii(a.modell.clone()));
    }
    if !zeit.is_empty() {
        null.setz(0x0132, Wert::Ascii(zeit));
    }
    if !a.fotograf.is_empty() {
        null.setz(0x013B, Wert::Ascii(a.fotograf.clone()));
    }
    if !icc.is_empty() {
        null.setz(0x8773, Wert::Undefined(icc.to_vec()));
    }
    null.setz(0x8769, Wert::Long(vec![0])); // Platzhalter Exif-Verzeichnis

    let ifd0_off = 8usize;
    let exif_off = ifd0_off + null.kopf_len() + null.daten_len();
    let bild_off = exif_off + exif.kopf_len() + exif.daten_len();
    for (tag, w) in null.eintraege.iter_mut() {
        match *tag {
            0x0111 => *w = Wert::Long(vec![bild_off as u32]),
            0x8769 => *w = Wert::Long(vec![exif_off as u32]),
            _ => {}
        }
    }

    let mut out = Vec::with_capacity(bild_off + bild_bytes);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd0_off as u32).to_le_bytes());
    out.extend_from_slice(&null.schreib(ifd0_off, 0, 0));
    out.extend_from_slice(&exif.schreib(exif_off, 0, 0));
    debug_assert_eq!(out.len(), bild_off, "Bilddaten stehen nicht dort, wo der Zeiger hinweist");
    for v in px {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Aufnahme {
        Aufnahme {
            hersteller: "HERSTELLER".into(),
            modell: "TESTKAMERA".into(),
            objektiv: "16-80mm F4 OIS".into(),
            fotograf: "Test".into(),
            zeitpunkt: 1_753_000_000,
            belichtung: 1.0 / 250.0,
            blende: 5.6,
            iso: 400.0,
            brennweite: 35.0,
        }
    }

    #[test]
    fn zeitumrechnung_stimmt() {
        // Gegenprobe mit bekannten Werten.
        assert_eq!(zeit_text(0), "");
        assert_eq!(zeit_text(1), "1970:01:01 00:00:01");
        assert_eq!(zeit_text(1_700_000_000), "2023:11:14 22:13:20");
    }

    #[test]
    fn belichtungszeit_als_bruch() {
        assert_eq!(rational(1.0 / 250.0), (1, 250));
        assert_eq!(rational(2.0), (2000, 1000));
    }

    /// Der Block muss als TIFF lesbar sein und die Zeiger muessen innerhalb liegen.
    #[test]
    fn exif_block_ist_wohlgeformt() {
        let b = exif_tiff(&probe(), 6000, 4000);
        assert_eq!(&b[0..2], b"II");
        assert_eq!(u16::from_le_bytes(b[2..4].try_into().unwrap()), 42);
        let ifd0 = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
        let n = u16::from_le_bytes(b[ifd0..ifd0 + 2].try_into().unwrap()) as usize;
        let mut exif_zeiger = 0usize;
        let mut voriger_tag = 0u16;
        for i in 0..n {
            let e = ifd0 + 2 + i * 12;
            let tag = u16::from_le_bytes(b[e..e + 2].try_into().unwrap());
            assert!(tag > voriger_tag, "Tags nicht aufsteigend: {tag:#06x}");
            voriger_tag = tag;
            if tag == 0x8769 {
                exif_zeiger = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
            }
        }
        assert!(exif_zeiger > 0 && exif_zeiger < b.len(), "Exif-Zeiger zeigt ins Leere");
        let m = u16::from_le_bytes(b[exif_zeiger..exif_zeiger + 2].try_into().unwrap()) as usize;
        assert!(m >= 8, "zu wenige Exif-Felder: {m}");
        assert!(exif_zeiger + 2 + m * 12 + 4 <= b.len());
    }

    #[test]
    fn jpeg_bekommt_beide_bloecke() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x04, 0, 0, 0xFF, 0xD9];
        let exif = exif_tiff(&probe(), 100, 100);
        let icc = crate::icc::srgb_profil();
        let out = jpeg_mit_metadaten(&jpeg, &exif, &icc);
        assert_eq!(&out[0..2], &[0xFF, 0xD8]);
        assert_eq!(&out[2..4], &[0xFF, 0xE1], "APP1 muss direkt folgen");
        assert_eq!(&out[6..12], b"Exif\0\0");
        // APP2 mit dem Profil muss vorkommen …
        let pos = out
            .windows(12)
            .position(|w| w == b"ICC_PROFILE\0")
            .expect("kein ICC-Segment");
        // … und hinter APP1 liegen.
        assert!(pos > 12);
        // Die Bilddaten muessen unveraendert am Ende stehen.
        assert_eq!(&out[out.len() - 8..], &jpeg[2..]);
    }

    #[test]
    fn png_bekommt_iccp_vor_den_bilddaten() {
        // Minimales PNG: Signatur + IHDR + IDAT + IEND (Inhalte egal).
        let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let ihdr = b"IHDR".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&[0u8; 4]);
        let ab_idat = png.len();
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0u8; 4]);

        let out = png_mit_icc(&png, &crate::icc::srgb_profil());
        let iccp = out.windows(4).position(|w| w == b"iCCP").unwrap();
        let idat = out.windows(4).position(|w| w == b"IDAT").unwrap();
        assert!(iccp < idat, "iCCP muss vor IDAT stehen");
        assert!(iccp > ab_idat - 8, "iCCP muss hinter IHDR stehen");
    }

    /// Der zlib-Rahmen muss von einem echten Entpacker gelesen werden koennen —
    /// dafuer pruefen wir Kopf, Endeflag und Pruefsumme.
    #[test]
    fn zlib_rahmen_ist_gueltig() {
        let daten: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let z = zlib_pack(&daten);
        assert_eq!(&z[0..2], &[0x78, 0x01]);
        assert_eq!(z[2], 1, "letzter Block muss markiert sein");
        let len = u16::from_le_bytes([z[3], z[4]]) as usize;
        assert_eq!(len, daten.len());
        assert_eq!(u16::from_le_bytes([z[5], z[6]]), !(len as u16));
        assert_eq!(&z[7..7 + len], &daten[..]);
        let pruef = u32::from_be_bytes(z[z.len() - 4..].try_into().unwrap());
        assert_eq!(pruef, adler32(&daten));
    }

    /// Und das Wichtigste beim TIFF: der Zeiger auf die Bilddaten muss stimmen,
    /// sonst liest jedes Programm Muell — und zwar ohne Fehlermeldung.
    #[test]
    fn tiff_zeigt_auf_die_richtigen_bilddaten() {
        let (w, h) = (7u32, 5u32);
        let px: Vec<u16> = (0..(w * h * 3)).map(|i| (i * 1000) as u16).collect();
        let t = tiff16(&px, w, h, &probe(), &crate::icc::srgb_profil());

        let ifd0 = u32::from_le_bytes(t[4..8].try_into().unwrap()) as usize;
        let n = u16::from_le_bytes(t[ifd0..ifd0 + 2].try_into().unwrap()) as usize;
        let mut off = 0usize;
        let mut anzahl_bytes = 0usize;
        for i in 0..n {
            let e = ifd0 + 2 + i * 12;
            let tag = u16::from_le_bytes(t[e..e + 2].try_into().unwrap());
            let v = u32::from_le_bytes(t[e + 8..e + 12].try_into().unwrap()) as usize;
            if tag == 0x0111 {
                off = v;
            }
            if tag == 0x0117 {
                anzahl_bytes = v;
            }
        }
        assert_eq!(anzahl_bytes, (w * h * 3 * 2) as usize);
        assert_eq!(off + anzahl_bytes, t.len(), "Bilddaten muessen die Datei abschliessen");
        let erstes = u16::from_le_bytes(t[off..off + 2].try_into().unwrap());
        let letztes = u16::from_le_bytes(t[t.len() - 2..].try_into().unwrap());
        assert_eq!(erstes, px[0]);
        assert_eq!(letztes, *px.last().unwrap());
    }
}
