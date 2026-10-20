//! Monitorprofil erkennen und einrechnen — **Kalibrierung**.
//!
//! ## Warum das noetig ist
//! Bis hierher endete die Kette mit „→ sRGB". Das ist die richtige Annahme fuer eine
//! *Datei*, die man weitergibt — aber die falsche fuer den *Bildschirm*, auf dem
//! man die Bilder beurteilt. Ein Weitfarb-Bildschirm (P3-Farbraum) deckt einen
//! deutlich groesseren Farbbereich ab als sRGB. Schickt man sRGB-Zahlen ungefragt
//! dorthin, zeigt der Bildschirm sie in *seinem* Raum — jede Farbe wird zu kraeftig.
//! Man dreht dann die Saettigung herunter, bis es „stimmt", und die Datei ist
//! anschliessend auf jedem anderen Geraet flau. Genau diese Falle schliesst dieses
//! Modul.
//!
//! ## Was hier passiert
//! macOS haelt fuer jeden Bildschirm ein ICC-Profil bereit (kalibriert oder ab Werk).
//! Wir holen es ueber den Farbprofil-Dienst des Systems, lesen Primaerfarben und
//! Uebertragungskurve heraus und
//! bauen daraus:
//!   * die Matrix **Arbeitsraum → Bildschirm-RGB** (statt → sRGB), und
//!   * eine Umkehrtabelle der Bildschirmkurve (statt der sRGB-Formel).
//!
//! ## Was hier bewusst NICHT passiert
//! Der **Export** bleibt sRGB. Eine Datei traegt den Bildschirm ihres Erzeugers nicht
//! mit sich herum; sie muss auf fremden Geraeten stimmen. Das Monitorprofil wirkt
//! ausschliesslich auf die Bildschirmvorschau.
//!
//! ## Warum kein little-cms
//! Bildschirmprofile sind praktisch immer „matrix/shaper" (Matrix + drei Kurven) —
//! beide Profile des Testrechners sind es. Dieser Fall laesst sich in wenigen Dutzend
//! Zeilen exakt lesen und rechnet auf der Grafikkarte in einem Schritt. Eine
//! vollstaendige CMM waere fuer Tabellen-Profile noetig; erkennen wir so eines,
//! fallen wir sauber auf sRGB zurueck, statt zu raten.

use crate::color::{self, Mat3};

/// Ein gelesenes Bildschirmprofil in der Form, die die Grafikkarte braucht.
#[derive(Debug, Clone)]
pub struct MonitorProfil {
    /// Anzeigename aus dem Profil („Color LCD", der Modellname des Bildschirms …).
    pub name: String,
    /// Arbeitsraum (Rec. 2020, linear) → Bildschirm-RGB (linear).
    pub work_to_display: Mat3,
    /// Umkehrkurve je Kanal: linear → Bildschirmwert, 1024 Stuetzstellen.
    pub inverse_trc: [Vec<f32>; 3],
    /// Wie weit der Bildschirm ueber sRGB hinausgeht (grobes Flaechenverhaeltnis).
    /// Nur zur Anzeige — damit sichtbar ist, dass die Erkennung etwas bewirkt hat.
    pub gamut_vs_srgb: f32,
}

impl MonitorProfil {
    /// Die Kurventabellen flach hintereinander — genau so, wie der Shader sie liest.
    pub fn trc_flat(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(3 * TRC_LEN);
        for ch in &self.inverse_trc {
            v.extend_from_slice(ch);
        }
        v
    }
}

pub const TRC_LEN: usize = 1024;

/// Das Profil des Hauptbildschirms. `None`, wenn es keins gibt oder es ein
/// Tabellenprofil ist, das wir nicht ehrlich rechnen koennen.
pub fn hauptbildschirm() -> Option<MonitorProfil> {
    let bytes = plattform::profil_bytes()?;
    parse_icc(&bytes)
}

// ---------------------------------------------------------------------------
// ICC lesen
// ---------------------------------------------------------------------------

fn be_u32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// s15Fixed16 — das Zahlenformat, in dem ICC alle Matrixwerte ablegt.
fn s15f16(d: &[u8], at: usize) -> Option<f32> {
    let raw = i32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?);
    Some(raw as f32 / 65536.0)
}

struct Tag {
    sig: [u8; 4],
    off: usize,
    len: usize,
}

fn tags(d: &[u8]) -> Vec<Tag> {
    let mut out = Vec::new();
    let Some(n) = be_u32(d, 128) else { return out };
    for i in 0..n as usize {
        let base = 132 + i * 12;
        let (Some(off), Some(len)) = (be_u32(d, base + 4), be_u32(d, base + 8)) else {
            break;
        };
        let Some(sig) = d.get(base..base + 4) else { break };
        out.push(Tag {
            sig: sig.try_into().unwrap_or([0; 4]),
            off: off as usize,
            len: len as usize,
        });
    }
    out
}

fn find<'a>(t: &'a [Tag], sig: &[u8; 4]) -> Option<&'a Tag> {
    t.iter().find(|x| &x.sig == sig)
}

/// XYZType-Tag lesen: drei s15Fixed16 nach einem 8-Byte-Kopf.
fn xyz_tag(d: &[u8], t: &Tag) -> Option<[f32; 3]> {
    if &d.get(t.off..t.off + 4)? != b"XYZ " {
        return None;
    }
    Some([
        s15f16(d, t.off + 8)?,
        s15f16(d, t.off + 12)?,
        s15f16(d, t.off + 16)?,
    ])
}

/// Uebertragungskurve als Tabelle **Bildschirmwert → linear**, auf `TRC_LEN`
/// Stuetzstellen gebracht. Beherrscht `curv` (leer / Gamma / Tabelle) und `para`
/// (die vier ueblichen parametrischen Formen).
fn trc_tag(d: &[u8], t: &Tag) -> Option<Vec<f32>> {
    let kind = d.get(t.off..t.off + 4)?;
    let mut out = vec![0f32; TRC_LEN];
    match kind {
        b"curv" => {
            let n = be_u32(d, t.off + 8)? as usize;
            if n == 0 {
                // Leer = Identitaet (linear).
                for (i, v) in out.iter_mut().enumerate() {
                    *v = i as f32 / (TRC_LEN - 1) as f32;
                }
            } else if n == 1 {
                // Ein einzelner u8Fixed8-Wert = schlichtes Gamma.
                let raw = u16::from_be_bytes(d.get(t.off + 12..t.off + 14)?.try_into().ok()?);
                let gamma = raw as f32 / 256.0;
                for (i, v) in out.iter_mut().enumerate() {
                    *v = (i as f32 / (TRC_LEN - 1) as f32).powf(gamma);
                }
            } else {
                // Tabelle aus u16-Werten, linear auf unsere Laenge umgerechnet.
                let base = t.off + 12;
                if base + n * 2 > d.len() {
                    return None;
                }
                let tab: Vec<f32> = (0..n)
                    .map(|i| {
                        u16::from_be_bytes([d[base + i * 2], d[base + i * 2 + 1]]) as f32 / 65535.0
                    })
                    .collect();
                for (i, v) in out.iter_mut().enumerate() {
                    let x = i as f32 / (TRC_LEN - 1) as f32 * (n - 1) as f32;
                    let i0 = x.floor() as usize;
                    let i1 = (i0 + 1).min(n - 1);
                    let f = x - i0 as f32;
                    *v = tab[i0] * (1.0 - f) + tab[i1] * f;
                }
            }
        }
        b"para" => {
            let ftype = u16::from_be_bytes(d.get(t.off + 8..t.off + 10)?.try_into().ok()?);
            let p = |i: usize| s15f16(d, t.off + 12 + i * 4).unwrap_or(0.0);
            let (g, a, b, c, dd, e, f) = match ftype {
                0 => (p(0), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                1 => (p(0), p(1), p(2), 0.0, -p(2) / p(1).max(1e-6), 0.0, 0.0),
                2 => (p(0), p(1), p(2), p(3), -p(2) / p(1).max(1e-6), p(3), p(3)),
                3 => (p(0), p(1), p(2), p(3), p(4), 0.0, 0.0),
                4 => (p(0), p(1), p(2), p(3), p(4), p(5), p(6)),
                _ => return None,
            };
            for (i, v) in out.iter_mut().enumerate() {
                let x = i as f32 / (TRC_LEN - 1) as f32;
                *v = if x >= dd {
                    (a * x + b).max(0.0).powf(g) + e
                } else {
                    c * x + f
                };
            }
        }
        _ => return None, // Tabellenprofil (`mft1`/`mAB `) — ehrlich aufgeben.
    }
    Some(out)
}

/// Kehrt eine monotone Tabelle um: aus „Bildschirmwert → linear" wird
/// „linear → Bildschirmwert". Genau das braucht der Shader am Ende.
fn invert_trc(fwd: &[f32]) -> Vec<f32> {
    let n = fwd.len();
    let lo = fwd[0];
    let hi = fwd[n - 1];
    let mut out = vec![0f32; TRC_LEN];
    let mut j = 0usize;
    for (i, v) in out.iter_mut().enumerate() {
        let y = lo + (hi - lo) * (i as f32 / (TRC_LEN - 1) as f32);
        while j + 1 < n && fwd[j + 1] < y {
            j += 1;
        }
        let y0 = fwd[j];
        let y1 = fwd[(j + 1).min(n - 1)];
        let f = if (y1 - y0).abs() < 1e-9 {
            0.0
        } else {
            (y - y0) / (y1 - y0)
        };
        *v = ((j as f32 + f) / (n - 1) as f32).clamp(0.0, 1.0);
    }
    out
}

/// Bradford-Anpassung zwischen zwei Weisspunkten.
///
/// ICC rechnet grundsaetzlich gegen D50, unser Arbeitsraum gegen D65. Ohne diesen
/// Schritt bekaeme die Vorschau einen leichten Stich ins Warme — klein genug, um ihn
/// nicht zu bemerken, und gross genug, um jede Farbentscheidung zu verfaelschen.
fn bradford(from: [f32; 3], to: [f32; 3]) -> Mat3 {
    const M: Mat3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let mul = |m: &Mat3, v: [f32; 3]| -> [f32; 3] {
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    };
    let s = mul(&M, from);
    let dst = mul(&M, to);
    let diag: Mat3 = [
        [dst[0] / s[0].max(1e-9), 0.0, 0.0],
        [0.0, dst[1] / s[1].max(1e-9), 0.0],
        [0.0, 0.0, dst[2] / s[2].max(1e-9)],
    ];
    let m_inv = color::mat_invert(&M).unwrap_or(color::IDENTITY);
    color::mat_mul(&m_inv, &color::mat_mul(&diag, &M))
}

const D50: [f32; 3] = [0.9642, 1.0000, 0.8249];
const D65: [f32; 3] = [0.95047, 1.00000, 1.08883];

/// Liest ein ICC-Profil und baut daraus, was die Vorschau braucht.
pub fn parse_icc(d: &[u8]) -> Option<MonitorProfil> {
    if d.len() < 132 || d.get(36..40)? != b"acsp" {
        return None;
    }
    // Nur RGB-Bildschirmprofile ergeben hier Sinn.
    if d.get(16..20)? != b"RGB " {
        return None;
    }
    let t = tags(d);
    let r = xyz_tag(d, find(&t, b"rXYZ")?)?;
    let g = xyz_tag(d, find(&t, b"gXYZ")?)?;
    let b = xyz_tag(d, find(&t, b"bXYZ")?)?;
    let trc = [
        invert_trc(&trc_tag(d, find(&t, b"rTRC")?)?),
        invert_trc(&trc_tag(d, find(&t, b"gTRC")?)?),
        invert_trc(&trc_tag(d, find(&t, b"bTRC")?)?),
    ];

    // Spalten sind die Primaerfarben: Bildschirm-RGB → XYZ (D50, wie ICC es fuehrt).
    let disp_to_xyz_d50: Mat3 = [
        [r[0], g[0], b[0]],
        [r[1], g[1], b[1]],
        [r[2], g[2], b[2]],
    ];
    // Auf D65 umrechnen, damit es zu unserem Arbeitsraum passt.
    let disp_to_xyz = color::mat_mul(&bradford(D50, D65), &disp_to_xyz_d50);
    let xyz_to_disp = color::mat_invert(&disp_to_xyz)?;
    let work_to_display = color::mat_mul(&xyz_to_disp, &color::arbeitsraum_zu_xyz_d65());

    Some(MonitorProfil {
        name: profil_name(d, &t).unwrap_or_else(|| "Bildschirm".into()),
        work_to_display,
        inverse_trc: trc,
        gamut_vs_srgb: gamut_flaeche(&disp_to_xyz) / gamut_flaeche(&color::SRGB_TO_XYZ),
    })
}

/// Grobes Mass fuer die Groesse des Farbbereichs: Dreiecksflaeche der Primaerfarben
/// in der xy-Ebene. Reicht voellig, um „groesser als sRGB" anzuzeigen.
fn gamut_flaeche(to_xyz: &Mat3) -> f32 {
    let xy = |c: usize| {
        let (x, y, z) = (to_xyz[0][c], to_xyz[1][c], to_xyz[2][c]);
        let s = (x + y + z).max(1e-9);
        (x / s, y / s)
    };
    let (ax, ay) = xy(0);
    let (bx, by) = xy(1);
    let (cx, cy) = xy(2);
    (((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)) * 0.5).abs()
}

/// Welche Sprache beim Profilnamen zuerst gilt (0 = Deutsch, 1 = Englisch).
/// Die Oberflaeche setzt das vor dem ersten Lesen — sonst stuende in einer
/// englischen Oberflaeche „Farb-LCD" statt „Color LCD".
static NAMENSSPRACHE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Profilnamen bevorzugt englisch (`true`) oder deutsch (`false`) lesen.
pub fn namen_auf_englisch(englisch: bool) {
    NAMENSSPRACHE.store(englisch as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Name des Profils.
///
/// `dscm` zuerst: macOS legt dort den Namen ab, den auch die Systemeinstellungen
/// zeigen („Color LCD", der Modellname des Bildschirms). In `desc` steht bei
/// denselben Profilen nur das nichtssagende „Display" — genau der Fall, den man
/// erst merkt, wenn man es einmal ausgegeben hat.
fn profil_name(d: &[u8], t: &[Tag]) -> Option<String> {
    for sig in [b"dscm", b"desc"] {
        let Some(tag) = find(t, sig) else { continue };
        let kind = d.get(tag.off..tag.off + 4);
        if kind == Some(b"mluc") {
            // `mluc` enthaelt denselben Namen in vielen Sprachen. Einfach den
            // ersten Eintrag zu nehmen liefert, was zufaellig vorn steht — bei
            // den Profilen von macOS kroatisch. Also gezielt suchen: Sprache der
            // Oberflaeche, sonst die andere (deutsch/englisch), sonst der erste.
            let anzahl = be_u32(d, tag.off + 8)? as usize;
            let recgroesse = be_u32(d, tag.off + 12)? as usize;
            let mut wahl: Option<(usize, usize)> = None; // (versatz, laenge)
            let mut rang = 99;
            for i in 0..anzahl {
                let r = tag.off + 16 + i * recgroesse.max(12);
                let sprache = d.get(r..r + 2)?;
                let len = be_u32(d, r + 4)? as usize;
                let off = be_u32(d, r + 8)? as usize;
                if off + len > tag.len {
                    continue;
                }
                let englisch = NAMENSSPRACHE.load(std::sync::atomic::Ordering::Relaxed) == 1;
                let punkte = match (sprache, englisch) {
                    (b"de", false) | (b"en", true) => 0,
                    (b"en", false) | (b"de", true) => 1,
                    _ => 2,
                };
                if punkte < rang {
                    rang = punkte;
                    wahl = Some((off, len));
                }
            }
            if let Some((off, len)) = wahl {
                // Der Versatz zaehlt ab Tag-Anfang, der Text ist UTF-16BE.
                let s = d.get(tag.off + off..tag.off + off + len)?;
                let u: Vec<u16> = s
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                let name = String::from_utf16_lossy(&u).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        } else if kind == Some(b"desc") {
            let n = be_u32(d, tag.off + 8)? as usize;
            if n > 1 && 12 + n <= tag.len {
                let s = d.get(tag.off + 12..tag.off + 12 + n - 1)?;
                let name = String::from_utf8_lossy(s).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Wo das Profil herkommt — je Betriebssystem
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod plattform {
    use std::ffi::c_void;

    #[link(name = "ColorSync", kind = "framework")] // Kennung: Systembibliothek
    #[link(name = "CoreGraphics", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn ColorSyncProfileCreateWithDisplayID(id: u32) -> *const c_void; // Kennung: Systemschnittstelle
        fn ColorSyncProfileCopyData(p: *const c_void, err: *mut *const c_void) -> *const c_void; // Kennung: Systemschnittstelle
        fn CFDataGetLength(d: *const c_void) -> isize;
        fn CFDataGetBytePtr(d: *const c_void) -> *const u8;
        fn CFRelease(p: *const c_void);
    }

    /// Fragt den Farbprofil-Dienst des Systems nach dem Profil des
    /// Hauptbildschirms. Das ist derselbe Weg, den auch die Systemeinstellungen
    /// gehen — kalibriert man den Bildschirm neu, sieht Fotowache das beim
    /// naechsten Start automatisch.
    pub fn profil_bytes() -> Option<Vec<u8>> {
        unsafe {
            let prof = ColorSyncProfileCreateWithDisplayID(CGMainDisplayID()); // Kennung: Systemschnittstelle
            if prof.is_null() {
                return None;
            }
            let data = ColorSyncProfileCopyData(prof, std::ptr::null_mut()); // Kennung: Systemschnittstelle
            CFRelease(prof);
            if data.is_null() {
                return None;
            }
            let len = CFDataGetLength(data) as usize;
            let ptr = CFDataGetBytePtr(data);
            let out = if ptr.is_null() || len == 0 {
                None
            } else {
                Some(std::slice::from_raw_parts(ptr, len).to_vec())
            };
            CFRelease(data);
            out
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod plattform {
    /// Linux: der Fenstermanager legt das Profil des Bildschirms als
    /// X-Eigenschaft `_ICC_PROFILE` ab; unter Wayland gibt es (noch) keinen
    /// einheitlichen Weg. Ohne Fund bleibt es bei sRGB — das ist die Annahme,
    /// unter der praktisch alle Linux-Programme ohnehin arbeiten.
    ///
    /// Windows: `GetICMProfileW` waere der Weg; solange es keinen Windows-Bau
    /// gibt, den jemand pruefen kann, wird hier nicht auf Verdacht gerufen.
    pub fn profil_bytes() -> Option<Vec<u8>> {
        let home = std::env::var("HOME").ok()?;
        for p in [
            format!("{home}/.local/share/icc/edid.icc"),
            "/usr/share/color/icc/colord/sRGB.icc".to_string(),
        ] {
            if let Ok(b) = std::fs::read(&p) {
                return Some(b);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein synthetisches sRGB-Profil muss praktisch die Einheitsmatrix ergeben —
    /// wer sRGB-Zahlen auf einen sRGB-Bildschirm schickt, darf nichts veraendern.
    #[test]
    fn srgb_profil_veraendert_nichts() {
        let icc = crate::icc::srgb_profil();
        let p = parse_icc(&icc).expect("sRGB-Profil muss lesbar sein");
        // Arbeitsraum → sRGB, auf beiden Wegen gerechnet, muss uebereinstimmen.
        let soll = color::working_to_srgb();
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (p.work_to_display[i][j] - soll[i][j]).abs() < 0.02,
                    "Zeile {i} Spalte {j}: {} statt {}",
                    p.work_to_display[i][j],
                    soll[i][j]
                );
            }
        }
        assert!((p.gamut_vs_srgb - 1.0).abs() < 0.02);
    }

    /// Und die Umkehrkurve muss die sRGB-Kennlinie treffen.
    #[test]
    fn umkehrkurve_trifft_srgb() {
        let icc = crate::icc::srgb_profil();
        let p = parse_icc(&icc).unwrap();
        let srgb_encode = |v: f32| {
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        for probe in [0.05f32, 0.18, 0.5, 0.9] {
            let i = (probe * (TRC_LEN - 1) as f32).round() as usize;
            let got = p.inverse_trc[1][i];
            assert!(
                (got - srgb_encode(probe)).abs() < 0.01,
                "bei {probe}: {got} statt {}",
                srgb_encode(probe)
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn echter_bildschirm_wird_erkannt() {
        // Auf einem Rechner ohne Bildschirm (CI) darf das leer bleiben — dann ist
        // nur wichtig, dass es nicht abstuerzt.
        if let Some(p) = hauptbildschirm() {
            assert!(!p.name.is_empty());
            assert!(p.gamut_vs_srgb > 0.5 && p.gamut_vs_srgb < 3.0, "{p:?}");
        }
    }
}
