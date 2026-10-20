//! DNG-Opcode-Leser — die Objektivkorrektur, die der Hersteller SELBST in die
//! Datei schreibt.
//!
//! Ein grosser Telefonhersteller legt in jede DNG seiner Telefone (lineare
//! Telefon-DNG wie Bayer-DNGs von Drittanbieter-Apps) die exakte Korrektur der
//! Aufnahme: `WarpRectilinear` (Verzeichnung, Opcode 1) und `FixVignetteRadial`
//! (Randabdunklung, Opcode 3) in den Opcode-Listen (TIFF-Tags 51008/51009/51022).
//! Gaengige Raw-Entwickler lesen genau diese Daten — damit ist JEDES Modell
//! dieser Telefone abgedeckt, ohne eine Profiltabelle je Geraet zu pflegen.
//! LibRaw reicht die Opcodes nicht durch, deshalb dieser eigene, kleine TIFF-Leser.
//!
//! Formeln (DNG-Spezifikation 1.4, Kapitel Opcodes; identisch im dng_sdk):
//! - WarpRectilinear: r_quelle = r_ziel · (kr0 + kr1·r² + kr2·r⁴ + kr3·r⁶),
//!   dazu tangentiale Terme kt0/kt1; Radius normiert auf den WEITESTEN
//!   Eckabstand vom (mitgelieferten) Zentrum.
//! - FixVignetteRadial: Gewinn g = 1 + k0·r² + k1·r⁴ + k2·r⁶ + k3·r⁸ + k4·r¹⁰,
//!   das Bild wird zum Rand hin mit g multipliziert.
//!
//! ⚠️ Die Opcode-Daten selbst sind IMMER Big-Endian — unabhängig von der
//! Byte-Reihenfolge der TIFF-Datei drumherum.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Die Verzeichnungs-Koeffizienten des WarpRectilinear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpKoeff {
    /// kr0..kr3 (eine Ebene; bei Farbquerfehler-Ebenen wird die mittlere
    /// = Grün genommen).
    pub kr: [f64; 4],
    /// kt0, kt1 — tangentiale Terme (bei Telefonen praktisch 0).
    pub kt: [f64; 2],
    /// Optisches Zentrum, normiert 0..1 auf Breite/Höhe.
    pub mitte: [f64; 2],
}

/// Was die Datei ueber ihre eigene Objektivkorrektur weiss. Drei getrennte
/// Aussagen — sie kommen in freier Kombination vor:
/// - Lineare Telefon-DNG: gar keine Opcodes, aber `linear` (Korrektur steckt
///   SCHON in den Pixeln — nachgemessen am Muster eines Telefons von 2020) →
///   bloss nichts doppelt korrigieren!
/// - Bayer-DNGs von Aufnahme-Apps: oft nur `vignette`, Verzeichnung offen
///   (Muster eines Telefons von 2018) → Vignette aus der Datei, Verzeichnung
///   aus dem Profil.
/// - Volle Listen mit beidem (andere Hersteller).
#[derive(Debug, Clone, PartialEq)]
pub struct DngWarp {
    /// WarpRectilinear, falls die Datei einen traegt.
    pub warp: Option<WarpKoeff>, // Kennung: DNG-Opcode
    /// k0..k4 des FixVignetteRadial, falls vorhanden.
    pub vignette: Option<[f64; 5]>,
    /// Die Rohdaten sind ein fertig entwickeltes lineares Bild
    /// (PhotometricInterpretation = LinearRaw) — bei Telefon-DNGs heisst das:
    /// Verzeichnung und Randabdunklung sind bereits eingerechnet.
    pub linear: bool,
}

/// Liest die Objektiv-Opcodes einer DNG. `None`, wenn die Datei keine trägt
/// oder kein TIFF ist — das ist kein Fehler, nur „keine Herstellerdaten".
pub fn lese_opcodes(path: impl AsRef<Path>) -> Option<DngWarp> {
    let mut f = File::open(path).ok()?;
    let mut kopf = [0u8; 8];
    f.read_exact(&mut kopf).ok()?;
    let gross = match &kopf[0..2] {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let u16_at = |b: &[u8]| -> u16 {
        if gross {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    };
    let u32_at = |b: &[u8]| -> u32 {
        if gross {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    };
    if u16_at(&kopf[2..4]) != 42 {
        return None;
    }

    // Alle Verzeichnisse einsammeln: Hauptkette + Unterverzeichnisse (Tag 330).
    // Telefon-DNGs tragen die Opcodes im Unterverzeichnis mit den Rohdaten.
    let mut offen = vec![u32_at(&kopf[4..8]) as u64];
    let mut gesehen: Vec<u64> = Vec::new();
    let mut verzeichnung: Option<WarpKoeff> = None;
    let mut vignette: Option<[f64; 5]> = None;
    let mut linear = false;

    while let Some(off) = offen.pop() {
        if off == 0 || gesehen.contains(&off) || gesehen.len() > 32 {
            continue;
        }
        gesehen.push(off);
        if f.seek(SeekFrom::Start(off)).is_err() {
            continue;
        }
        let mut n2 = [0u8; 2];
        if f.read_exact(&mut n2).is_err() {
            continue;
        }
        let n = u16_at(&n2) as usize;
        if n > 512 {
            continue; // kein plausibles Verzeichnis
        }
        let mut eintraege = vec![0u8; n * 12 + 4];
        if f.read_exact(&mut eintraege).is_err() {
            continue;
        }
        // Ketten-Nachfolger
        offen.push(u32_at(&eintraege[n * 12..]) as u64);

        for e in eintraege[..n * 12].chunks_exact(12) {
            let tag = u16_at(&e[0..2]);
            let typ = u16_at(&e[2..4]);
            let anzahl = u32_at(&e[4..8]) as u64;
            match tag {
                // PhotometricInterpretation: 34892 = LinearRaw — die Rohdaten
                // sind ein fertig entwickeltes Bild (lineare Telefon-DNG). 32803 waere
                // ein klassisches Bayer-Mosaik.
                262 => {
                    let wert = if typ == 3 {
                        u16_at(&e[8..10]) as u32
                    } else {
                        u32_at(&e[8..12])
                    };
                    if wert == 34892 {
                        linear = true;
                    }
                }
                // SubIFDs: ein oder mehrere Verzeichnis-Offsets
                330 => {
                    if anzahl == 1 {
                        offen.push(u32_at(&e[8..12]) as u64);
                    } else if typ == 4 {
                        let ziel = u32_at(&e[8..12]) as u64;
                        let hier = f.stream_position().ok();
                        if f.seek(SeekFrom::Start(ziel)).is_ok() {
                            let mut b = vec![0u8; (anzahl.min(8) * 4) as usize];
                            if f.read_exact(&mut b).is_ok() {
                                for c in b.chunks_exact(4) {
                                    offen.push(u32_at(c) as u64);
                                }
                            }
                        }
                        if let Some(h) = hier {
                            let _ = f.seek(SeekFrom::Start(h));
                        }
                    }
                }
                // OpcodeList1/2/3
                51008 | 51009 | 51022 => {
                    if anzahl < 4 || anzahl > 16 * 1024 * 1024 {
                        continue;
                    }
                    let ziel = u32_at(&e[8..12]) as u64;
                    let hier = f.stream_position().ok();
                    if f.seek(SeekFrom::Start(ziel)).is_ok() {
                        let mut daten = vec![0u8; anzahl as usize];
                        if f.read_exact(&mut daten).is_ok() {
                            liste_lesen(&daten, &mut verzeichnung, &mut vignette);
                        }
                    }
                    if let Some(h) = hier {
                        let _ = f.seek(SeekFrom::Start(h));
                    }
                }
                _ => {}
            }
        }
    }

    if verzeichnung.is_none() && vignette.is_none() && !linear {
        return None;
    }
    Some(DngWarp {
        warp: verzeichnung, // Kennung: DNG-Opcode
        vignette,
        linear,
    })
}

/// Die Farbprofil-Tags, die eine DNG in IFD0 selbst mitbringt (DNG-Spezifikation,
/// Kapitel „Camera Profiles"): Matrizen fuer zwei Lichtarten, Vorwaertsmatrizen,
/// Farbtabellen, Grundkurve. Telefone, Kameras mit DNG als Rohformat, Drohnen …
/// legen hier ihr eigenes Profil ab — ohne diese Tags bliebe fuer sie nur eine
/// einzelne Matrix.
const PROFIL_TAGS: [u16; 18] = [
    0xC614, // UniqueCameraModel
    0xC6F8, // ProfileName
    0xC621, // ColorMatrix1
    0xC622, // ColorMatrix2
    0xC65A, // CalibrationIlluminant1
    0xC65B, // CalibrationIlluminant2
    0xC714, // ForwardMatrix1
    0xC715, // ForwardMatrix2
    0xC6F9, // ProfileHueSatMapDims
    0xC6FA, // ProfileHueSatMapData1
    0xC6FB, // ProfileHueSatMapData2
    0xC7A3, // ProfileHueSatMapEncoding
    0xC725, // ProfileLookTableDims
    0xC726, // ProfileLookTableData
    0xC7A4, // ProfileLookTableEncoding
    0xC6FC, // ProfileToneCurve
    0xC7A5, // BaselineExposureOffset
    0xC7A6, // DefaultBlackRender
];

fn typ_groesse(typ: u16) -> u64 {
    match typ {
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

/// Liest das eingebettete Kameraprofil einer DNG und gibt es als kleine,
/// eigenstaendige TIFF-Datei zurueck — genau so aufgebaut wie eine DCP-Datei
/// (ein Verzeichnis, dieselben Tags), damit der Profil-Leser der Farbkette es
/// unveraendert lesen kann. `None`, wenn die Datei keine DNG ist oder keine
/// ColorMatrix1 traegt.
pub fn lese_profil(path: impl AsRef<Path>) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let mut kopf = [0u8; 8];
    f.read_exact(&mut kopf).ok()?;
    let gross = match &kopf[0..2] {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let u16_at = |b: &[u8]| -> u16 {
        if gross {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    };
    let u32_at = |b: &[u8]| -> u32 {
        if gross {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    };
    if u16_at(&kopf[2..4]) != 42 {
        return None;
    }
    f.seek(SeekFrom::Start(u32_at(&kopf[4..8]) as u64)).ok()?;
    let mut n2 = [0u8; 2];
    f.read_exact(&mut n2).ok()?;
    let n = u16_at(&n2) as usize;
    if n == 0 || n > 1024 {
        return None;
    }
    let mut eintraege = vec![0u8; n * 12];
    f.read_exact(&mut eintraege).ok()?;

    let mut ist_dng = false;
    let mut gefunden: Vec<(u16, u16, u32, [u8; 4])> = Vec::new();
    for e in eintraege.chunks_exact(12) {
        let tag = u16_at(&e[0..2]);
        if tag == 0xC612 {
            ist_dng = true; // DNGVersion
        }
        if PROFIL_TAGS.contains(&tag) {
            gefunden.push((tag, u16_at(&e[2..4]), u32_at(&e[4..8]), [e[8], e[9], e[10], e[11]]));
        }
    }
    if !ist_dng || !gefunden.iter().any(|g| g.0 == 0xC621) {
        return None;
    }
    gefunden.sort_by_key(|g| g.0);

    // Neue Datei: Kopf, ein Verzeichnis, danach die Werte, die nicht in die
    // vier Byte des Eintrags passen. Byte-Reihenfolge wie im Original.
    let w16 = |v: u16| if gross { v.to_be_bytes() } else { v.to_le_bytes() };
    let w32 = |v: u32| if gross { v.to_be_bytes() } else { v.to_le_bytes() };
    let mut aus: Vec<u8> = Vec::new();
    aus.extend(if gross { *b"MM\0*" } else { *b"II*\0" });
    aus.extend(w32(8));
    let mut daten: Vec<u8> = Vec::new();
    let daten_start = 8 + 2 + gefunden.len() * 12 + 4;
    let mut verzeichnis: Vec<u8> = Vec::new();
    verzeichnis.extend(w16(gefunden.len() as u16));
    for (tag, typ, anzahl, wert) in &gefunden {
        let groesse = typ_groesse(*typ) * *anzahl as u64;
        let feld = if groesse <= 4 {
            *wert
        } else {
            // Farbtabellen sind einige 10 KB; mehr als 8 MB ist kaputt.
            if groesse > 8 * 1024 * 1024 {
                return None;
            }
            let quelle = u32_at(wert) as u64;
            let mut b = vec![0u8; groesse as usize];
            f.seek(SeekFrom::Start(quelle)).ok()?;
            f.read_exact(&mut b).ok()?;
            if daten.len() % 2 == 1 {
                daten.push(0);
            }
            let ziel = (daten_start + daten.len()) as u32;
            daten.extend(b);
            w32(ziel)
        };
        verzeichnis.extend(w16(*tag));
        verzeichnis.extend(w16(*typ));
        verzeichnis.extend(w32(*anzahl));
        verzeichnis.extend(feld);
    }
    verzeichnis.extend(w32(0));
    aus.extend(verzeichnis);
    aus.extend(daten);
    Some(aus)
}

/// Eine Opcode-Liste durchgehen. Aufbau (IMMER Big-Endian): Anzahl (u32),
/// dann je Opcode: Kennung (u32), DNG-Version (4 Byte), Flags (u32),
/// Parameterlänge in Byte (u32), Parameter.
fn liste_lesen(
    daten: &[u8],
    verzeichnung: &mut Option<WarpKoeff>,
    vignette: &mut Option<[f64; 5]>,
) {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let be_f64 = |b: &[u8]| {
        f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    };
    if daten.len() < 4 {
        return;
    }
    let anzahl = be32(&daten[0..4]) as usize;
    let mut p = 4usize;
    for _ in 0..anzahl.min(64) {
        if p + 16 > daten.len() {
            return;
        }
        let kennung = be32(&daten[p..p + 4]);
        let laenge = be32(&daten[p + 12..p + 16]) as usize;
        p += 16;
        if p + laenge > daten.len() {
            return;
        }
        let par = &daten[p..p + laenge];
        match kennung {
            // WarpRectilinear: Ebenenzahl, je Ebene 6 f64, dann Zentrum (2 f64)
            1 => {
                if par.len() >= 4 {
                    let ebenen = be32(&par[0..4]) as usize;
                    if ebenen >= 1 && par.len() >= 4 + ebenen * 48 + 16 {
                        // Bei 3 Ebenen (Farbquerfehler) die mittlere = Grün.
                        let wahl = if ebenen >= 3 { 1 } else { 0 };
                        let b = &par[4 + wahl * 48..];
                        let kr = [
                            be_f64(&b[0..8]),
                            be_f64(&b[8..16]),
                            be_f64(&b[16..24]),
                            be_f64(&b[24..32]),
                        ];
                        let kt = [be_f64(&b[32..40]), be_f64(&b[40..48])];
                        let m = &par[4 + ebenen * 48..];
                        let mitte = [be_f64(&m[0..8]), be_f64(&m[8..16])];
                        if kr.iter().all(|v| v.is_finite()) && mitte.iter().all(|v| v.is_finite()) {
                            *verzeichnung = Some(WarpKoeff { kr, kt, mitte });
                        }
                    }
                }
            }
            // FixVignetteRadial: k0..k4, cx, cy — je f64
            3 => {
                if par.len() >= 56 {
                    let k = [
                        be_f64(&par[0..8]),
                        be_f64(&par[8..16]),
                        be_f64(&par[16..24]),
                        be_f64(&par[24..32]),
                        be_f64(&par[32..40]),
                    ];
                    if k.iter().all(|v| v.is_finite()) {
                        *vignette = Some(k);
                    }
                }
            }
            _ => {}
        }
        p += laenge;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Eine kleine DNG-Attrappe bauen: TIFF-Kopf, ein Verzeichnis mit
    /// OpcodeList3, darin WarpRectilinear + FixVignetteRadial.
    fn attrappe(little_endian: bool) -> Vec<u8> {
        let mut liste: Vec<u8> = Vec::new();
        liste.extend(2u32.to_be_bytes()); // zwei Opcodes

        // WarpRectilinear, 1 Ebene
        let mut par: Vec<u8> = Vec::new();
        par.extend(1u32.to_be_bytes());
        for v in [1.02f64, -0.08, 0.03, -0.005, 0.0, 0.0] {
            par.extend(v.to_be_bytes());
        }
        for v in [0.5f64, 0.5] {
            par.extend(v.to_be_bytes());
        }
        liste.extend(1u32.to_be_bytes());
        liste.extend([1, 4, 0, 0]); // DNG-Version
        liste.extend(0u32.to_be_bytes()); // Flags
        liste.extend((par.len() as u32).to_be_bytes());
        liste.extend(&par);

        // FixVignetteRadial
        let mut par2: Vec<u8> = Vec::new();
        for v in [0.3f64, 0.1, 0.0, 0.0, 0.0, 0.5, 0.5] {
            par2.extend(v.to_be_bytes());
        }
        liste.extend(3u32.to_be_bytes());
        liste.extend([1, 3, 0, 0]);
        liste.extend(0u32.to_be_bytes());
        liste.extend((par2.len() as u32).to_be_bytes());
        liste.extend(&par2);

        let u16b = |v: u16| if little_endian { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if little_endian { v.to_le_bytes() } else { v.to_be_bytes() };

        let mut d: Vec<u8> = Vec::new();
        d.extend(if little_endian { *b"II" } else { *b"MM" });
        d.extend(u16b(42));
        d.extend(u32b(8)); // IFD0 direkt nach dem Kopf
        // IFD0: 1 Eintrag (OpcodeList3), Liste folgt nach dem Verzeichnis.
        let listen_off = 8 + 2 + 12 + 4;
        d.extend(u16b(1));
        d.extend(u16b(51022));
        d.extend(u16b(7)); // UNDEFINED
        d.extend(u32b(liste.len() as u32));
        d.extend(u32b(listen_off as u32));
        d.extend(u32b(0)); // keine weitere IFD
        d.extend(&liste);
        d
    }

    /// Beide Byte-Reihenfolgen der TIFF-Huelle — die Opcode-Daten selbst sind
    /// immer Big-Endian, genau das prueft der Wechsel.
    #[test]
    fn opcodes_werden_gelesen() {
        for le in [true, false] {
            let dir = std::env::temp_dir().join(format!("fotowache_dngtest_{le}.dng"));
            let mut f = File::create(&dir).unwrap();
            f.write_all(&attrappe(le)).unwrap();
            drop(f);
            let w = lese_opcodes(&dir).expect("Opcodes muessen gefunden werden");
            let _ = std::fs::remove_file(&dir);
            let vz = w.warp.expect("Verzeichnungs-Opcode muss ankommen"); // Kennung: DNG-Opcode
            assert!((vz.kr[0] - 1.02).abs() < 1e-12);
            assert!((vz.kr[1] + 0.08).abs() < 1e-12);
            assert_eq!(vz.mitte, [0.5, 0.5]);
            let v = w.vignette.expect("Vignette-Opcode muss ankommen");
            assert!((v[0] - 0.3).abs() < 1e-12);
            assert!(!w.linear, "Attrappe traegt kein LinearRaw");
        }
    }

    #[test]
    fn keine_opcodes_kein_fund() {
        let dir = std::env::temp_dir().join("fotowache_dngtest_leer.bin");
        std::fs::write(&dir, b"kein tiff").unwrap();
        assert!(lese_opcodes(&dir).is_none());
        let _ = std::fs::remove_file(&dir);
    }
}
