//! Objekte entfernen — der Reparaturpinsel.
//!
//! ## Das Verfahren
//! Klassischer Heilpinsel, wie ihn gaengige Raw-Entwickler und Bildbearbeitungen
//! benutzen, in drei Schritten:
//!
//! 1. **Quelle suchen.** Rings um die markierte Stelle werden Kandidaten abgetastet.
//!    Bewertet wird nicht die Stelle selbst (die soll ja weg), sondern der **Ring
//!    drumherum**: passt der Rand des Kandidaten zum Rand des Lochs, passt auch die
//!    Fuellung. Das ist der ganze Trick — er sorgt dafuer, dass eine Fuge im Putz
//!    ihre Fortsetzung findet statt eines beliebigen glatten Flecks.
//! 2. **Helligkeit angleichen.** Die mittlere Differenz beider Raender wird auf die
//!    Kopie addiert. Ohne diesen Schritt sieht man den Flicken als hellere oder
//!    dunklere Scheibe — der haeufigste Grund, warum Retusche auffliegt.
//! 3. **Weich einblenden.** Nach aussen laeuft die Kopie ueber ein weiches
//!    Randprofil aus, damit keine Kreiskante entsteht.
//!
//! ## Warum auf dem Bild VOR den Reglern
//! Gerechnet wird auf den linearen Kameradaten, bevor irgendein Regler wirkt.
//! Repariert man erst am Ende, muss man nach jedem Reglerzug neu reparieren — und
//! die Stelle veraendert sich, waehrend man daneben an der Belichtung dreht.
//! So wird die Stelle einmal geschlossen und verhaelt sich danach wie echtes Bild.
//!
//! ## Warum nicht auf der Grafikkarte
//! Die Quellsuche ist ein Suchproblem mit Abbruch, kein Rechnen pro Pixel. Auf der
//! CPU ist sie in wenigen Millisekunden erledigt (die Flecken sind klein), auf der
//! Grafikkarte waere sie umstaendlich und nicht schneller.

/// Ein Fleck in Pixelkoordinaten.
#[derive(Debug, Clone, Copy)]
pub struct Fleck {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// Wendet alle Flecken auf einen verschraenkten RGBA-Halbgleitkomma-Puffer an
/// (so, wie er auf die Grafikkarte geht). Arbeitet der Reihe nach — spaetere
/// Flecken sehen das Ergebnis frueherer, damit man auch benachbarte Stellen
/// nacheinander saeubern kann.
pub fn anwenden(data: &mut [u16], w: u32, h: u32, flecken: &[Fleck]) {
    for f in flecken {
        einen(data, w, h, *f);
    }
}

/// Rechnet die Anteils-Angaben aus den Bearbeitungsdaten in Pixel um.
pub fn aus_edit(heal: &[[f32; 3]], w: u32, h: u32) -> Vec<Fleck> {
    heal.iter()
        .map(|[x, y, r]| Fleck {
            x: x * w as f32,
            y: y * h as f32,
            // Radius am Bildbreiten-Anteil gemessen, damit ein Fleck in der
            // Vorschau und im Export dieselbe Stelle gleich gross trifft.
            r: (r * w as f32).max(2.0),
        })
        .collect()
}

fn px(data: &[u16], w: u32, h: u32, x: i32, y: i32) -> [f32; 3] {
    let x = x.clamp(0, w as i32 - 1) as usize;
    let y = y.clamp(0, h as i32 - 1) as usize;
    let i = (y * w as usize + x) * 4;
    [
        half::f16::from_bits(data[i]).to_f32(),
        half::f16::from_bits(data[i + 1]).to_f32(),
        half::f16::from_bits(data[i + 2]).to_f32(),
    ]
}

fn set(data: &mut [u16], w: u32, x: i32, y: i32, v: [f32; 3]) {
    let i = (y as usize * w as usize + x as usize) * 4;
    for c in 0..3 {
        data[i + c] = half::f16::from_f32(v[c].max(0.0)).to_bits();
    }
}

/// Punkte auf dem Ring um einen Mittelpunkt — die Vergleichsprobe.
/// 24 Richtungen auf zwei Radien reichen: mehr Proben verbessern das Ergebnis
/// nicht mehr messbar, kosten aber bei jeder Kandidatensuche erneut.
fn ring_proben(r: f32) -> Vec<(f32, f32)> {
    let mut v = Vec::with_capacity(48);
    for k in 0..24 {
        let a = k as f32 / 24.0 * std::f32::consts::TAU;
        for m in [1.08f32, 1.30] {
            v.push((a.cos() * r * m, a.sin() * r * m));
        }
    }
    v
}

fn einen(data: &mut [u16], w: u32, h: u32, f: Fleck) {
    let r = f.r;
    if r < 1.0 || w < 8 || h < 8 {
        return;
    }
    let proben = ring_proben(r);

    // Rand des Lochs einmal einlesen — er ist die Messlatte fuer alle Kandidaten.
    let ziel: Vec<[f32; 3]> = proben
        .iter()
        .map(|(dx, dy)| px(data, w, h, (f.x + dx) as i32, (f.y + dy) as i32))
        .collect();
    let ziel_mittel = mittel(&ziel);

    // Kandidaten: Ringe in wachsendem Abstand, je 16 Richtungen. Nah genug, dass
    // Licht und Struktur noch passen; weit genug, dass die Stoerung selbst nicht
    // mitkopiert wird.
    let mut bester: Option<(f32, f32, f32)> = None; // (fehler, cx, cy)
    for schritt in [2.0f32, 3.0, 4.2, 6.0] {
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU + schritt;
            let cx = f.x + a.cos() * r * schritt;
            let cy = f.y + a.sin() * r * schritt;
            // Kandidat muss vollstaendig im Bild liegen.
            if cx - r * 1.4 < 0.0
                || cy - r * 1.4 < 0.0
                || cx + r * 1.4 >= w as f32
                || cy + r * 1.4 >= h as f32
            {
                continue;
            }
            let kand: Vec<[f32; 3]> = proben
                .iter()
                .map(|(dx, dy)| px(data, w, h, (cx + dx) as i32, (cy + dy) as i32))
                .collect();
            let kand_mittel = mittel(&kand);
            // Bewertet wird die **Struktur**, nicht die absolute Helligkeit:
            // beide Raender werden vorher auf ihren Mittelwert bezogen. Sonst
            // gewinnt immer der Kandidat mit der aehnlichsten Helligkeit, auch
            // wenn er voellig andere Strukturen hat.
            let mut fehler = 0.0f32;
            for (a, b) in ziel.iter().zip(kand.iter()) {
                for c in 0..3 {
                    let d = (a[c] - ziel_mittel[c]) - (b[c] - kand_mittel[c]);
                    fehler += d * d;
                }
            }
            // Leichter Zuschlag fuer Entfernung: bei gleichwertigen Kandidaten
            // gewinnt der naehere, weil dort Licht und Schaerfe besser passen.
            fehler *= 1.0 + schritt * 0.02;
            if bester.map_or(true, |(bf, _, _)| fehler < bf) {
                bester = Some((fehler, cx, cy));
            }
        }
    }
    let Some((_, cx, cy)) = bester else { return };

    // Schritt 2: Helligkeitsversatz aus den beiden Raendern.
    let kand: Vec<[f32; 3]> = proben
        .iter()
        .map(|(dx, dy)| px(data, w, h, (cx + dx) as i32, (cy + dy) as i32))
        .collect();
    let kand_mittel = mittel(&kand);
    let versatz = [
        ziel_mittel[0] - kand_mittel[0],
        ziel_mittel[1] - kand_mittel[1],
        ziel_mittel[2] - kand_mittel[2],
    ];

    // Schritt 3: kopieren mit weichem Rand.
    let x0 = ((f.x - r) as i32).max(0);
    let x1 = ((f.x + r) as i32 + 1).min(w as i32 - 1);
    let y0 = ((f.y - r) as i32).max(0);
    let y1 = ((f.y + r) as i32 + 1).min(h as i32 - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - f.x;
            let dy = y as f32 + 0.5 - f.y;
            let d = (dx * dx + dy * dy).sqrt() / r;
            if d >= 1.0 {
                continue;
            }
            // Weiches Randprofil: innen voll, aussen 0, dazwischen glatt.
            let t = ((1.0 - d) / 0.35).clamp(0.0, 1.0);
            let a = t * t * (3.0 - 2.0 * t);
            let s = px(data, w, h, (cx + dx) as i32, (cy + dy) as i32);
            let alt = px(data, w, h, x, y);
            let neu = [
                alt[0] + ((s[0] + versatz[0]).max(0.0) - alt[0]) * a,
                alt[1] + ((s[1] + versatz[1]).max(0.0) - alt[1]) * a,
                alt[2] + ((s[2] + versatz[2]).max(0.0) - alt[2]) * a,
            ];
            set(data, w, x, y, neu);
        }
    }
}

fn mittel(v: &[[f32; 3]]) -> [f32; 3] {
    let n = v.len().max(1) as f32;
    let mut m = [0f32; 3];
    for p in v {
        for c in 0..3 {
            m[c] += p[c];
        }
    }
    [m[0] / n, m[1] / n, m[2] / n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bild(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> Vec<u16> {
        let mut d = vec![0u16; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                let i = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    d[i + c] = half::f16::from_f32(v[c]).to_bits();
                }
                d[i + 3] = half::f16::from_f32(1.0).to_bits();
            }
        }
        d
    }

    /// Der eigentliche Nachweis: ein Fleck auf einem Verlauf muss verschwinden,
    /// und zwar so, dass der Verlauf an der Stelle weiterlaeuft.
    #[test]
    fn fleck_verschwindet_im_verlauf() {
        let (w, h) = (200u32, 200u32);
        let grund = |x: u32, _y: u32| {
            let v = 0.2 + 0.6 * (x as f32 / 200.0);
            [v, v, v]
        };
        let mut d = bild(w, h, |x, y| {
            let mut v = grund(x, y);
            // Stoerung: heller Klecks bei (100, 100), Radius 8.
            let dx = x as f32 - 100.0;
            let dy = y as f32 - 100.0;
            if dx * dx + dy * dy < 64.0 {
                v = [0.95, 0.1, 0.1];
            }
            v
        });

        let vorher = px(&d, w, h, 100, 100);
        assert!(vorher[0] > 0.9 && vorher[1] < 0.2, "Testbild falsch: {vorher:?}");

        anwenden(&mut d, w, h, &[Fleck { x: 100.0, y: 100.0, r: 11.0 }]);

        let nachher = px(&d, w, h, 100, 100);
        let soll = grund(100, 100);
        assert!(
            (nachher[0] - soll[0]).abs() < 0.06
                && (nachher[1] - soll[1]).abs() < 0.06
                && (nachher[2] - soll[2]).abs() < 0.06,
            "Stelle nicht sauber geschlossen: {nachher:?} statt ~{soll:?}"
        );
    }

    /// Und der Gegentest: ausserhalb des Flecks darf sich nichts aendern.
    #[test]
    fn umgebung_bleibt_unberuehrt() {
        let (w, h) = (120u32, 120u32);
        let mut d = bild(w, h, |x, _| {
            let v = x as f32 / 120.0;
            [v, v, v]
        });
        let vorher = px(&d, w, h, 10, 10);
        anwenden(&mut d, w, h, &[Fleck { x: 60.0, y: 60.0, r: 6.0 }]);
        let nachher = px(&d, w, h, 10, 10);
        assert_eq!(vorher, nachher);
    }

    /// Anteile muessen sich unabhaengig von der Aufloesung auf dieselbe Stelle
    /// abbilden — sonst sitzt die Retusche im Export woanders als in der Vorschau.
    #[test]
    fn anteile_skalieren_mit() {
        let klein = aus_edit(&[[0.5, 0.25, 0.01]], 1000, 800);
        let gross = aus_edit(&[[0.5, 0.25, 0.01]], 4000, 3200);
        assert_eq!(klein[0].x / 1000.0, gross[0].x / 4000.0);
        assert_eq!(klein[0].y / 800.0, gross[0].y / 3200.0);
        assert_eq!(klein[0].r / 1000.0, gross[0].r / 4000.0);
    }
}
