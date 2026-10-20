//! Farbmathematik: von den Sensorwerten der Kamera zu einem definierten Arbeitsraum.
//!
//! Das ist der Teil, den die Web-Version gar nicht hatte. Dort landeten die Pixel
//! ungefragt als sRGB in einer Canvas — jede Kamera sah dadurch anders „falsch" aus.
//! Hier gehen wir den Weg, den gaengige Raw-Entwickler auch gehen:
//!
//!   Kamera-RGB → CIE XYZ → linearer Arbeitsraum (Rec. 2020) → Bildschirm (sRGB)
//!
//! Der Arbeitsraum ist bewusst **weit** (Rec. 2020) und **linear**: Saettigung,
//! Belichtung und Kontrast rechnen sich in einem engen Raum wie sRGB in Farbfehler
//! hinein (Blau kippt nach Violett, Rot laeuft in die Kante). Erst ganz am Ende
//! wird auf den Bildschirmraum gebracht.

/// 3x3-Matrix, zeilenweise.
pub type Mat3 = [[f32; 3]; 3];

/// sRGB-Primaerfarben → CIE XYZ (D65). Aus der sRGB-Spezifikation.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];

/// Rec. 2020 → CIE XYZ (D65) — unser Arbeitsraum.
pub const REC2020_TO_XYZ: Mat3 = [
    [0.6369580, 0.1446169, 0.1688810],
    [0.2627002, 0.6779981, 0.0593017],
    [0.0000000, 0.0280727, 1.0609851],
];

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn mat_invert(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let mut o = [[0f32; 3]; 3];
    o[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det;
    o[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det;
    o[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det;
    o[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det;
    o[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det;
    o[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det;
    o[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det;
    o[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det;
    o[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det;
    Some(o)
}

/// Baut die Matrix **Kamera-RGB → linearer Arbeitsraum**.
///
/// LibRaw liefert `cam_xyz` in der Leserichtung des DNG-Standards: **XYZ → Kamera**.
/// Wir brauchen die Gegenrichtung, und zwar so normiert, dass ein neutrales Grau
/// auch neutral bleibt. Das ist genau das Verfahren aus dcraws `cam_xyz_coeff`:
///
/// 1. `cam_ws = cam_xyz · ws_to_xyz` — Kamera aus Arbeitsraum ausgedrueckt.
/// 2. Jede Zeile durch ihre Summe teilen. Damit bildet (1,1,1) im Arbeitsraum auf
///    (1,1,1) in der Kamera ab — der Weisspunkt sitzt fest, sonst bekaeme das
///    ganze Bild einen Stich.
/// 3. Invertieren.
///
/// Ohne Schritt 2 ist das Ergebnis mathematisch „richtig" und sieht trotzdem falsch aus —
/// ein Fehler, den man erst am fertigen Bild bemerkt.
pub fn camera_to_working(cam_xyz: &Mat3, ws_to_xyz: &Mat3) -> Mat3 {
    let mut cam_ws = mat_mul(cam_xyz, ws_to_xyz);
    for row in cam_ws.iter_mut() {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-9 {
            for v in row.iter_mut() {
                *v /= sum;
            }
        }
    }
    mat_invert(&cam_ws).unwrap_or(IDENTITY)
}

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Arbeitsraum seit Prozess 2: **lineares ROMM-RGB (D50)** — der Raum, in dem
/// die DNG-Spezifikation Kameraprofile definiert. Fuer Bildschirm und Dateien
/// wird per Bradford nach D65 angepasst. Spalten = Primaerfarben in XYZ (D65).
pub fn arbeitsraum_zu_xyz_d65() -> Mat3 {
    let pro = crate::dcp::ROMM_ZU_XYZ;
    let anp = crate::dcp::bradford(crate::dcp::D50_XY, (0.3127, 0.3290));
    let m = crate::dcp::mul(&anp, &pro);
    crate::dcp::als_f32(&m)
}

/// Arbeitsraum → sRGB (linear), fuer Dateien und als Bildschirm-Rueckfall.
pub fn working_to_srgb() -> Mat3 {
    let xyz_to_srgb = mat_invert(&SRGB_TO_XYZ).unwrap_or(IDENTITY);
    mat_mul(&xyz_to_srgb, &arbeitsraum_zu_xyz_d65())
}

/// Temperatur/Toenung als Multiplikatoren im **Kamerafarbraum**.
///
/// Wichtig: die Regler wirken *relativ* zum Weissabgleich der Aufnahme, denn der
/// steckt schon in den dekodierten Pixeln (er muss vor dem Demosaicing sitzen).
/// Man bewegt also die Kamera-Einstellung, statt eine absolute Kelvin-Zahl zu
/// setzen — genau das Verhalten der Web-Version, damit die KI-Werte weiter passen.
///
/// `temp` und `tint` kommen in Nutzer-Einheiten (-100..100).
pub fn wb_multipliers(temp: f32, tint: f32) -> [f32; 3] {
    // Waermer = mehr Rot, weniger Blau. Der Faktor 0.3 ist so gewaehlt, dass der
    // volle Ausschlag etwa dem entspricht, was die Web-Version gezeigt hat.
    let t = temp / 100.0;
    let g = tint / 100.0;
    let r = (1.0 + 0.30 * t) * (1.0 - 0.05 * g);
    let gr = 1.0 - 0.18 * g;
    let b = (1.0 - 0.30 * t) * (1.0 - 0.05 * g);
    // Auf Gruen normieren, damit die Helligkeit beim Drehen nicht mitwandert.
    [r / gr, 1.0, b / gr]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_roundtrip() {
        let m = SRGB_TO_XYZ;
        let inv = mat_invert(&m).unwrap();
        let id = mat_mul(&m, &inv);
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((id[i][j] - want).abs() < 1e-5, "{id:?}");
            }
        }
    }

    /// Der eigentliche Test der Farbkette: neutrales Kameragrau muss neutral bleiben.
    /// Genau hier faellt der vergessene Normierungsschritt auf.
    #[test]
    fn neutral_stays_neutral() {
        // Beispielmatrix einer echten Kamera (die Testkamera, 40 MP, XYZ -> Kamera).
        let cam_xyz: Mat3 = [
            [1.1809, -0.5358, -0.1141],
            [-0.4248, 1.2164, 0.2343],
            [-0.0514, 0.1097, 0.5848],
        ];
        let m = camera_to_working(&cam_xyz, &REC2020_TO_XYZ);
        let grey = [0.5f32, 0.5, 0.5];
        let out: Vec<f32> = (0..3)
            .map(|i| (0..3).map(|j| m[i][j] * grey[j]).sum())
            .collect();
        let spread = out.iter().cloned().fold(f32::MIN, f32::max)
            - out.iter().cloned().fold(f32::MAX, f32::min);
        assert!(spread < 1e-4, "Grau bekam einen Farbstich: {out:?}");
    }
}
