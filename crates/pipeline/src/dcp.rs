//! Kameraprofile nach DNG-Spezifikation — der Weg zu den Farben der Referenz-Entwicklung.
//!
//! Die Referenz-Entwicklung entwickelt nicht mit EINER Farbmatrix, sondern mit
//! einem Kameraprofil (DCP): zwei Matrizen fuer zwei Lichtarten (Kunstlicht A
//! und Tageslicht D65), zwischen denen je nach Weissabgleich gemischt wird, dazu
//! eine Farbton/Saettigungs-Tabelle (HueSatMap), eine Look-Tabelle und eine
//! Grundkurve. Genau das macht ihren typischen Look der Farben aus — Hauttoene,
//! Himmelblau, Gruen.
//!
//! Fotowache liest diese Profile dort, wo andere Anwendungen sie installiert haben
//! (installierte Kameraprofile, DCP) — es bringt selbst keine fremden
//! Profildaten mit. Fehlt das Profil, gilt der Rueckfall: LibRaws Matrix (dieselbe
//! Tageslicht-Matrix, die auch das installierte Standardprofil traegt) ohne
//! Tabellen, mit derselben Grundkurve.
//!
//! Die Farbmathematik folgt der DNG-Spezifikation (Kapitel „Mapping Camera
//! Color Space to CIE XYZ Space") und dem DNG-SDK: Weisspunkt aus dem
//! Kamera-Weissabgleich iterativ bestimmen, Matrizen im Kehrwert der
//! Farbtemperatur mischen, Vorwaertsmatrix auf den Verbindungsraum D50.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

pub type M3 = [[f64; 3]; 3];
pub type V3 = [f64; 3];

// ---------------------------------------------------------------------------
// Kleine Matrix-Mathematik (f64 — Profile sind empfindlich gegen Rundung)
// ---------------------------------------------------------------------------

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn mulv(a: &M3, v: &V3) -> V3 {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

pub fn inv(m: &M3) -> Option<M3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-14 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d,
        ],
    ])
}

fn diag(v: &V3) -> M3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

fn mischen(a: &M3, b: &M3, g: f64) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = g * a[i][j] + (1.0 - g) * b[i][j];
        }
    }
    o
}

pub fn als_f32(m: &M3) -> [[f32; 3]; 3] {
    let mut o = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = m[i][j] as f32;
        }
    }
    o
}

/// Weisspunkt des Verbindungsraums (D50), wie das DNG-SDK ihn fuehrt.
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

pub fn xy_zu_xyz(xy: (f64, f64)) -> V3 {
    let (x, y) = (xy.0.clamp(1e-6, 0.999999), xy.1.clamp(1e-6, 0.999999));
    [x / y, 1.0, (1.0 - x - y) / y]
}

pub fn xyz_zu_xy(v: &V3) -> (f64, f64) {
    let s = v[0] + v[1] + v[2];
    if s > 0.0 {
        (v[0] / s, v[1] / s)
    } else {
        D50_XY
    }
}

/// Lineares ROMM-RGB (ISO 22028-2, D50) → XYZ D50. Darin rechnet das DNG-SDK die
/// Profiltabellen — und darin rechnet Fotowache seit Prozess 2 insgesamt.
pub const ROMM_ZU_XYZ: M3 = [
    [0.7976749, 0.1351917, 0.0313534],
    [0.2880402, 0.7118741, 0.0000857],
    [0.0000000, 0.0000000, 0.8252100],
];

pub fn xyz_zu_romm() -> M3 {
    inv(&ROMM_ZU_XYZ).expect("ROMM-RGB-Matrix invertierbar")
}

/// Bradford-Anpassung zwischen zwei Weisspunkten (Verhaeltnisse begrenzt wie
/// im DNG-SDK, damit Extremwerte nicht explodieren).
pub fn bradford(von: (f64, f64), nach: (f64, f64)) -> M3 {
    const MB: M3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let a = mulv(&MB, &xy_zu_xyz(von));
    let b = mulv(&MB, &xy_zu_xyz(nach));
    let mut d = [0.0; 3];
    for i in 0..3 {
        d[i] = if a[i] > 0.0 && b[i] > 0.0 {
            (b[i] / a[i]).clamp(0.1, 10.0)
        } else {
            1.0
        };
    }
    let mbi = inv(&MB).expect("Bradford invertierbar");
    mul(&mbi, &mul(&diag(&d), &MB))
}

// ---------------------------------------------------------------------------
// Farbtemperatur und Toenung — exakt die Skala der Referenz-Entwicklung
// ---------------------------------------------------------------------------

/// Robertson-Isothermen (Kehrwert-Temperatur in Mired, u, v, Steigung) — die
/// klassische Tabelle aus Wyszecki & Stiles, wie sie das DNG-SDK verwendet.
const ROBERTSON: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// Toenung der Referenz-Entwicklung = Abstand von der Planck-Kurve in uv mal −3000.
const TINT_SKALA: f64 = -3000.0;

/// Weisspunkt → (Kelvin, Toenung) in genau den Einheiten, die die
/// Referenz-Entwicklung anzeigt.
pub fn xy_zu_temp(xy: (f64, f64)) -> (f64, f64) {
    let (x, y) = xy;
    let u = 2.0 * x / (1.5 - x + 6.0 * y);
    let v = 3.0 * y / (1.5 - x + 6.0 * y);
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    for i in 1..=30 {
        let mut du = 1.0;
        let mut dv = ROBERTSON[i].3;
        let len = (1.0 + dv * dv).sqrt();
        du /= len;
        dv /= len;
        let uu = u - ROBERTSON[i].1;
        let vv = v - ROBERTSON[i].2;
        let mut dt = -uu * dv + vv * du;
        if dt <= 0.0 || i == 30 {
            if dt > 0.0 {
                dt = 0.0;
            }
            dt = -dt;
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let temp = 1.0e6 / (ROBERTSON[i - 1].0 * f + ROBERTSON[i].0 * (1.0 - f));
            let uu = u - (ROBERTSON[i - 1].1 * f + ROBERTSON[i].1 * (1.0 - f));
            let vv = v - (ROBERTSON[i - 1].2 * f + ROBERTSON[i].2 * (1.0 - f));
            let mut du2 = du * (1.0 - f) + last_du * f;
            let mut dv2 = dv * (1.0 - f) + last_dv * f;
            let len2 = (du2 * du2 + dv2 * dv2).sqrt();
            du2 /= len2;
            dv2 /= len2;
            let tint = (uu * du2 + vv * dv2) * TINT_SKALA;
            return (temp, tint);
        }
        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }
    (5000.0, 0.0)
}

/// (Kelvin, Toenung) → Weisspunkt — die Umkehrung von `xy_zu_temp`.
pub fn temp_zu_xy(temp: f64, tint: f64) -> (f64, f64) {
    let r = 1.0e6 / temp.clamp(1000.0, 1.0e6);
    let offset = tint / TINT_SKALA;
    for i in 0..30 {
        if r < ROBERTSON[i + 1].0 || i == 29 {
            let f = (ROBERTSON[i + 1].0 - r) / (ROBERTSON[i + 1].0 - ROBERTSON[i].0);
            let mut u = ROBERTSON[i].1 * f + ROBERTSON[i + 1].1 * (1.0 - f);
            let mut v = ROBERTSON[i].2 * f + ROBERTSON[i + 1].2 * (1.0 - f);
            let (mut uu1, mut vv1) = (1.0, ROBERTSON[i].3);
            let (mut uu2, mut vv2) = (1.0, ROBERTSON[i + 1].3);
            let len1 = (1.0 + vv1 * vv1).sqrt();
            let len2 = (1.0 + vv2 * vv2).sqrt();
            uu1 /= len1;
            vv1 /= len1;
            uu2 /= len2;
            vv2 /= len2;
            let mut uu3 = uu1 * f + uu2 * (1.0 - f);
            let mut vv3 = vv1 * f + vv2 * (1.0 - f);
            let len3 = (uu3 * uu3 + vv3 * vv3).sqrt();
            uu3 /= len3;
            vv3 /= len3;
            u += uu3 * offset;
            v += vv3 * offset;
            return (1.5 * u / (u - 4.0 * v + 2.0), v / (u - 4.0 * v + 2.0));
        }
    }
    D50_XY
}

/// EXIF-Lichtart → Farbtemperatur, exakt wie das DNG-SDK sie annimmt
/// (`IlluminantToTemperature`: Normlicht A = 2850 K, nicht 2856; Leuchtstoff
/// jeweils die Mitte seines Bereichs).
fn lichtart_kelvin(code: u16) -> f64 {
    match code {
        17 | 3 => 2850.0,                         // Normlicht A, Kunstlicht
        24 => 3200.0,                             // ISO-Studio-Kunstlicht
        23 => 5000.0,                             // D50
        20 | 1 | 9 | 4 | 18 => 5500.0,            // D55, Tageslicht, schoenes Wetter, Blitz, B
        21 | 19 | 10 => 6500.0,                   // D65, C, bewoelkt
        22 | 11 => 7500.0,                        // D75, Schatten
        12 => (5700.0 + 7100.0) * 0.5,            // Tageslicht-Leuchtstoff
        13 => (4600.0 + 5500.0) * 0.5,            // tagesweiss
        14 | 2 => (3800.0 + 4500.0) * 0.5,        // kaltweiss / Leuchtstoff
        15 => (3250.0 + 3800.0) * 0.5,            // weiss
        16 => (2600.0 + 3250.0) * 0.5,            // warmweiss
        _ => 0.0,
    }
}

/// Vorwaertsmatrix so normieren, dass Kamera-Weiss (1,1,1) exakt auf D50-Weiss
/// faellt (`FM ← diag(XYZ_D50) · diag(FM·1)⁻¹ · FM`, wie im DNG-SDK).
fn fm_normieren(fm: M3) -> M3 {
    let d50 = xy_zu_xyz(D50_XY);
    let mut o = fm;
    for i in 0..3 {
        let summe: f64 = fm[i].iter().sum();
        if summe.abs() > 1e-9 {
            for j in 0..3 {
                o[i][j] = fm[i][j] * d50[i] / summe;
            }
        }
    }
    o
}

// ---------------------------------------------------------------------------
// Das Profil
// ---------------------------------------------------------------------------

/// Farbton/Saettigung/Helligkeit-Tabelle: je Eintrag (Farbtonverschiebung in
/// Grad, Saettigungsfaktor, Helligkeitsfaktor). Reihenfolge wie im DNG-Standard:
/// Helligkeit aussen, dann Farbton, Saettigung innen.
#[derive(Debug, Clone)]
pub struct Tabelle {
    pub farbtoene: u32,
    pub saettigungen: u32,
    pub werte: u32,
    /// 0 = linear, 1 = sRGB-kodierte Helligkeitsachse.
    pub kodierung: u32,
    pub daten: Vec<f32>,
}

/// Woher ein Profil stammt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Herkunft {
    /// Ein DCP-Profil, das eine andere Anwendung auf diesem Rechner
    /// installiert hat.
    Installiert,
    /// Von der DNG selbst mitgebracht.
    Eingebettet,
    /// Fotowaches eigene Wiedergabe aus LibRaws Kameramatrix.
    Eigen,
}

/// Schluessel fuer `Edit::profil`, unter dem das eingebettete DNG-Profil
/// gewaehlt wird (neben "" = automatisch und "fotoraw" = eigene Wiedergabe).
pub const EINGEBETTET: &str = "dng";

/// Name des installierten Standardprofils: Teil seines DCP-Dateinamens, Name
/// seines Unterordners, Anzeigename im Profilmenue und gespeicherter Schluessel
/// fuer `Edit::profil` (aeltere Begleitdateien tragen ihn).
pub const STANDARDPROFIL: &str = "Adobe Standard"; // Kennung: DCP-Dateiname, gespeicherter Schluessel

#[derive(Debug, Clone)]
pub struct Kameraprofil {
    /// Name des installierten Standardprofils o. ae. — oder der Rueckfall.
    pub name: String,
    pub kamera: String,
    pub quelle: Option<PathBuf>,
    pub herkunft: Herkunft,
    cm1: M3,
    cm2: M3,
    fm1: Option<M3>,
    fm2: Option<M3>,
    t1: f64,
    t2: f64,
    hsm1: Option<Tabelle>,
    hsm2: Option<Tabelle>,
    pub look: Option<Tabelle>,
    /// Grundkurve, 1025 gleichabstaendige Werte (linear → linear).
    pub kurve: Vec<f32>,
    pub belichtung_versatz: f32,
    /// DefaultBlackRender: true = automatischer Schwarz-Abzug (Vorgabe der DNG-Spezifikation).
    pub schwarz_auto: bool,
}

/// Ergebnis fuer einen bestimmten Weissabgleich: alles, was die GPU braucht.
#[derive(Debug, Clone)]
pub struct Farbstand {
    /// Kamera (wie LibRaw sie liefert: mit dem Aufnahme-Weissabgleich
    /// multipliziert) → lineares ROMM-RGB D50, neutrales Sensor-Weiss = 1.
    pub kamera_zu_pro: [[f32; 3]; 3],
    /// Fuer den gewaehlten Weisspunkt gemischte HueSatMap (falls vorhanden).
    pub hsm: Option<Tabelle>,
    /// Weissabgleich in den Einheiten der Referenz-Entwicklung.
    pub kelvin: f32,
    pub toenung: f32,
    /// Weissabgleich der Aufnahme in den Einheiten der Referenz-Entwicklung.
    pub kelvin_aufnahme: f32,
    pub toenung_aufnahme: f32,
}

impl Kameraprofil {
    /// Rueckfall ohne installiertes Kameraprofil: LibRaws Matrix (die D65-Matrix
    /// der Kamera, wie sie auch das installierte Standardprofil traegt) fuer beide
    /// Lichtarten, keine Tabellen, Grundkurve des DNG-SDK (`grundkurve`).
    pub fn rueckfall(cam_xyz: &[[f32; 3]; 3], kamera: &str) -> Self {
        let mut cm = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                cm[i][j] = cam_xyz[i][j] as f64;
            }
        }
        // Eine leere/kaputte Matrix (manche Exoten) waere nicht invertierbar:
        // dann sRGB-artig neutral annehmen statt Muell zu rechnen.
        if inv(&cm).is_none() {
            cm = inv(&ROMM_ZU_XYZ).unwrap();
        }
        Self {
            name: "Fotowache Standard".into(),
            kamera: kamera.into(),
            quelle: None,
            herkunft: Herkunft::Eigen,
            cm1: cm,
            cm2: cm,
            fm1: None,
            fm2: None,
            t1: 6500.0,
            t2: 6500.0,
            hsm1: None,
            hsm2: None,
            look: None,
            kurve: crate::grundkurve::GRUNDKURVE.to_vec(),
            belichtung_versatz: 0.0,
            schwarz_auto: true,
        }
    }

    pub fn ist_installiert(&self) -> bool {
        self.herkunft == Herkunft::Installiert
    }

    /// Liest eine DCP-Datei (TIFF-Aufbau mit Kennung „IIRC"/„MMCR").
    pub fn lesen(daten: &[u8], quelle: Option<PathBuf>) -> Option<Self> {
        let t = TiffTags::lesen(daten)?;
        let kamera = t.text(0xC614).unwrap_or_default();
        let name = t.text(0xC6F8).unwrap_or_else(|| "Kameraprofil".into());
        let cm1 = t.matrix(0xC621)?;
        let cm2 = t.matrix(0xC622).unwrap_or(cm1);
        let fm1 = t.matrix(0xC714).map(fm_normieren);
        let fm2 = t.matrix(0xC715).map(fm_normieren).or(fm1);
        let mut t1 = t.zahlen(0xC65A).and_then(|v| v.first().copied()).map(|c| lichtart_kelvin(c as u16)).unwrap_or(0.0);
        let mut t2 = t.zahlen(0xC65B).and_then(|v| v.first().copied()).map(|c| lichtart_kelvin(c as u16)).unwrap_or(0.0);
        if t1 <= 0.0 {
            t1 = 6500.0;
        }
        if t2 <= 0.0 {
            t2 = t1;
        }
        // Sicherstellen: t1 ist die niedrigere Temperatur (wie im SDK).
        let (cm1, cm2, fm1, fm2, t1, t2, tausch) = if t1 > t2 {
            (cm2, cm1, fm2, fm1, t2, t1, true)
        } else {
            (cm1, cm2, fm1, fm2, t1, t2, false)
        };
        let hsm_kod = t.zahlen(0xC7A3).and_then(|v| v.first().copied()).unwrap_or(0.0) as u32;
        let (hsm1, hsm2) = match t.zahlen(0xC6F9) {
            Some(d) if d.len() == 3 => {
                let mk = |tag: u16| -> Option<Tabelle> {
                    let v = t.floats(tag)?;
                    let n = (d[0] * d[1] * d[2].max(1.0)) as usize * 3;
                    (v.len() == n).then(|| Tabelle {
                        farbtoene: d[0] as u32,
                        saettigungen: d[1] as u32,
                        werte: (d[2] as u32).max(1),
                        kodierung: hsm_kod,
                        daten: v,
                    })
                };
                let a = mk(0xC6FA);
                let b = mk(0xC6FB);
                let (a, b) = match (a, b) {
                    (Some(a), Some(b)) => (Some(a), Some(b)),
                    (Some(a), None) => (Some(a.clone()), Some(a)),
                    (None, Some(b)) => (Some(b.clone()), Some(b)),
                    _ => (None, None),
                };
                if tausch {
                    (b, a)
                } else {
                    (a, b)
                }
            }
            _ => (None, None),
        };
        let look = match t.zahlen(0xC725) {
            Some(d) if d.len() == 3 => t.floats(0xC726).and_then(|v| {
                let n = (d[0] * d[1] * d[2].max(1.0)) as usize * 3;
                (v.len() == n).then(|| Tabelle {
                    farbtoene: d[0] as u32,
                    saettigungen: d[1] as u32,
                    werte: (d[2] as u32).max(1),
                    kodierung: t.zahlen(0xC7A4).and_then(|v| v.first().copied()).unwrap_or(0.0) as u32,
                    daten: v,
                })
            }),
            _ => None,
        };
        let kurve = match t.floats(0xC6FC) {
            Some(p) if p.len() >= 4 && p.len() % 2 == 0 => spline_tabelle(&p),
            _ => crate::grundkurve::GRUNDKURVE.to_vec(),
        };
        let belichtung_versatz = t
            .zahlen(0xC7A5)
            .and_then(|v| v.first().copied())
            .unwrap_or(0.0) as f32;
        let schwarz_auto = t.zahlen(0xC7A6).and_then(|v| v.first().copied()).unwrap_or(0.0) == 0.0;
        Some(Self {
            name,
            kamera,
            quelle,
            herkunft: Herkunft::Installiert,
            cm1,
            cm2,
            fm1,
            fm2,
            t1,
            t2,
            hsm1,
            hsm2,
            look,
            kurve,
            belichtung_versatz,
            schwarz_auto,
        })
    }

    /// Mischgewicht der Lichtart 1 fuer eine Farbtemperatur (DNG-SDK).
    fn gewicht(&self, kelvin: f64) -> f64 {
        if (self.t2 - self.t1).abs() < 1.0 {
            return 1.0;
        }
        if kelvin <= self.t1 {
            1.0
        } else if kelvin >= self.t2 {
            0.0
        } else {
            (1.0 / kelvin - 1.0 / self.t2) / (1.0 / self.t1 - 1.0 / self.t2)
        }
    }

    fn xyz_zu_kamera(&self, xy: (f64, f64)) -> (M3, f64) {
        let (k, _) = xy_zu_temp(xy);
        let g = self.gewicht(k);
        (mischen(&self.cm1, &self.cm2, g), g)
    }

    /// Kamera-Neutral (Kehrwert des Weissabgleichs) → Weisspunkt (iterativ).
    pub fn neutral_zu_xy(&self, neutral: &V3) -> (f64, f64) {
        let mut last = D50_XY;
        for pass in 0..30 {
            let (cm, _) = self.xyz_zu_kamera(last);
            let Some(cmi) = inv(&cm) else { return last };
            let mut next = xyz_zu_xy(&mulv(&cmi, neutral));
            if (next.0 - last.0).abs() + (next.1 - last.1).abs() < 1e-7 {
                return next;
            }
            if pass == 29 {
                next = ((last.0 + next.0) * 0.5, (last.1 + next.1) * 0.5);
            }
            last = next;
        }
        last
    }

    /// Weisspunkt → Kamera-Neutral (groesster Wert 1).
    pub fn xy_zu_neutral(&self, xy: (f64, f64)) -> V3 {
        let (cm, _) = self.xyz_zu_kamera(xy);
        let n = mulv(&cm, &xy_zu_xyz(xy));
        let m = n.iter().cloned().fold(0.0f64, f64::max).max(1e-9);
        [
            (n[0] / m).clamp(0.001, 1.0),
            (n[1] / m).clamp(0.001, 1.0),
            (n[2] / m).clamp(0.001, 1.0),
        ]
    }

    /// Der Farbstand fuer einen Weissabgleich.
    ///
    /// `angewandt` sind die Faktoren, mit denen LibRaw die Pixel multipliziert
    /// hat (Pixel = Sensorwert · Faktor, siehe `RawMeta::wb_mul`), `aufnahme`
    /// der Weissabgleich der Aufnahme (`RawMeta::wb_aufnahme`). Meist dieselben
    /// Verhaeltnisse — aber nicht bei Daten ohne Mosaik, die LibRaw gar nicht
    /// abgleicht. `ziel` = None heisst „wie aufgenommen", sonst (Kelvin,
    /// Toenung) in den Einheiten der Referenz-Entwicklung.
    pub fn farbstand(&self, angewandt: &[f32; 4], aufnahme: &[f32; 4], ziel: Option<(f64, f64)>) -> Farbstand {
        // Neutral der Aufnahme: Kehrwert der Multiplikatoren, groesster Wert 1.
        let m = [
            aufnahme[0].max(1e-6) as f64,
            aufnahme[1].max(1e-6) as f64,
            aufnahme[2].max(1e-6) as f64,
        ];
        let n0 = {
            let n = [1.0 / m[0], 1.0 / m[1], 1.0 / m[2]];
            let mx = n.iter().cloned().fold(0.0f64, f64::max);
            [n[0] / mx, n[1] / mx, n[2] / mx]
        };
        let a = [
            angewandt[0].max(1e-9) as f64,
            angewandt[1].max(1e-9) as f64,
            angewandt[2].max(1e-9) as f64,
        ];
        let xy0 = self.neutral_zu_xy(&n0);
        let (k0, t0) = xy_zu_temp(xy0);
        let (xy, n) = match ziel {
            Some((k, t)) => {
                let xy = temp_zu_xy(k, t);
                (xy, self.xy_zu_neutral(xy))
            }
            None => (xy0, n0),
        };
        let (kelvin, toenung) = xy_zu_temp(xy);
        let (cm, g) = self.xyz_zu_kamera(xy);

        // Pixel p (LibRaw) = Kamera · a (Sensor-Saettigung = 1). Gesucht:
        // Kamera / N — neutrales Sensor-Weiss des Ziel-Lichts landet bei 1.
        let von_pixel_zu_kamera_durch_n = diag(&[
            1.0 / (a[0] * n[0]),
            1.0 / (a[1] * n[1]),
            1.0 / (a[2] * n[2]),
        ]);
        let kamera_zu_xyz50: M3 = match (&self.fm1, &self.fm2) {
            (Some(a), Some(b)) => {
                // Vorwaertsmatrix: bildet das weissabgeglichene Kamera-Weiss
                // (1,1,1) auf D50-Weiss ab.
                mul(&mischen(a, b, g), &von_pixel_zu_kamera_durch_n)
            }
            _ => {
                // Nur Farbmatrix: Verbindungsraum → Kamera fuer diesen
                // Weisspunkt, normiert, invertiert (DNG-SDK).
                let pcs_zu_kamera = mul(&cm, &bradford(D50_XY, xy));
                let w = mulv(&pcs_zu_kamera, &xy_zu_xyz(D50_XY));
                let s = w.iter().cloned().fold(0.0f64, f64::max).max(1e-9);
                let mut p = pcs_zu_kamera;
                for r in p.iter_mut() {
                    for v in r.iter_mut() {
                        *v /= s;
                    }
                }
                let kamera_zu_pcs = inv(&p).unwrap_or(ROMM_ZU_XYZ);
                // Pixel → Rohkamera: · N (Kamera = Kamera/N · N).
                mul(&kamera_zu_pcs, &mul(&diag(&n), &von_pixel_zu_kamera_durch_n))
            }
        };
        let kamera_zu_pro = mul(&xyz_zu_romm(), &kamera_zu_xyz50);
        let hsm = match (&self.hsm1, &self.hsm2) {
            (Some(a), Some(b)) => {
                let gf = g as f32;
                let mut t = a.clone();
                for (o, (x, y)) in t.daten.iter_mut().zip(a.daten.iter().zip(&b.daten)) {
                    *o = gf * x + (1.0 - gf) * y;
                }
                Some(t)
            }
            _ => None,
        };
        Farbstand {
            kamera_zu_pro: als_f32(&kamera_zu_pro),
            hsm,
            kelvin: kelvin as f32,
            toenung: toenung as f32,
            kelvin_aufnahme: k0 as f32,
            toenung_aufnahme: t0 as f32,
        }
    }
}

// ---------------------------------------------------------------------------
// Farbtabellen auf der CPU: Abfrage wie im Schader und Verkettung zweier
// Tabellen zu einer (fuer die Raw-Profile in `looks`: Look der DCP + Look des Profils)
// ---------------------------------------------------------------------------

fn srgb_kod(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn srgb_dekod(x: f64) -> f64 {
    let v = x.clamp(0.0, 1.0);
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Eintrag (Farbtonverschiebung in Grad, Saettigungsfaktor, Wertfaktor) einer
/// Tabelle an der Stelle (Farbton 0..6, Saettigung, Wert in der Kodierung der
/// Tabelle) — trilinear, genau wie `farbtabelle` im Schader.
fn tabelle_eintrag(t: &Tabelle, h: f64, s: f64, v_kod: f64) -> [f64; 3] {
    let (hn, sn, vn) = (t.farbtoene as usize, t.saettigungen as usize, t.werte as usize);
    let h_skal = h.rem_euclid(6.0) * hn as f64 / 6.0;
    let s_skal = s.clamp(0.0, 1.0) * (sn - 1) as f64;
    let mut h0 = (h_skal.floor() as usize).min(hn - 1);
    let s0 = (s_skal.floor() as usize).min(sn - 2);
    let mut h1 = h0 + 1;
    if h0 >= hn - 1 {
        h0 = hn - 1;
        h1 = 0;
    }
    let hf = h_skal - h0 as f64;
    let sf = s_skal - s0 as f64;
    let e = |vi: usize, hi: usize, si: usize| -> [f64; 3] {
        let o = ((vi * hn + hi) * sn + si) * 3;
        [t.daten[o] as f64, t.daten[o + 1] as f64, t.daten[o + 2] as f64]
    };
    let ebene = |vi: usize| -> [f64; 3] {
        let mut r = [0.0; 3];
        for k in 0..3 {
            let a = e(vi, h0, s0)[k] * (1.0 - hf) + e(vi, h1, s0)[k] * hf;
            let b = e(vi, h0, s0 + 1)[k] * (1.0 - hf) + e(vi, h1, s0 + 1)[k] * hf;
            r[k] = a * (1.0 - sf) + b * sf;
        }
        r
    };
    if vn < 2 {
        return ebene(0);
    }
    let v_skal = v_kod.clamp(0.0, 1.0) * (vn - 1) as f64;
    let v0 = (v_skal.floor() as usize).min(vn - 2);
    let vf = v_skal - v0 as f64;
    let (a, b) = (ebene(v0), ebene(v0 + 1));
    [a[0] * (1.0 - vf) + b[0] * vf, a[1] * (1.0 - vf) + b[1] * vf, a[2] * (1.0 - vf) + b[2] * vf]
}

/// Eine Tabelle auf einen HSV-Wert (Farbton 0..6, Wert linear) anwenden —
/// wie der Look-Durchgang im Schader (Wert gedeckelt).
fn tabelle_anwenden(t: &Tabelle, hsv: [f64; 3]) -> [f64; 3] {
    let v_kod = if t.kodierung == 1 { srgb_kod(hsv[2]) } else { hsv[2] };
    let d = tabelle_eintrag(t, hsv[0], hsv[1], v_kod);
    let h = hsv[0] + d[0] * (6.0 / 360.0);
    let s = (hsv[1] * d[1]).min(1.0);
    let v_neu = (v_kod * d[2]).clamp(0.0, 1.0);
    let v = if t.kodierung == 1 { srgb_dekod(v_neu) } else { v_neu };
    [h, s, v]
}

/// Zwei nacheinander angewandte Tabellen (erst `a`, dann `b`) zu einer
/// verrechnen. An jeder Stuetzstelle ist das Ergebnis exakt; dazwischen wird
/// wie gewohnt interpoliert. Raster: das feinere von beiden je Achse, Wertachse
/// in der Kodierung von `a`.
pub fn tabellen_verketten(a: &Tabelle, b: &Tabelle) -> Tabelle {
    let hn = a.farbtoene.max(b.farbtoene);
    let sn = a.saettigungen.max(b.saettigungen).max(2);
    let vn = a.werte.max(b.werte).max(1);
    let kodierung = a.kodierung;
    let mut daten = Vec::with_capacity((hn * sn * vn * 3) as usize);
    for vi in 0..vn {
        let v_kod = if vn < 2 { 1.0 } else { vi as f64 / (vn - 1) as f64 };
        let v = if kodierung == 1 { srgb_dekod(v_kod) } else { v_kod };
        for hi in 0..hn {
            let h = hi as f64 * 6.0 / hn as f64;
            for si in 0..sn {
                let s = si as f64 / (sn - 1) as f64;
                let nach_a = tabelle_anwenden(a, [h, s, v]);
                let nach_b = tabelle_anwenden(b, nach_a);
                // Faktoren an Stellen mit Saettigung/Wert 0 als Grenzwert der
                // beiden Einzelfaktoren (dort ist das Verhaeltnis unbestimmt).
                let ea = tabelle_eintrag(a, h, s, if a.kodierung == 1 { srgb_kod(v) } else { v });
                let v_b = if b.kodierung == 1 { srgb_kod(nach_a[2]) } else { nach_a[2] };
                let eb = tabelle_eintrag(b, nach_a[0], nach_a[1], v_b);
                let dh = (nach_b[0] - h) * (360.0 / 6.0);
                let ds = if s > 1e-6 { nach_b[1] / s } else { ea[1] * eb[1] };
                let dv = if v > 1e-6 {
                    let (k0, k2) = if kodierung == 1 { (srgb_kod(v), srgb_kod(nach_b[2])) } else { (v, nach_b[2]) };
                    if k0 > 1e-6 { k2 / k0 } else { ea[2] * eb[2] }
                } else {
                    ea[2] * eb[2]
                };
                daten.extend([dh as f32, ds as f32, dv as f32]);
            }
        }
    }
    Tabelle { farbtoene: hn, saettigungen: sn, werte: vn, kodierung, daten }
}

impl Kameraprofil {
    /// Dieses Profil (das installierte Standardprofil der Kamera) mit einem
    /// Raw-Profil aus `looks` obendrauf: dessen Look-Tabelle laeuft nach der
    /// eigenen Look-Tabelle der DCP — beide werden zu einer verrechnet.
    pub fn mit_look(&self, look: &crate::looks::LookProfil) -> Kameraprofil {
        let mut p = self.clone();
        p.name = look.name.clone();
        p.look = Some(match &self.look {
            Some(eigen) => tabellen_verketten(eigen, &look.tabelle),
            None => look.tabelle.clone(),
        });
        p
    }
}

// ---------------------------------------------------------------------------
// Profile finden
// ---------------------------------------------------------------------------

/// Orte, an denen andere Anwendungen ihre Kameraprofile (DCP) ablegen — plus ein
/// eigener Ordner fuer Profile, die man selbst hinlegt (z. B. aus einem DNG-Konverter).
pub fn suchorte() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(eigen) = std::env::var("FOTOWACHE_PROFILE") {
        v.push(PathBuf::from(eigen));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        for app in [
            "/Applications/Adobe Lightroom.app/Contents/Resources/CameraProfiles", // Kennung: Suchpfad
            "/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/CameraProfiles", // Kennung: Suchpfad
            "/Applications/Adobe DNG Converter.app/Contents/Resources/CameraProfiles", // Kennung: Suchpfad
            "/Library/Application Support/Adobe/CameraRaw/CameraProfiles", // Kennung: Suchpfad
        ] {
            v.push(PathBuf::from(app));
        }
        if let Some(h) = &home {
            v.push(h.join("Library/Application Support/Adobe/CameraRaw/CameraProfiles")); // Kennung: Suchpfad
            v.push(h.join("Library/Application Support/Fotowache/Profile"));
        }
    } else if cfg!(target_os = "windows") {
        v.push(PathBuf::from(r"C:\ProgramData\Adobe\CameraRaw\CameraProfiles")); // Kennung: Suchpfad
        v.push(PathBuf::from(r"C:\Program Files\Adobe\Adobe Lightroom Classic\Resources\CameraProfiles")); // Kennung: Suchpfad
        v.push(PathBuf::from(r"C:\Program Files\Adobe\Adobe DNG Converter\Resources\CameraProfiles")); // Kennung: Suchpfad
        if let Some(a) = std::env::var_os("APPDATA") {
            let a = PathBuf::from(a);
            v.push(a.join("Adobe/CameraRaw/CameraProfiles")); // Kennung: Suchpfad
            v.push(a.join("Fotowache/Profile"));
        }
    } else if let Some(h) = &home {
        v.push(h.join(".local/share/Fotowache/Profile"));
        v.push(h.join(".config/Fotowache/Profile"));
    }
    v
}

fn vergleichsform(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Sucht „<Hersteller> <Modell> <STANDARDPROFIL>.dcp" in allen Suchorten
/// (auch im gleichnamigen Unterordner `STANDARDPROFIL`).
pub fn finden(hersteller: &str, modell: &str) -> Option<PathBuf> {
    let soll = vergleichsform(&format!("{hersteller} {modell} {STANDARDPROFIL}"));
    // Manche Hersteller schreibt LibRaw anders als die installierten Profile —
    // das Modell allein ist eindeutig genug, solange der Name mit
    // `STANDARDPROFIL` endet.
    let soll_modell = vergleichsform(&format!("{modell} {STANDARDPROFIL}"));
    for ort in suchorte() {
        for ordner in [ort.join(STANDARDPROFIL), ort.clone()] {
            let Ok(rd) = std::fs::read_dir(&ordner) else { continue };
            let mut kandidaten: Vec<PathBuf> = Vec::new();
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp")) {
                    continue;
                }
                let stamm = vergleichsform(&p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                if stamm == soll {
                    return Some(p);
                }
                if stamm.ends_with(&soll_modell) {
                    kandidaten.push(p);
                }
            }
            // Nur ein eindeutiger Treffer ueber das Modell zaehlt.
            if kandidaten.len() == 1 {
                return kandidaten.pop();
            }
        }
    }
    None
}

/// Alle Profile, die fuer diese Kamera lokal vorliegen: das installierte
/// Standardprofil und die installierten Kamera-Abgleich-Profile („Camera
/// Standard", „Camera Vivid" …).
/// Rueckgabe: (Anzeigename, Datei).
pub fn profile_fuer_kamera(hersteller: &str, modell: &str) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    if std::env::var_os("FOTOWACHE_OHNE_FREMDPROFILE").is_some() {
        return v;
    }
    if let Some(p) = finden(hersteller, modell) {
        // Die Raw-Profile aus `looks` werden bewusst NICHT angeboten: Fotowaches
        // Wiedergabe mit dem installierten Standardprofil ist gegen
        // Referenz-Exporte MIT deren Standard-Look kalibriert — dessen Tabelle
        // obendrauf zaehlte doppelt (gemessen: Farbkraft +4 %, Farbabstand
        // unveraendert). Der Leser (`looks`) und `mit_look` bleiben fuer einen
        // spaeteren Ausbau, der die Looks RELATIV zum Standard-Look anwendet.
        if std::env::var_os("FOTOWACHE_LOOKS").is_some() {
            for name in crate::looks::ANGEBOTEN {
                if crate::looks::laden(name).is_some() {
                    v.push((name.to_string(), p.clone()));
                }
            }
        }
        v.push((STANDARDPROFIL.into(), p));
    }
    let kamera = vergleichsform(&format!("{hersteller} {modell}"));
    for ort in suchorte() {
        let Ok(rd) = std::fs::read_dir(ort.join("Camera")) else { continue };
        for e in rd.flatten() {
            let ordner = e.path();
            let name = ordner.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if vergleichsform(&name) != kamera {
                continue;
            }
            let Ok(dateien) = std::fs::read_dir(&ordner) else { continue };
            for d in dateien.flatten() {
                let p = d.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dcp")) {
                    continue;
                }
                let stamm = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                // „<Hersteller> <Modell> Camera Vivid" → „Camera Vivid"
                let anzeige = stamm
                    .find("Camera ")
                    .map(|i| stamm[i..].to_string())
                    .unwrap_or(stamm.clone());
                if !v.iter().any(|(n, _)| *n == anzeige) {
                    v.push((anzeige, p));
                }
            }
        }
    }
    // Reihenfolge wie in der Referenz-Entwicklung: deren eigene Profile zuerst,
    // dann die Kamera-Abgleich-Profile.
    let rang = |n: &str| -> usize {
        if let Some(i) = crate::looks::ANGEBOTEN.iter().position(|a| *a == n) {
            i
        } else if n == STANDARDPROFIL {
            crate::looks::ANGEBOTEN.len()
        } else {
            crate::looks::ANGEBOTEN.len() + 1
        }
    };
    v.sort_by(|a, b| rang(&a.0).cmp(&rang(&b.0)).then(a.0.cmp(&b.0)));
    v
}

/// Ein benanntes Profil fuer die Kamera laden (Name aus `profile_fuer_kamera`).
pub fn benannt(hersteller: &str, modell: &str, name: &str) -> Option<Arc<Kameraprofil>> {
    let schluessel = format!("{hersteller}|{modell}|benannt|{name}");
    let speicher = ZWISCHENSPEICHER.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(p) = speicher.lock().ok().and_then(|m| m.get(&schluessel).cloned()) {
        return Some(p);
    }
    let (_, pfad) = profile_fuer_kamera(hersteller, modell).into_iter().find(|(n, _)| n == name)?;
    let basis = std::fs::read(&pfad).ok().and_then(|d| Kameraprofil::lesen(&d, Some(pfad)))?;
    let p = Arc::new(match crate::looks::laden(name) {
        Some(look) if crate::looks::ANGEBOTEN.contains(&name) => basis.mit_look(&look),
        _ => basis,
    });
    if let Ok(mut m) = speicher.lock() {
        m.insert(schluessel, p.clone());
    }
    Some(p)
}

static ZWISCHENSPEICHER: OnceLock<Mutex<HashMap<String, Arc<Kameraprofil>>>> = OnceLock::new();

fn fingerabdruck(d: &[u8]) -> u64 {
    // FNV-1a — nur zum Wiedererkennen im Zwischenspeicher.
    d.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

/// Das Profil, das eine DNG selbst mitbringt (falls sie eines traegt).
pub fn eingebettet(meta: &rawcore::RawMeta) -> Option<Arc<Kameraprofil>> {
    let daten = meta.dng_profil.as_ref()?;
    let schluessel = format!("dng|{:016x}", fingerabdruck(daten));
    let speicher = ZWISCHENSPEICHER.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(p) = speicher.lock().ok().and_then(|m| m.get(&schluessel).cloned()) {
        return Some(p);
    }
    let mut p = Kameraprofil::lesen(daten, None)?;
    p.herkunft = Herkunft::Eingebettet;
    // Viele Hersteller lassen den Profilnamen leer — dann heisst es so wie
    // in der Referenz-Entwicklung.
    if p.name.is_empty() || p.name == "Kameraprofil" {
        p.name = "Eingebettet".into();
    }
    if p.kamera.is_empty() {
        p.kamera = format!("{} {}", meta.make, meta.model);
    }
    let p = Arc::new(p);
    if let Ok(mut m) = speicher.lock() {
        m.insert(schluessel, p.clone());
    }
    Some(p)
}

/// Das Profil fuer ein Foto, in der Reihenfolge der Referenz-Entwicklung: das
/// installierte Standardprofil, wenn eine andere Anwendung eines fuer die Kamera
/// mitbringt; sonst das Profil, das die DNG selbst traegt; sonst Fotowaches eigene
/// Wiedergabe aus LibRaws Matrix.
/// Einmal je Kamera gelesen und dann geteilt.
pub fn fuer_meta(meta: &rawcore::RawMeta) -> Arc<Kameraprofil> {
    let (hersteller, modell) = (meta.make.as_str(), meta.model.as_str());
    let schluessel = format!(
        "{hersteller}|{modell}|{:?}|{:016x}",
        meta.cam_to_xyz,
        meta.dng_profil.as_deref().map(fingerabdruck).unwrap_or(0)
    );
    let speicher = ZWISCHENSPEICHER.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(p) = speicher.lock().ok().and_then(|m| m.get(&schluessel).cloned()) {
        return p;
    }
    let kamera = format!("{hersteller} {modell}");
    let installiert = if std::env::var_os("FOTOWACHE_OHNE_FREMDPROFILE").is_some() {
        None
    } else {
        finden(hersteller, modell).and_then(|p| {
            std::fs::read(&p).ok().and_then(|d| Kameraprofil::lesen(&d, Some(p)))
        })
    };
    let p = match installiert {
        Some(a) => Arc::new(a),
        None => eingebettet(meta).unwrap_or_else(|| Arc::new(Kameraprofil::rueckfall(&meta.cam_to_xyz, &kamera))),
    };
    if let Ok(mut m) = speicher.lock() {
        m.insert(schluessel, p.clone());
    }
    p
}

// ---------------------------------------------------------------------------
// Grundkurve aus Stuetzpunkten (Spline, wie das DNG-SDK)
// ---------------------------------------------------------------------------

fn spline_tabelle(punkte: &[f32]) -> Vec<f32> {
    let xs: Vec<f64> = punkte.iter().step_by(2).map(|&v| v as f64).collect();
    let ys: Vec<f64> = punkte.iter().skip(1).step_by(2).map(|&v| v as f64).collect();
    let n = xs.len();
    if n < 2 {
        return crate::grundkurve::GRUNDKURVE.to_vec();
    }
    // Natuerlicher kubischer Spline.
    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    let mut c = vec![0.0; n];
    let mut r = vec![0.0; n];
    for i in 1..n - 1 {
        let h0 = xs[i] - xs[i - 1];
        let h1 = xs[i + 1] - xs[i];
        a[i] = h0;
        b[i] = 2.0 * (h0 + h1);
        c[i] = h1;
        r[i] = 6.0 * ((ys[i + 1] - ys[i]) / h1.max(1e-12) - (ys[i] - ys[i - 1]) / h0.max(1e-12));
    }
    b[0] = 1.0;
    b[n - 1] = 1.0;
    let mut m2 = vec![0.0; n];
    // Thomas-Algorithmus
    for i in 1..n {
        let w = a[i] / b[i - 1].max(1e-12);
        b[i] -= w * c[i - 1];
        r[i] -= w * r[i - 1];
    }
    m2[n - 1] = r[n - 1] / b[n - 1].max(1e-12);
    for i in (0..n - 1).rev() {
        m2[i] = (r[i] - c[i] * m2[i + 1]) / b[i].max(1e-12);
    }
    (0..1025)
        .map(|k| {
            let x = k as f64 / 1024.0;
            let mut i = 0;
            while i < n - 2 && x > xs[i + 1] {
                i += 1;
            }
            let h = (xs[i + 1] - xs[i]).max(1e-12);
            let t = ((x - xs[i]) / h).clamp(0.0, 1.0);
            let y = (1.0 - t) * ys[i]
                + t * ys[i + 1]
                + ((1.0 - t).powi(3) - (1.0 - t)) * m2[i] * h * h / 6.0
                + (t.powi(3) - t) * m2[i + 1] * h * h / 6.0;
            y.clamp(0.0, 1.0) as f32
        })
        .collect()
}

// ---------------------------------------------------------------------------
// TIFF-Leser fuer DCP-Dateien
// ---------------------------------------------------------------------------

struct TiffTags<'a> {
    d: &'a [u8],
    le: bool,
    tags: HashMap<u16, (u16, u32, u32)>, // typ, anzahl, versatz-oder-wert-position
}

impl<'a> TiffTags<'a> {
    fn lesen(d: &'a [u8]) -> Option<Self> {
        if d.len() < 8 {
            return None;
        }
        let le = match &d[..4] {
            b"IIRC" | b"II*\0" => true,
            b"MMCR" | b"MM\0*" => false,
            _ => return None,
        };
        let mut s = Self {
            d,
            le,
            tags: HashMap::new(),
        };
        let ifd = s.u32(4)? as usize;
        let n = s.u16(ifd)? as usize;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let tag = s.u16(e)?;
            let typ = s.u16(e + 2)?;
            let anzahl = s.u32(e + 4)?;
            let groesse = typ_groesse(typ) as u64 * anzahl as u64;
            let pos = if groesse <= 4 { (e + 8) as u32 } else { s.u32(e + 8)? };
            s.tags.insert(tag, (typ, anzahl, pos));
        }
        Some(s)
    }
    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.d.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.d.get(at..at + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn i32(&self, at: usize) -> Option<i32> {
        self.u32(at).map(|v| v as i32)
    }
    fn text(&self, tag: u16) -> Option<String> {
        let (_, n, pos) = *self.tags.get(&tag)?;
        let b = self.d.get(pos as usize..pos as usize + n as usize)?;
        Some(String::from_utf8_lossy(b).trim_end_matches('\0').trim().to_string())
    }
    /// Zahlenwerte beliebigen Typs als f64.
    fn zahlen(&self, tag: u16) -> Option<Vec<f64>> {
        let (typ, n, pos) = *self.tags.get(&tag)?;
        let p = pos as usize;
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n as usize {
            let x = match typ {
                1 | 7 => *self.d.get(p + i)? as f64,
                3 => self.u16(p + i * 2)? as f64,
                4 => self.u32(p + i * 4)? as f64,
                8 => self.u16(p + i * 2)? as i16 as f64,
                9 => self.i32(p + i * 4)? as f64,
                5 => {
                    let (a, b) = (self.u32(p + i * 8)?, self.u32(p + i * 8 + 4)?);
                    if b == 0 { 0.0 } else { a as f64 / b as f64 }
                }
                10 => {
                    let (a, b) = (self.i32(p + i * 8)?, self.i32(p + i * 8 + 4)?);
                    if b == 0 { 0.0 } else { a as f64 / b as f64 }
                }
                11 => f32::from_bits(self.u32(p + i * 4)?) as f64,
                12 => {
                    let b = self.d.get(p + i * 8..p + i * 8 + 8)?;
                    let a: [u8; 8] = b.try_into().ok()?;
                    if self.le { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) }
                }
                _ => return None,
            };
            v.push(x);
        }
        Some(v)
    }
    fn floats(&self, tag: u16) -> Option<Vec<f32>> {
        self.zahlen(tag).map(|v| v.into_iter().map(|x| x as f32).collect())
    }
    fn matrix(&self, tag: u16) -> Option<M3> {
        let v = self.zahlen(tag)?;
        if v.len() < 9 {
            return None;
        }
        Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
    }
}

fn typ_groesse(typ: u16) -> u32 {
    match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kelvin_hin_und_zurueck() {
        for (k, t) in [(2856.0, 0.0), (5000.0, 10.0), (6504.0, -8.0), (9000.0, 25.0)] {
            let xy = temp_zu_xy(k, t);
            let (k2, t2) = xy_zu_temp(xy);
            assert!((k2 - k).abs() / k < 0.002, "{k} -> {k2}");
            assert!((t2 - t).abs() < 0.2, "{t} -> {t2}");
        }
        // D65 liegt bei 6503 K / Toenung +9,8 — genau die Werte, die das DNG-SDK
        // (und damit die Referenz-Entwicklung) fuer D65 liefert. Deren Vorgabe
        // „Tageslicht" 5500/+10 liegt deshalb auch bei Toenung +10.
        let (k, t) = xy_zu_temp((0.3127, 0.3290));
        assert!((k - 6503.0).abs() < 15.0, "{k}");
        assert!((t - 9.8).abs() < 0.5, "{t}");
        // Referenzwerte des SDK: 5000 K / 0 → (0,34510, 0,35162).
        let (x, y) = temp_zu_xy(5000.0, 0.0);
        assert!((x - 0.34510).abs() < 2e-4 && (y - 0.35162).abs() < 2e-4, "{x} {y}");
    }

    fn testkamera() -> Kameraprofil {
        // Die Tageslicht-Matrix der Testkamera (40 MP), wie LibRaw sie fuehrt.
        let cm: [[f32; 3]; 3] = [
            [1.1809, -0.5358, -0.1141],
            [-0.4248, 1.2164, 0.2343],
            [-0.0514, 0.1097, 0.5848],
        ];
        Kameraprofil::rueckfall(&cm, "Testkamera")
    }

    /// Neutrales Grau bleibt neutral — bei jedem Weissabgleich der Aufnahme.
    #[test]
    fn grau_bleibt_grau() {
        let p = testkamera();
        for mul in [[1.9f32, 1.0, 1.6, 1.0], [1.4, 1.0, 2.4, 1.0]] {
            // Ein neutrales Objekt hat nach LibRaws Weissabgleich gleiche Kanaele;
            // in Pixeleinheiten liegt neutrales Sensor-Weiss beim Pegel min/max.
            let gr = mul[0].max(mul[2]).max(1.0);
            let pegel = 1.0 / gr;
            let angewandt = mul.map(|m| m / gr);
            let f = p.farbstand(&angewandt, &mul, None);
            let pix = [pegel; 3];
            let m = f.kamera_zu_pro;
            let o: Vec<f32> = (0..3).map(|i| (0..3).map(|j| m[i][j] * pix[j]).sum()).collect();
            for v in &o {
                assert!((v - 1.0).abs() < 0.01, "Sensor-Weiss muss ROMM-RGB-Weiss sein: {o:?}");
            }
        }
    }

    /// Eine DNG-Attrappe mit eigenem Profil (zwei Matrizen, Lichtarten, Name)
    /// in beiden Byte-Reihenfolgen: `rawcore::dng::lese_profil` muss daraus
    /// eine Datei machen, die der DCP-Leser versteht — mit denselben Werten.
    #[test]
    fn eingebettetes_dng_profil_wird_gelesen() {
        for le in [true, false] {
            let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
            let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
            let cm1: [i32; 9] = [12312, -5440, -1307, -6408, 15499, 824, -1075, 1677, 7220];
            let cm2: [i32; 9] = [11000, -4000, -1000, -5000, 14000, 1000, -1000, 2000, 6000];
            let name = b"Testprofil\0";
            // Eintraege: DNGVersion, UniqueCameraModel(fehlt absichtlich),
            // ColorMatrix1/2, CalibrationIlluminant1/2, ProfileName.
            let eintraege: Vec<(u16, u16, u32)> = vec![
                (0xC612, 1, 4),
                (0xC621, 10, 9),
                (0xC622, 10, 9),
                (0xC65A, 3, 1),
                (0xC65B, 3, 1),
                (0xC6F8, 2, name.len() as u32),
            ];
            let mut d: Vec<u8> = Vec::new();
            d.extend(if le { *b"II" } else { *b"MM" });
            d.extend(w16(42));
            d.extend(w32(8));
            let daten_start = 8 + 2 + eintraege.len() * 12 + 4;
            let mut daten: Vec<u8> = Vec::new();
            d.extend(w16(eintraege.len() as u16));
            for (tag, typ, anzahl) in &eintraege {
                d.extend(w16(*tag));
                d.extend(w16(*typ));
                d.extend(w32(*anzahl));
                match tag {
                    0xC612 => d.extend([1, 4, 0, 0]),
                    0xC65A => d.extend([w16(17), [0, 0]].concat()), // Normlicht A
                    0xC65B => d.extend([w16(21), [0, 0]].concat()), // D65
                    0xC621 | 0xC622 => {
                        d.extend(w32((daten_start + daten.len()) as u32));
                        let m = if *tag == 0xC621 { cm1 } else { cm2 };
                        for v in m {
                            daten.extend(w32(v as u32));
                            daten.extend(w32(10000));
                        }
                    }
                    _ => {
                        d.extend(w32((daten_start + daten.len()) as u32));
                        daten.extend(name);
                    }
                }
            }
            d.extend(w32(0));
            d.extend(&daten);
            let pfad = std::env::temp_dir().join(format!("fotowache_dngprofil_{le}.dng"));
            std::fs::write(&pfad, &d).unwrap();
            let blob = rawcore::dng::lese_profil(&pfad).expect("Profil muss gefunden werden");
            let _ = std::fs::remove_file(&pfad);
            let p = Kameraprofil::lesen(&blob, None).expect("DCP-Leser muss es verstehen");
            assert_eq!(p.name, "Testprofil");
            // Normlicht A (2850 K) ist die kaeltere Lichtart → cm1.
            assert!((p.t1 - 2850.0).abs() < 1.0 && (p.t2 - 6500.0).abs() < 1.0, "{} {}", p.t1, p.t2);
            assert!((p.cm1[0][0] - 1.2312).abs() < 1e-9 && (p.cm2[2][2] - 0.6).abs() < 1e-9);
        }
    }

    fn konstante_tabelle(h: u32, sat: u32, v: u32, eintrag: [f32; 3]) -> Tabelle {
        Tabelle {
            farbtoene: h,
            saettigungen: sat,
            werte: v,
            kodierung: 0,
            daten: (0..h * sat * v).flat_map(|_| eintrag).collect(),
        }
    }

    /// Zwei Tabellen hintereinander = eine verrechnete: Farbtonverschiebungen
    /// addieren sich, Saettigungs- und Wertfaktoren multiplizieren sich — auch
    /// bei verschiedenen Rastern.
    #[test]
    fn verkettete_tabellen_wirken_wie_nacheinander() {
        let a = konstante_tabelle(6, 4, 2, [10.0, 0.8, 0.9]);
        let b = konstante_tabelle(12, 3, 1, [-4.0, 1.25, 1.1]);
        let c = tabellen_verketten(&a, &b);
        assert_eq!((c.farbtoene, c.saettigungen, c.werte), (12, 4, 2));
        for (h, sat, v) in [(0.5, 0.5, 0.4), (3.2, 0.3, 0.7), (5.9, 0.6, 0.2)] {
            let einzeln = tabelle_anwenden(&b, tabelle_anwenden(&a, [h, sat, v]));
            let zusammen = tabelle_anwenden(&c, [h, sat, v]);
            for k in 0..3 {
                assert!((einzeln[k] - zusammen[k]).abs() < 1e-4, "{k}: {einzeln:?} {zusammen:?}");
            }
        }
        // Neutral bleibt neutral (Saettigung 0 bleibt 0).
        assert!(tabelle_anwenden(&c, [1.0, 0.0, 0.5])[1].abs() < 1e-9);
    }

    /// Waermer stellen (hoehere Kelvin) macht das Bild waermer: Rot rauf, Blau runter.
    #[test]
    fn waermer_heisst_mehr_rot() {
        let p = testkamera();
        let mul = [1.9f32, 1.0, 1.6, 1.0];
        let angewandt = mul.map(|m| m / 1.9);
        let a = p.farbstand(&angewandt, &mul, None);
        let b = p.farbstand(&angewandt, &mul, Some((a.kelvin as f64 + 1500.0, a.toenung as f64)));
        let grau = [0.3f32; 3];
        let anw = |m: &[[f32; 3]; 3]| -> Vec<f32> { (0..3).map(|i| (0..3).map(|j| m[i][j] * grau[j]).sum()).collect() };
        let (oa, ob) = (anw(&a.kamera_zu_pro), anw(&b.kamera_zu_pro));
        assert!(ob[0] / ob[2] > oa[0] / oa[2] * 1.05, "{oa:?} {ob:?}");
    }
}
