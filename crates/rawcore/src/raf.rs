//! RAF-Objektivkorrektur der Kamera, eingebettet in die Datei.
//!
//! Die Kamera schreibt fuer jede Aufnahme die Korrektur des angesetzten Objektivs
//! in die RAF — genau das, was die Referenz-Entwicklung als Objektivprofil
//! „Kameraeinstellungen" (eingebettet) anwendet und in jede XMP schreibt. Der Ort:
//! der CFA-Bereich beginnt mit einem kleinen TIFF (Kopf `II*\0`), dessen
//! erster Eintrag (0xF000) auf das Hersteller-IFD zeigt. Darin:
//!
//! - 0xF00B GeometricDistortionParams  — Verzeichnung in Prozent je Stuetzstelle
//! - 0xF00F ChromaticAberrationParams  — Farbsaum Rot/Blau als Massstab
//! - 0xF010 VignettingParams           — Helligkeit in Prozent der Mitte
//!
//! Stuetzstellen sind Radien relativ zur halben Bilddiagonale. Aufbau
//! (6x6-Sensoren der vierten/fuenften Generation, 19/29/19 Werte): [Kennwert,
//! 9 Radien, 9 Werte] bzw. fuer den Farbsaum [Kennwert, 9 Radien, 9 × Rot,
//! 9 × Blau, Kennwert]. So liest es auch ein verbreiteter quelloffener
//! Raw-Entwickler (`exif.cc`, „embedded metadata"). (Fruehere Suche nach 0x1500 ff.
//! war die falsche Stelle — daher galt die Testkamera (40 MP) lange als „ohne
//! Korrekturdaten".)

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct RafObjektiv {
    /// Radien der Stuetzstellen, Anteil der halben Bilddiagonale.
    pub knoten: Vec<f32>,
    /// Verzeichnung in Prozent je Stuetzstelle (negativ = Tonne).
    pub verzeichnung: Vec<f32>,
    /// Laterale Farbsaeume: Massstab-Abweichung von Rot und Blau gegen Gruen.
    pub ca_rot: Vec<f32>,
    pub ca_blau: Vec<f32>,
    /// Randhelligkeit in Prozent der Bildmitte (100 = keine Vignette).
    pub vignette: Vec<f32>,
}

fn u16_le(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn u32_le(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(at)?, *d.get(at + 1)?, *d.get(at + 2)?, *d.get(at + 3)?]))
}

/// Liest die Korrekturtabellen; `None`, wenn die Datei keine (oder ein
/// unbekanntes Format) traegt.
pub fn lesen(pfad: &Path) -> Option<RafObjektiv> {
    let mut f = std::fs::File::open(pfad).ok()?;
    let mut kopf = [0u8; 108];
    f.read_exact(&mut kopf).ok()?;
    if &kopf[..16] != b"FUJIFILMCCD-RAW " { // Kennung: Dateikopf der RAF
        return None;
    }
    let cfa = u32::from_be_bytes([kopf[100], kopf[101], kopf[102], kopf[103]]) as u64;
    if cfa == 0 {
        return None;
    }
    // Das Hersteller-IFD samt Tabellen liegt in den ersten Kilobytes des CFA-Bereichs.
    let mut d = vec![0u8; 16384];
    f.seek(SeekFrom::Start(cfa)).ok()?;
    let n = f.read(&mut d).ok()?;
    d.truncate(n);
    aus_cfa(&d)
}

/// Auswertung des CFA-Anfangs (getrennt, damit testbar).
pub fn aus_cfa(d: &[u8]) -> Option<RafObjektiv> {
    if d.get(..4)? != b"II*\0" {
        return None;
    }
    let ifd0 = u32_le(d, 4)? as usize;
    let n0 = u16_le(d, ifd0)? as usize;
    let mut hersteller_ifd = None;
    for i in 0..n0 {
        let e = ifd0 + 2 + i * 12;
        if u16_le(d, e)? == 0xF000 {
            hersteller_ifd = Some(u32_le(d, e + 8)? as usize);
        }
    }
    let ifd = hersteller_ifd?;
    let n = u16_le(d, ifd)? as usize;
    let mut tabellen: [Option<Vec<f32>>; 3] = [None, None, None];
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        let tag = u16_le(d, e)?;
        let typ = u16_le(d, e + 2)?;
        let anzahl = u32_le(d, e + 4)? as usize;
        let ort = u32_le(d, e + 8)? as usize;
        let slot = match tag {
            0xF00B => 0,
            0xF00F => 1,
            0xF010 => 2,
            _ => continue,
        };
        if typ != 10 || anzahl > 64 {
            continue; // SRATIONAL erwartet
        }
        let mut v = Vec::with_capacity(anzahl);
        for k in 0..anzahl {
            let z = u32_le(d, ort + k * 8)? as i32;
            let nn = u32_le(d, ort + k * 8 + 4)? as i32;
            v.push(if nn != 0 { z as f32 / nn as f32 } else { 0.0 });
        }
        tabellen[slot] = Some(v);
    }
    let [Some(dist), Some(ca), Some(vig)] = tabellen else {
        return None;
    };
    // Nur das gut belegte Format der vierten/fuenften Sensorgeneration (neun
    // Stuetzstellen).
    if dist.len() != 19 || ca.len() != 29 || vig.len() != 19 {
        return None;
    }
    let knoten: Vec<f32> = dist[1..10].to_vec();
    if knoten != ca[1..10] || knoten != vig[1..10] {
        return None; // die drei Tabellen muessen dieselben Radien haben
    }
    if !knoten.windows(2).all(|w| w[1] > w[0]) || knoten[0] <= 0.0 { // Kennung: Slice-Methode
        return None;
    }
    let pruef = |v: &[f32]| v.iter().all(|x| x.is_finite());
    let o = RafObjektiv {
        knoten,
        verzeichnung: dist[10..19].to_vec(),
        ca_rot: ca[10..19].to_vec(),
        ca_blau: ca[19..28].to_vec(),
        vignette: vig[10..19].to_vec(),
    };
    if !(pruef(&o.verzeichnung) && pruef(&o.ca_rot) && pruef(&o.ca_blau) && pruef(&o.vignette)) {
        return None;
    }
    // Unsinnige Werte (kaputte Datei) lieber ignorieren als das Bild verbiegen.
    if o.verzeichnung.iter().any(|x| x.abs() > 40.0)
        || o.vignette.iter().any(|x| *x < 10.0 || *x > 150.0)
        || o.ca_rot.iter().chain(&o.ca_blau).any(|x| x.abs() > 0.05)
    {
        return None;
    }
    Some(o)
}
