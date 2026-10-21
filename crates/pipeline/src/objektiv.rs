//! Objektivkorrektur — Kamera-Korrektur, echte Profile, gemessene Randabdunklung.
//!
//! **RAF-Kameras (6x6-Farbmuster, vierte/fuenfte Sensorgeneration):** die RAF
//! traegt die Korrektur des angesetzten Objektivs selbst — Verzeichnung,
//! Farbsaeume und Vignettierung je Aufnahme (`rawcore::raf`). Genau das wendet
//! die Referenz-Entwicklung als Profil „Kameraeinstellungen" an; Fotowache nimmt
//! es jetzt ebenso, vor allem anderen. (Die fruehere Annahme „die Testkamera
//! traegt keine Korrekturdaten" kam von der Suche nach den falschen Tags
//! 0x1500 ff. — richtig sind 0xF00B/0xF00F/0xF010.)
//!
//! Ohne Kameradaten: **Herstellerdaten aus der DNG** (Opcodes), dann ein
//! **installiertes Objektivprofil** (LCP, nur wenn eine andere Anwendung es
//! mitgebracht hat), dann **lensfun** (mitgeliefert, CC BY-SA). Ohne jedes
//! Profil bleibt das Bild unkorrigiert wie in der Referenz-Entwicklung — die
//! alte Schaetzung aus der Brennweite gilt automatisch nur noch bei RAF-Kameras
//! (dort gegen echte Fotos abgeglichen) und bleibt sonst im Profilmenue waehlbar.
//!
//! `ptlens`: r_quelle = r_anzeige · (a·r³ + b·r² + c·r + d) mit d = 1−a−b−c.
//! Der Radius ist auf die **kleinere Halbseite** des Bildes normiert.

/// Eine Stuetzstelle des ptlens-Modells: Brennweite in mm, dann a, b, c.
type Stuetze = (f32, f32, f32, f32);

/// Reisezoom 18–300 mm F3.5-6.3 (B061X) mit dem Anschluss der RAF-Kameras.
/// Quelle: lensfun, Eintrag B061X (kalibriert an einer APS-C-Kamera eines
/// anderen Herstellers, gleicher Bildkreis APS-C). Ein beliebtes Reisezoom an
/// RAF-Kameras.
const REISEZOOM_B061: &[Stuetze] = &[
    (18.0, 0.02805, -0.10668, 0.06382),
    (27.0, 0.01683, -0.04455, 0.03014),
    (38.0, -0.01212, 0.06048, -0.07478),
    (53.0, -0.00924, 0.05064, -0.06029),
    (60.0, -0.00417, 0.0245, -0.02118),
    (86.0, -0.00333, 0.02128, -0.02027),
    (100.0, 0.01239, -0.03935, 0.05252),
    (129.0, -0.00494, 0.02116, -0.01855),
    (158.0, -0.00539, 0.01724, -0.00662),
    (196.0, -0.02316, 0.09408, -0.11133),
    (241.0, 0.00321, -0.01004, 0.01943),
    (277.0, 0.01905, -0.04979, 0.03944),
    (300.0, 0.03685, -0.12817, 0.15026),
];

/// Ein Profil: woran man das Objektiv im EXIF erkennt, und seine Stuetzstellen.
struct Profil {
    /// Teilstring des EXIF-`LensModel`, unabhaengig von Gross-/Kleinschreibung.
    kennung: &'static str,
    name: &'static str,
    stuetzen: &'static [Stuetze],
}

const PROFILE: &[Profil] = &[Profil {
    kennung: "B061",
    name: "Reisezoom 18–300 mm (B061)",
    stuetzen: REISEZOOM_B061,
}];

/// Generische Telefon-Modul-Kurven fuer den Fall, dass eine Datei KEINE
/// eingebetteten Korrekturdaten traegt (Normalfall ist: sie traegt welche,
/// siehe `rawcore::dng`). Haupt- und Telekamera stammen aus der einzigen
/// echten lensfun-Kalibrierung eines Telefons des Telefonherstellers (Generation
/// 2018, `misc.xml`) — die Optiken der Nachbarmodelle sind eng verwandt. Das
/// Ultraweitwinkel ist bewusst zurueckhaltend gewaehlt (unkalibriert): lieber
/// Rest-Tonne als verbogene Geraden in die andere Richtung.
pub const TELEFON_HAUPT: [f32; 3] = [0.02465, -0.089989, 0.111108];
pub const TELEFON_TELE: [f32; 3] = [-0.044229, 0.125299, -0.112978];
pub const TELEFON_UW: [f32; 3] = [0.0, -0.12, 0.0];

/// Hoechstens so viele Stuetzstellen fuehrt der Schader (RAF: 9 + Mitte,
/// Objektivprofile: 16).
pub const RADIAL_MAX: usize = 16;

/// Radiale Korrektur aus Stuetzstellen (Kameradaten der RAF). Radien als Anteil
/// der halben Bilddiagonale; zwischen den Stuetzstellen linear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radial {
    pub n: usize,
    /// Radien im aufgenommenen Bild.
    pub r_quelle: [f32; RADIAL_MAX],
    /// Dieselben Stellen im korrigierten Bild (r_quelle / m).
    pub r_ziel: [f32; RADIAL_MAX],
    /// Massstab der Verzeichnung: Quelle = m · Ziel.
    pub m: [f32; RADIAL_MAX],
    /// Farbsaeume: Rot/Blau werden um (1 + ca) gegen Gruen skaliert.
    pub ca_r: [f32; RADIAL_MAX],
    pub ca_b: [f32; RADIAL_MAX],
    /// Helligkeitsausgleich der Vignettierung (100 / Randhelligkeit).
    pub gain: [f32; RADIAL_MAX],
    /// Fuellmassstab: verhindert Abtasten ausserhalb des Bildes.
    pub fuell: f32,
}

impl Radial {
    /// Aus den Tabellen der RAF; ein neutraler Knoten in der Mitte kommt dazu.
    pub fn aus_raf(f: &rawcore::raf::RafObjektiv, breite: u32, hoehe: u32) -> Option<Self> {
        let n = f.knoten.len() + 1;
        if n > RADIAL_MAX || f.knoten.is_empty() {
            return None;
        }
        let mut r = Radial {
            n,
            r_quelle: [0.0; RADIAL_MAX],
            r_ziel: [0.0; RADIAL_MAX],
            m: [1.0; RADIAL_MAX],
            ca_r: [0.0; RADIAL_MAX],
            ca_b: [0.0; RADIAL_MAX],
            gain: [1.0; RADIAL_MAX],
            fuell: 1.0,
        };
        for i in 0..f.knoten.len() {
            let m = (1.0 + f.verzeichnung[i] / 100.0).max(0.3);
            r.r_quelle[i + 1] = f.knoten[i];
            r.m[i + 1] = m;
            r.r_ziel[i + 1] = f.knoten[i] / m;
            r.ca_r[i + 1] = f.ca_rot[i];
            r.ca_b[i + 1] = f.ca_blau[i];
            r.gain[i + 1] = 100.0 / f.vignette[i].max(10.0);
        }
        // Die Mitte ist neutral; Farbsaum dort wie am ersten Knoten (stetig).
        r.ca_r[0] = r.ca_r[1];
        r.ca_b[0] = r.ca_b[1];
        if !r.r_ziel[..n].windows(2).all(|w| w[1] > w[0]) {
            return None; // nicht umkehrbar — lieber gar nicht korrigieren
        }
        r.fuell = r.fuellmassstab(breite, hoehe);
        Some(r)
    }

    fn lerp(knoten: &[f32], werte: &[f32], x: f32) -> f32 {
        if x <= knoten[0] {
            return werte[0];
        }
        for i in 1..knoten.len() {
            if x <= knoten[i] {
                let t = (x - knoten[i - 1]) / (knoten[i] - knoten[i - 1]).max(1e-9);
                return werte[i - 1] + (werte[i] - werte[i - 1]) * t;
            }
        }
        werte[knoten.len() - 1]
    }

    /// Verzeichnungs-Massstab fuer einen Radius im korrigierten Bild.
    pub fn m_an(&self, r_ziel: f32) -> f32 {
        Self::lerp(&self.r_ziel[..self.n], &self.m[..self.n], r_ziel)
    }

    /// Helligkeitsausgleich an einem Radius im aufgenommenen Bild.
    pub fn gain_an(&self, r_quelle: f32) -> f32 {
        Self::lerp(&self.r_quelle[..self.n], &self.gain[..self.n], r_quelle)
    }

    /// Wie weit die Ausgabe hineinzoomen muss, damit kein Randpunkt ausserhalb
    /// des aufgenommenen Bildes abtastet (Kissen-Verzeichnung im Tele).
    fn fuellmassstab(&self, breite: u32, hoehe: u32) -> f32 {
        let (hw, hh) = (breite as f32 / 2.0, hoehe as f32 / 2.0);
        let diag = (hw * hw + hh * hh).sqrt().max(1.0);
        let mut groesste = 1.0f32;
        for i in 0..=64 {
            let t = i as f32 / 64.0;
            for (x, y) in [(hw * (2.0 * t - 1.0), hh), (hw, hh * (2.0 * t - 1.0))] {
                let m = self.m_an((x * x + y * y).sqrt() / diag);
                groesste = groesste.max((m * x.abs() / hw).max(m * y.abs() / hh));
            }
        }
        1.0 / groesste
    }
}

/// Was die Pipeline fuer ein konkretes Foto wissen muss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Objektiv {
    /// ptlens a, b, c fuer die Aufnahme-Brennweite (0,0,0 = keine Verzeichnung).
    pub abc: [f32; 3],
    /// Fuellmassstab: verhindert, dass die Korrektur ausserhalb des Bildes
    /// abtastet (die Web-Version hat mit verschmierten Raendern bezahlt, bis sie
    /// stattdessen die Ecken leicht zuschnitt — derselbe Gedanke hier).
    pub skala: f32,
    /// Gemessene Randabdunklung 0..70 in Regler-Einheiten (`lensDevignette`).
    pub devignette: f32,
    /// Woher die Verzeichnung stammt — fuer die Anzeige in der Oberflaeche.
    pub quelle: ObjektivQuelle,
    /// Aufnahme-Brennweite in mm — braucht die manuelle Profilwahl
    /// (Reisezoom-Stuetzstellen, Brennweiten-Schätzung).
    pub brennweite: f32,
    /// Brennweite in Kleinbild-mm, so gut wir sie kennen (0 = unbekannt).
    pub brennweite_kb: f32,
    /// Kamera-Korrektur aus der Datei (RAF) — hat Vorrang vor allem anderen.
    pub radial: Option<Radial>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjektivQuelle {
    /// Vom Hersteller in die Datei geschriebene Korrektur (Telefon-DNG & Co.).
    Datei,
    /// Kamera-Korrektur der RAF (wie das Profil „Kameraeinstellungen" der
    /// Referenz-Entwicklung).
    Kamera,
    /// Installiertes Objektivprofil (LCP), das eine andere Anwendung mitgebracht hat.
    Installiert,
    /// Profil aus der lensfun-Datenbank.
    Datenbank,
    Profil,
    /// Generisches Telefon-Modul (Datei ohne eingebettete Korrektur).
    Modul,
    Brennweite,
    Keine,
}

impl Default for Objektiv {
    fn default() -> Self {
        Self {
            abc: [0.0; 3],
            skala: 1.0,
            devignette: 0.0,
            quelle: ObjektivQuelle::Keine,
            brennweite: 0.0,
            brennweite_kb: 0.0,
            radial: None,
        }
    }
}

impl Objektiv {
    /// Wie weit (in Pixeln des vollen Bildes) die Korrektur ein Pixel
    /// hoechstens verschiebt — so viel Rand braucht ein 1:1-Ausschnitt.
    pub fn max_verschiebung(&self, breite: u32, hoehe: u32) -> f32 {
        let (hw, hh) = (breite as f32 / 2.0, hoehe as f32 / 2.0);
        let diag = (hw * hw + hh * hh).sqrt();
        let mut groesste = 0f32;
        for i in 0..=32 {
            let r = i as f32 / 32.0; // Anteil der halben Diagonale
            let mut f = 1.0f32;
            if let Some(rad) = &self.radial {
                f *= rad.fuell * rad.m_an(r * rad.fuell);
                // Farbsaum verschiebt Rot/Blau zusaetzlich.
                let ca = rad.ca_r.iter().chain(rad.ca_b.iter()).fold(0f32, |a, b| a.max(b.abs()));
                f += ca;
            }
            if self.abc != [0.0; 3] {
                let rk = r * diag / hw.min(hh).max(1.0);
                f *= ptlens(self.abc, rk) * self.skala;
            }
            groesste = groesste.max((1.0 - f).abs() * r * diag);
        }
        groesste
    }

    /// Wohin ein Pixel des korrigierten Bilds im aufgenommenen Bild faellt — fuer
    /// Rot, Gruen und Blau getrennt (Farbsaeume). Rechnet genau wie
    /// `quelle_gruen` und der Farbsaum-Schritt in `develop.wgsl` (ohne manuelle
    /// Entzerrung und Geraderichten). `p` in Pixeln, Bildgroesse `breite` x `hoehe`.
    pub fn quelle(&self, p: [f32; 2], breite: f32, hoehe: f32) -> [[f32; 2]; 3] {
        let mitte = [breite * 0.5, hoehe * 0.5];
        let halbdiag = (mitte[0] * mitte[0] + mitte[1] * mitte[1]).sqrt().max(1.0);
        let mut d = [p[0] - mitte[0], p[1] - mitte[1]];
        let laenge = |d: [f32; 2]| (d[0] * d[0] + d[1] * d[1]).sqrt();
        if let Some(rad) = &self.radial {
            d = [d[0] * rad.fuell, d[1] * rad.fuell];
            let m = rad.m_an(laenge(d) / halbdiag);
            d = [d[0] * m, d[1] * m];
        }
        if self.abc != [0.0; 3] {
            let r = laenge(d) / mitte[0].min(mitte[1]).max(1.0);
            let f = ptlens(self.abc, r) * self.skala;
            d = [d[0] * f, d[1] * f];
        }
        let g = [mitte[0] + d[0], mitte[1] + d[1]];
        let (mut r, mut b) = (g, g);
        if let Some(rad) = &self.radial {
            let rq = laenge(d) / halbdiag;
            let ca_r = 1.0 + Radial::lerp(&rad.r_quelle[..rad.n], &rad.ca_r[..rad.n], rq);
            let ca_b = 1.0 + Radial::lerp(&rad.r_quelle[..rad.n], &rad.ca_b[..rad.n], rq);
            r = [mitte[0] + d[0] * ca_r, mitte[1] + d[1] * ca_r];
            b = [mitte[0] + d[0] * ca_b, mitte[1] + d[1] * ca_b];
        }
        [r, g, b]
    }

    /// Ausgleich der Randabdunklung an einer Stelle des AUFGENOMMENEN Bilds
    /// (Kameradaten der RAF; ohne sie 1). Wie Schritt 2 in `develop.wgsl`.
    pub fn rand_gain(&self, q: [f32; 2], breite: f32, hoehe: f32) -> f32 {
        match &self.radial {
            Some(rad) => {
                let (hx, hy) = (breite * 0.5, hoehe * 0.5);
                let rq = ((q[0] - hx).powi(2) + (q[1] - hy).powi(2)).sqrt() / (hx * hx + hy * hy).sqrt().max(1.0);
                rad.gain_an(rq).max(0.1)
            }
            None => 1.0,
        }
    }

    /// Vergroesserung der Bildmitte durch die Korrektur (korrigiert / aufgenommen):
    /// die Brennweite in Pixeln des korrigierten Bilds ist die der Aufnahme mal
    /// diesen Wert.
    pub fn mitten_vergroesserung(&self) -> f32 {
        let mut f = 1.0f32;
        if let Some(rad) = &self.radial {
            f *= rad.fuell * rad.m_an(0.0);
        }
        if self.abc != [0.0; 3] {
            f *= ptlens(self.abc, 0.0) * self.skala;
        }
        1.0 / f.max(1e-6)
    }

    pub fn aktiv(&self) -> bool {
        self.abc != [0.0; 3] || self.devignette > 0.0 || self.radial.is_some()
    }

    pub fn quelle_text(&self) -> &'static str {
        match self.quelle {
            ObjektivQuelle::Datei => "Hersteller-Korrektur aus der Datei",
            ObjektivQuelle::Kamera => "Korrektur der Kamera",
            ObjektivQuelle::Installiert => "Installiertes Objektivprofil",
            ObjektivQuelle::Datenbank => "Objektivprofil aus der Datenbank",
            ObjektivQuelle::Profil => PROFILE[0].name,
            ObjektivQuelle::Modul => "Telefonkamera (generisch)",
            ObjektivQuelle::Brennweite => "Schätzung aus der Brennweite",
            ObjektivQuelle::Keine => "kein Profil",
        }
    }

    /// Loest die Profilwahl aus dem Menue auf: "" = das automatisch Erkannte,
    /// sonst die feste Wahl. Randabdunklung bleibt immer die erkannte —
    /// gemessen oder aus der Datei, beides gilt fuer DIESES Foto.
    pub fn gewaehlt(&self, wahl: &str, breite: u32, hoehe: u32) -> Objektiv {
        let abc = match wahl {
            "" => return *self,
            "b061" => interpoliere(REISEZOOM_B061, self.brennweite.clamp(18.0, 300.0)),
            "iphone_haupt" => TELEFON_HAUPT, // Kennung: gespeicherter Schluessel
            "iphone_uw" => TELEFON_UW, // Kennung: gespeicherter Schluessel
            "iphone_tele" => TELEFON_TELE, // Kennung: gespeicherter Schluessel
            "schaetzung" => heuristik(if self.brennweite_kb > 0.0 {
                self.brennweite_kb
            } else {
                self.brennweite
            }),
            _ => return *self,
        };
        // Eine feste Wahl ersetzt nur die VERZEICHNUNG — Farbsaeume und
        // Vignettierung aus den Kameradaten gelten weiter fuer dieses Foto.
        let radial = self.radial.map(|mut r| {
            for i in 0..r.n {
                r.m[i] = 1.0;
                r.r_ziel[i] = r.r_quelle[i];
            }
            r.fuell = 1.0;
            r
        });
        Objektiv {
            abc,
            skala: fuellmassstab(abc, breite, hoehe),
            quelle: match wahl {
                "b061" => ObjektivQuelle::Profil,
                "schaetzung" => ObjektivQuelle::Brennweite,
                _ => ObjektivQuelle::Modul,
            },
            radial,
            ..*self
        }
    }
}

/// Stuetzstellen linear ueber die Brennweite interpolieren.
fn interpoliere(stuetzen: &[Stuetze], brennweite: f32) -> [f32; 3] {
    let erste = stuetzen[0];
    let letzte = stuetzen[stuetzen.len() - 1];
    if brennweite <= erste.0 {
        return [erste.1, erste.2, erste.3];
    }
    if brennweite >= letzte.0 {
        return [letzte.1, letzte.2, letzte.3];
    }
    for paar in stuetzen.windows(2) {
        let (f0, a0, b0, c0) = paar[0];
        let (f1, a1, b1, c1) = paar[1];
        if brennweite <= f1 {
            let t = (brennweite - f0) / (f1 - f0);
            return [a0 + (a1 - a0) * t, b0 + (b1 - b0) * t, c0 + (c1 - c0) * t];
        }
    }
    [0.0; 3]
}

/// Ohne Profil: die bewusst dezente Brennweiten-Heuristik der Web-Version
/// (`estimateDistortionFromFocal`), umgerechnet in einen einfachen kubischen
/// Anteil. 50–85 mm KB ergibt bewusst 0.
fn heuristik(brennweite_kb: f32) -> [f32; 3] {
    if !brennweite_kb.is_finite() || brennweite_kb <= 0.0 {
        return [0.0; 3];
    }
    // Web-Reglerwert -100..100 → dort k = wert/100 · 0,35 im r²-Modell.
    let wert: f32 = if brennweite_kb <= 16.0 {
        55.0
    } else if brennweite_kb <= 24.0 {
        38.0
    } else if brennweite_kb <= 35.0 {
        20.0
    } else if brennweite_kb <= 50.0 {
        8.0
    } else if brennweite_kb <= 85.0 {
        0.0
    } else if brennweite_kb <= 135.0 {
        -6.0
    } else {
        -10.0
    };
    if wert == 0.0 {
        return [0.0; 3];
    }
    // In ptlens uebersetzt: reiner b-Anteil (r²), d gleicht am Rand aus.
    // Tonne beheben (wert > 0) heisst am Rand nach innen abtasten → b < 0.
    [0.0, -wert / 100.0 * 0.10, 0.0]
}

/// Verzeichnungskoeffizienten fuer ein Objektiv bei einer Brennweite.
pub fn verzeichnung(lens: &str, brennweite: f32, crop: f32) -> ([f32; 3], ObjektivQuelle) {
    let l = lens.to_ascii_lowercase();
    for p in PROFILE {
        if l.contains(&p.kennung.to_ascii_lowercase()) {
            return (interpoliere(p.stuetzen, brennweite), ObjektivQuelle::Profil);
        }
    }
    let abc = heuristik(brennweite * crop);
    if abc == [0.0; 3] {
        (abc, ObjektivQuelle::Keine)
    } else {
        (abc, ObjektivQuelle::Brennweite)
    }
}

/// p(r) des ptlens-Modells.
fn ptlens(abc: [f32; 3], r: f32) -> f32 {
    let [a, b, c] = abc;
    let d = 1.0 - a - b - c;
    a * r * r * r + b * r * r + c * r + d
}

/// Fuellmassstab: nirgends am Rand darf weiter aussen abgetastet werden, als
/// das Bild reicht — sonst schmieren die Raender (Web-Falle v6.1.0). Der Rand
/// wird numerisch abgelaufen; zurueck kommt der Teiler fuer die Abtastung.
pub fn fuellmassstab(abc: [f32; 3], breite: u32, hoehe: u32) -> f32 {
    if abc == [0.0; 3] {
        return 1.0;
    }
    let (hw, hh) = (breite as f32 / 2.0, hoehe as f32 / 2.0);
    let norm = hw.min(hh).max(1.0);
    let mut max_f = 1.0f32;
    // Rand des Bildkastens in 1%-Schritten ablaufen.
    for i in 0..=100 {
        let t = i as f32 / 100.0;
        for (x, y) in [
            (hw * (2.0 * t - 1.0), hh),  // obere/untere Kante
            (hw, hh * (2.0 * t - 1.0)),  // linke/rechte Kante
        ] {
            let r = (x * x + y * y).sqrt() / norm;
            max_f = max_f.max(ptlens(abc, r));
        }
    }
    1.0 / max_f
}

/// Randabdunklung am Bild messen — 1:1 die Logik der Web-Version
/// (`measureVignetteAmount`): Ecken gegen Mitte auf der gamma-gewichteten
/// Helligkeit, konservative Schwelle, Deckel 70.
pub fn vignette_messen(img: &rawcore::RawImage) -> f32 {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 32 || h < 32 {
        return 0.0;
    }
    let feld_w = (w as f32 * 0.16) as usize;
    let feld_h = (h as f32 * 0.16) as usize;
    let schritt = (feld_w * feld_h / 4000).max(1);
    let mittel = |x0: usize, y0: usize| -> f32 {
        let mut s = 0.0f32;
        let mut n = 0.0f32;
        let mut i = 0usize;
        for y in y0..(y0 + feld_h).min(h) {
            for x in x0..(x0 + feld_w).min(w) {
                i += 1;
                if i % schritt != 0 {
                    continue;
                }
                let p = (y * w + x) * 3;
                let lum = 0.2126 * img.pixels[p] as f32
                    + 0.7152 * img.pixels[p + 1] as f32
                    + 0.0722 * img.pixels[p + 2] as f32;
                // Wie im Web auf der Anzeige-Achse vergleichen, nicht linear —
                // sonst passen die Schwellen (0,90 / 0,55) nicht.
                s += (lum / 65535.0).max(0.0).powf(1.0 / 2.2);
                n += 1.0;
            }
        }
        if n > 0.0 {
            s / n
        } else {
            0.0
        }
    };
    let mitte = mittel(w / 2 - feld_w / 2, h / 2 - feld_h / 2);
    if mitte < 10.0 / 255.0 {
        return 0.0; // zu dunkel, Messung unzuverlaessig
    }
    let ecken = [
        mittel(0, 0),
        mittel(w - feld_w, 0),
        mittel(0, h - feld_h),
        mittel(w - feld_w, h - feld_h),
    ];
    // Eine Objektiv-Vignette trifft ALLE vier Ecken gleich — ein Szenenschatten
    // (Baum oben links, Wiese unten) nicht. Deshalb zaehlt die HELLSTE Ecke:
    // sie ist am wenigsten von der Szene verdunkelt und damit die ehrlichste
    // Schaetzung der Vignette. Gemessen an DSCF1532: der Mittelwert aller Ecken
    // haette +75 Aufhellung ergeben und die dunkle Baumecke plattgemacht —
    // die Kamera selbst laesst sie dunkel.
    let hellste = ecken.iter().cloned().fold(0.0f32, f32::max) / mitte;
    let dunkelste = ecken.iter().cloned().fold(f32::MAX, f32::min) / mitte;
    if hellste >= 0.90 {
        return 0.0;
    }
    // Sind die Ecken stark unterschiedlich, dominiert die Szene — Finger weg.
    if hellste - dunkelste > 0.25 {
        return 0.0;
    }
    // 0,7-Daempfung wie in der Web-Version: lieber etwas Rest-Vignette als
    // aufgehellte Bildinhalte.
    (((0.90 - hellste) / (0.90 - 0.55) * 100.0) * 0.7)
        .clamp(0.0, 70.0)
        .round()
}

/// Die Herstellerdaten einer DNG (WarpRectilinear/FixVignetteRadial) in unser
/// ptlens-Modell uebersetzen.
///
/// Beide Modelle multiplizieren den Radius mit einem Polynom — nur die Basis
/// unterscheidet sich (gerade Potenzen r²/r⁴/r⁶ dort, kubisch hier) und die
/// Normierung (weitester Eckabstand dort, kleinere Halbseite hier). Deshalb:
/// kleinste Quadrate ueber den ganzen Bildbereich. Der Fit wird auf p(1) = 1
/// normiert — das aendert nur den Massstab (reines Zoomen verbiegt keine
/// Geraden), und den faengt der Fuellmassstab ohnehin ab.
pub fn aus_dng(dw: &rawcore::dng::DngWarp, breite: u32, hoehe: u32, gemessen: f32) -> Objektiv {
    let mut abc = [0.0f32; 3];
    if let Some(wk) = &dw.warp {
        abc = warp_fit(wk, breite, hoehe);
    }
    // Randabdunklung: die Datei weiss es exakt — der Gewinn am weitesten Rand
    // (r = 1, also einfach die Summe der k), auf die Anzeige-Achse (Gamma 2,2)
    // gebracht und in dieselben Regler-Einheiten uebersetzt wie die Messung.
    let devignette = match &dw.vignette {
        Some(kv) => {
            let g = 1.0 + kv.iter().sum::<f64>();
            if g > 1.0 && g.is_finite() {
                let ratio = (1.0 / g).powf(1.0 / 2.2);
                (((0.90 - ratio) / (0.90 - 0.55) * 100.0) * 0.7).clamp(0.0, 70.0) as f32
            } else {
                0.0
            }
        }
        // Fertig entwickelte Rohdaten (lineare Telefon-DNG): Randabdunklung ist schon
        // eingerechnet — die Messung am Bild wuerde nur Szene sehen.
        None if dw.linear => 0.0,
        None => gemessen,
    };
    Objektiv {
        abc,
        skala: fuellmassstab(abc, breite, hoehe),
        devignette,
        quelle: ObjektivQuelle::Datei,
        brennweite: 0.0,
        brennweite_kb: 0.0,
        radial: None,
    }
}

/// Den WarpRectilinear einer DNG in ptlens-Koeffizienten uebersetzen
/// (kleinste Quadrate, siehe `aus_dng`).
fn warp_fit(wk: &rawcore::dng::WarpKoeff, breite: u32, hoehe: u32) -> [f32; 3] {
    let (w, h) = (breite as f64, hoehe as f64);
    let (cx, cy) = (wk.mitte[0].clamp(0.0, 1.0) * w, wk.mitte[1].clamp(0.0, 1.0) * h);
    // Weitester Eckabstand vom optischen Zentrum — die DNG-Normierung.
    let maxd = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .iter()
        .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
        .fold(0.0f64, f64::max)
        .max(1.0);
    let minhalb = (w.min(h) / 2.0).max(1.0);
    let f = |rd: f64| {
        let r2 = rd * rd;
        wk.kr[0] + wk.kr[1] * r2 + wk.kr[2] * r2 * r2 + wk.kr[3] * r2 * r2 * r2
    };
    // Von unserer Normierung (kleinere Halbseite) in die der DNG umrechnen.
    let k = minhalb / maxd;
    let bezug = f(k);
    let mut abc = [0.0f32; 3];
    if bezug.is_finite() && bezug > 0.1 {
        let ziel = |r: f64| f(k * r) / bezug;
        // Kleinste Quadrate: p(r) − 1 = a(r³−1) + b(r²−1) + c(r−1).
        let rmax = maxd / minhalb;
        let basen = |r: f64| [r * r * r - 1.0, r * r - 1.0, r - 1.0];
        let mut m = [[0.0f64; 3]; 3];
        let mut v = [0.0f64; 3];
        let mut abweichung = 0.0f64;
        for i in 1..=80 {
            let r = rmax * i as f64 / 80.0;
            let y = ziel(r) - 1.0;
            abweichung = abweichung.max(y.abs());
            let b = basen(r);
            for zeile in 0..3 {
                v[zeile] += b[zeile] * y;
                for spalte in 0..3 {
                    m[zeile][spalte] += b[zeile] * b[spalte];
                }
            }
        }
        // Praktisch keine Verzeichnung? Dann auch keinen Warp anwerfen.
        if abweichung > 5e-4 {
            let det = |m: &[[f64; 3]; 3]| {
                m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                    - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                    + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
            };
            let d = det(&m);
            if d.abs() > 1e-12 {
                let mut geloest = [0.0f64; 3];
                for sp in 0..3 {
                    let mut ms = m;
                    for zeile in 0..3 {
                        ms[zeile][sp] = v[zeile];
                    }
                    geloest[sp] = det(&ms) / d;
                }
                if geloest.iter().all(|g| g.is_finite() && g.abs() < 2.0) {
                    abc = [geloest[0] as f32, geloest[1] as f32, geloest[2] as f32];
                }
            }
        }
    }
    abc
}

/// Kameramodul eines Telefons aus Brennweite und Blende bestimmen (die EXIF-Zeile
/// nennt das Modul nicht beim Namen). Ultraweitwinkel liegen alle unter 3 mm;
/// Haupt- und Telekameras ueberlappen in der Brennweite, trennen sich aber
/// sauber an der Blende (Haupt f/1,5–1,9 · Tele ab f/2).
fn telefon_modul(brennweite: f32, blende: f32) -> ([f32; 3], f32) {
    if brennweite > 0.0 && brennweite < 3.0 {
        (TELEFON_UW, 13.0)
    } else if blende >= 2.05 {
        (TELEFON_TELE, 65.0)
    } else {
        (TELEFON_HAUPT, 26.0)
    }
}

/// Alles fuer ein konkretes Foto zusammensetzen.
pub fn erkennen(img: &rawcore::RawImage) -> Objektiv {
    // RAF: die Kamera hat die Korrektur fuer genau diese Aufnahme mitgeliefert.
    if let Some(f) = &img.meta.datei_objektiv {
        if let Some(radial) = Radial::aus_raf(f, img.width, img.height) {
            return Objektiv {
                abc: [0.0; 3],
                skala: 1.0,
                devignette: 0.0,
                quelle: ObjektivQuelle::Kamera,
                brennweite: img.meta.focal_len,
                // Aus der EXIF-Kleinbildbrennweite: die Mittelformat-Modelle
                // haben Faktor 0,79, nicht APS-C.
                brennweite_kb: if img.meta.brennweite_kb > 0.0 {
                    img.meta.brennweite_kb
                } else {
                    img.meta.focal_len * 1.534
                },
                radial: Some(radial),
            };
        }
    }
    let gemessen = vignette_messen(img);
    let telefon = img.meta.make.to_ascii_lowercase().contains("apple"); // Kennung: Herstellerkennung

    // 1) Herstellerdaten aus der Datei — exakt fuer DIESE Aufnahme, deckt
    //    jedes Telefon-Modell ab (und jede andere Kamera, die DNG-Opcodes
    //    schreibt), ganz ohne Profiltabelle.
    if let Some(dw) = &img.meta.dng_warp {
        let mut o = aus_dng(dw, img.width, img.height, gemessen);
        o.brennweite = img.meta.focal_len;
        if telefon {
            let (modul_abc, kb) = telefon_modul(img.meta.focal_len, img.meta.aperture);
            o.brennweite_kb = kb;
            // Bayer-DNG mit Vignette-Opcode, aber OHNE Warp-Opcode (das Muster
            // der Telefon-Generation 2018): Randabdunklung aus der Datei,
            // Verzeichnung aus der Modulkurve — beides zusammen, nicht entweder-oder.
            // Bei fertig entwickelten Rohdaten (lineare Telefon-DNG, `linear`) bleibt
            // abc leer: dort ist die Verzeichnung SCHON in den Pixeln.
            if dw.warp.is_none() && !dw.linear {
                o.abc = modul_abc;
                o.skala = fuellmassstab(modul_abc, img.width, img.height);
            }
        }
        return o;
    }

    // 2) Telefon ohne eingebettete Korrektur: generisches Modul statt der
    //    RAF-Heuristik — die wuerde mit dem falschen Crop-Faktor rechnen
    //    (4 mm Telefon-Hauptkamera != 6 mm Kleinbild-Fischauge).
    if telefon {
        let (abc, kb) = telefon_modul(img.meta.focal_len, img.meta.aperture);
        return Objektiv {
            abc,
            skala: fuellmassstab(abc, img.width, img.height),
            devignette: gemessen,
            quelle: ObjektivQuelle::Modul,
            brennweite: img.meta.focal_len,
            brennweite_kb: kb,
            radial: None,
        };
    }

    // Formatfaktor der Kamera: aus der EXIF-Kleinbildbrennweite, sonst aus
    // lensfun, sonst APS-C angenommen.
    let lf = crate::objektivdaten::Objektivdatenbank::mitgeliefert();
    let lf_kamera = lf.kamera(&img.meta.make, &img.meta.model);
    let crop = if img.meta.brennweite_kb > 0.0 && img.meta.focal_len > 0.0 {
        img.meta.brennweite_kb / img.meta.focal_len
    } else {
        lf_kamera.map(|k| k.crop).unwrap_or(1.534)
    };
    let objektiv_name = img.meta.lens.trim();
    if !objektiv_name.is_empty() && img.meta.focal_len > 0.0 {
        // Installiertes Objektivprofil (nur wenn eine andere Anwendung es
        // mitgebracht hat) — exakt, was die Referenz-Entwicklung fuer dieses
        // Objektiv anwendet.
        if let Some(radial) = crate::objektivdaten::lcp_radial(
            &img.meta.make,
            objektiv_name,
            img.meta.focal_len,
            img.meta.aperture,
            img.meta.focal_len * crop,
            img.width,
            img.height,
        ) {
            return Objektiv {
                abc: [0.0; 3],
                skala: 1.0,
                devignette: 0.0,
                quelle: ObjektivQuelle::Installiert,
                brennweite: img.meta.focal_len,
                brennweite_kb: img.meta.focal_len * crop,
                radial: Some(radial),
            };
        }
        // lensfun (mitgeliefert) — die offene Datenbank fuer alle.
        if let Some(o) = lf.objektiv(lf_kamera, objektiv_name) {
            if let Some(radial) = o.radial(img.meta.focal_len, img.meta.aperture, crop, img.width, img.height) {
                // Ohne Vignettierungsdaten im Profil bleibt die Randabdunklung,
                // wie sie ist — so haelt es auch die Referenz-Entwicklung.
                return Objektiv {
                    abc: [0.0; 3],
                    skala: 1.0,
                    devignette: 0.0,
                    quelle: ObjektivQuelle::Datenbank,
                    brennweite: img.meta.focal_len,
                    brennweite_kb: img.meta.focal_len * crop,
                    radial: Some(radial),
                };
            }
        }
    }

    // Ein fest eingebautes Profil (am Objektivnamen erkannt) gilt; eine
    // Schaetzung aus der Brennweite dagegen nur bei RAF-Kameras, wo sie gegen
    // echte Fotos abgeglichen ist. Bei fremden Objektiven raet sie oft
    // daneben — die Referenz-Entwicklung korrigiert ohne Profil gar nicht,
    // Fotowache jetzt auch nicht. (Die Schaetzung bleibt im Profilmenue waehlbar.)
    let (abc, quelle) = verzeichnung(&img.meta.lens, img.meta.focal_len, crop);
    let raf_kamera = img.meta.make.to_ascii_lowercase().contains("fuji"); // Kennung: Herstellerkennung
    if quelle == ObjektivQuelle::Profil || raf_kamera {
        return Objektiv {
            abc,
            skala: fuellmassstab(abc, img.width, img.height),
            devignette: gemessen,
            quelle,
            brennweite: img.meta.focal_len,
            brennweite_kb: img.meta.focal_len * crop,
            radial: None,
        };
    }
    Objektiv {
        abc: [0.0; 3],
        skala: 1.0,
        devignette: 0.0,
        quelle: ObjektivQuelle::Keine,
        brennweite: img.meta.focal_len,
        brennweite_kb: img.meta.focal_len * crop,
        radial: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profil_wird_am_namen_erkannt() {
        let (abc, quelle) = verzeichnung("18-300mm F/3.5-6.3 DiIII-A VC VXD B061X", 18.0, 1.534);
        assert_eq!(quelle, ObjektivQuelle::Profil);
        assert!((abc[0] - 0.02805).abs() < 1e-5);
    }

    #[test]
    fn cpu_abbildung_rechnet_wie_das_ptlens_modell() {
        let abc = [0.01, -0.03, 0.0];
        let o = Objektiv { abc, skala: fuellmassstab(abc, 600, 400), ..Objektiv::default() };
        // Die Mitte bleibt, wo sie ist; ohne Farbsaum-Daten alle Kanaele gleich.
        let m = o.quelle([300.0, 200.0], 600.0, 400.0);
        assert!((m[1][0] - 300.0).abs() < 1e-4 && (m[1][1] - 200.0).abs() < 1e-4);
        assert_eq!(m[0], m[1]);
        // Ein Randpunkt: Abstand = Zielabstand · p(r) · Fuellmassstab, r auf die kleinere Halbseite.
        let q = o.quelle([500.0, 200.0], 600.0, 400.0)[1];
        let soll = 200.0 * ptlens(abc, 200.0 / 200.0) * o.skala;
        assert!((q[0] - 300.0 - soll).abs() < 1e-3, "{} {}", q[0] - 300.0, soll);
        // Vergroesserung der Mitte passt zur Abbildung kleiner Abstaende.
        let klein = o.quelle([301.0, 200.0], 600.0, 400.0)[1][0] - 300.0;
        assert!((1.0 / klein - o.mitten_vergroesserung()).abs() < 1e-3);
        // Ohne Profil: unveraendert, kein Randausgleich.
        let leer = Objektiv::default();
        assert_eq!(leer.quelle([12.0, 34.0], 600.0, 400.0)[2], [12.0, 34.0]);
        assert_eq!(leer.rand_gain([0.0, 0.0], 600.0, 400.0), 1.0);
    }

    #[test]
    fn zwischen_stuetzstellen_wird_interpoliert() {
        // 22,5 mm liegt genau zwischen 18 und 27.
        let abc = interpoliere(REISEZOOM_B061, 22.5);
        assert!((abc[0] - (0.02805 + 0.01683) / 2.0).abs() < 1e-5);
        // Ausserhalb wird festgehalten, nicht weitergesponnen.
        assert_eq!(interpoliere(REISEZOOM_B061, 400.0), interpoliere(REISEZOOM_B061, 300.0));
    }

    #[test]
    fn unbekanntes_objektiv_faellt_auf_brennweite_zurueck() {
        let (abc, quelle) = verzeichnung("35mm F1.4 R", 35.0, 1.534);
        // 35 mm × 1,534 ≈ 54 mm KB → im Null-Fenster der Heuristik.
        assert_eq!(quelle, ObjektivQuelle::Keine);
        assert_eq!(abc, [0.0; 3]);
        let (abc, quelle) = verzeichnung("16mm F2.8", 16.0, 1.534);
        assert_eq!(quelle, ObjektivQuelle::Brennweite);
        assert!(abc[1] < 0.0, "Weitwinkel: Tonne beheben → am Rand nach innen");
    }

    #[test]
    fn fuellmassstab_verhindert_abtasten_ausserhalb() {
        // Kissen-Korrektur (p > 1 am Rand) — genau der Fall, der im Web die
        // Raender verschmierte.
        let abc = [-0.01212, 0.06048, -0.07478]; // Reisezoom (B061) bei 38 mm
        let s = fuellmassstab(abc, 3000, 2000);
        assert!(s < 1.0, "muss zuschneiden, war {s}");
        // Nach Anwendung darf am Rand nichts mehr ueberstehen:
        let norm = 1000.0f32;
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            for (x, y) in [(1500.0 * (2.0 * t - 1.0), 1000.0), (1500.0, 1000.0 * (2.0 * t - 1.0))] {
                let r = (x * x + y * y as f32).sqrt() / norm;
                assert!(ptlens(abc, r) * s <= 1.0 + 1e-4);
            }
        }
        // Ohne Verzeichnung kein Zuschnitt:
        assert_eq!(fuellmassstab([0.0; 3], 3000, 2000), 1.0);
    }

    fn testbild(vignette: bool) -> rawcore::RawImage {
        let (w, h) = (128u32, 96u32);
        let mut px = vec![0u16; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f32 - w as f32 / 2.0) / (w as f32 / 2.0);
                let dy = (y as f32 - h as f32 / 2.0) / (h as f32 / 2.0);
                let r2 = (dx * dx + dy * dy) / 2.0;
                let hell = if vignette { 0.5 * (1.0 - 0.75 * r2) } else { 0.5 };
                let v = (hell.max(0.0) * 65535.0) as u16;
                let i = ((y * w + x) * 3) as usize;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
            }
        }
        rawcore::RawImage {
            width: w,
            height: h,
            pixels: px,
            meta: rawcore::RawMeta {
                make: "Test".into(),
                model: "Vignette".into(),
                muster_6x6: false,
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
        }
    }

    #[test]
    fn vignette_wird_gemessen_aber_nicht_erfunden() {
        let mit = vignette_messen(&testbild(true));
        assert!(mit > 10.0, "deutliche Randabdunklung muss auffallen, war {mit}");
        assert!(mit <= 70.0);
        let ohne = vignette_messen(&testbild(false));
        assert_eq!(ohne, 0.0, "gleichmaessiges Bild darf keine Korrektur ausloesen");
    }

    /// Die Hersteller-Korrektur einer DNG (gerade Potenzen, Eck-Normierung)
    /// muss nach dem Fit im ptlens-Modell (kubisch, Halbseiten-Normierung)
    /// praktisch dieselbe Abbildung ergeben — sonst korrigieren wir etwas
    /// anderes, als der Hersteller gemessen hat.
    fn warp(kr: [f64; 4]) -> rawcore::dng::DngWarp {
        rawcore::dng::DngWarp {
            warp: Some(rawcore::dng::WarpKoeff {
                kr,
                kt: [0.0; 2],
                mitte: [0.5, 0.5],
            }),
            vignette: None,
            linear: false,
        }
    }

    #[test]
    fn dng_warp_wird_ins_ptlens_modell_uebersetzt() {
        let dw = warp([1.0, -0.09, 0.02, 0.0]);
        let (w, h) = (4000u32, 3000u32);
        let o = aus_dng(&dw, w, h, 0.0);
        assert_eq!(o.quelle, ObjektivQuelle::Datei);
        assert_ne!(o.abc, [0.0; 3], "deutliche Verzeichnung muss ankommen");
        // Fit gegen das Original pruefen — ueber den ganzen Radiusbereich.
        let minhalb = 1500.0f64;
        let maxd = (2000.0f64 * 2000.0 + 1500.0 * 1500.0).sqrt();
        let f = |rd: f64| 1.0 - 0.09 * rd * rd + 0.02 * rd.powi(4);
        let bezug = f(minhalb / maxd);
        let mut schlimmste = 0.0f64;
        for i in 1..=60 {
            let r = (maxd / minhalb) * i as f64 / 60.0;
            let ziel = f(r * minhalb / maxd) / bezug;
            let ist = ptlens(o.abc, r as f32) as f64;
            schlimmste = schlimmste.max((ist - ziel).abs());
        }
        assert!(
            schlimmste < 0.004,
            "Fit weicht zu weit ab: {schlimmste}"
        );
    }

    #[test]
    fn identitaets_warp_ergibt_keine_verzeichnung() {
        let dw = warp([1.0, 0.0, 0.0, 0.0]);
        let o = aus_dng(&dw, 4000, 3000, 12.0);
        assert_eq!(o.abc, [0.0; 3]);
        assert_eq!(o.skala, 1.0);
        // Ohne Vignette-Opcode gilt die Messung am Bild weiter.
        assert_eq!(o.devignette, 12.0);
    }

    /// Fertig entwickelte Rohdaten (lineare Telefon-DNG): NICHTS mehr
    /// korrigieren — Verzeichnung und Randabdunklung stecken schon in den Pixeln.
    /// Nachgemessen am Muster eines Telefons (Generation 2020): keine
    /// Opcode-Listen, LinearRaw.
    #[test]
    fn lineare_telefon_dng_wird_nicht_doppelt_korrigiert() {
        let dw = rawcore::dng::DngWarp {
            warp: None,
            vignette: None,
            linear: true,
        };
        let o = aus_dng(&dw, 4000, 3000, 40.0);
        assert_eq!(o.abc, [0.0; 3]);
        assert_eq!(o.devignette, 0.0, "gemessene Szene darf hier nicht einfliessen");
        assert_eq!(o.quelle, ObjektivQuelle::Datei);
    }

    #[test]
    fn datei_vignette_schlaegt_die_messung() {
        let dw = rawcore::dng::DngWarp {
            warp: Some(rawcore::dng::WarpKoeff {
                kr: [1.0, 0.0, 0.0, 0.0],
                kt: [0.0; 2],
                mitte: [0.5, 0.5],
            }),
            vignette: Some([0.5, 0.0, 0.0, 0.0, 0.0]),
            linear: false,
        };
        let o = aus_dng(&dw, 4000, 3000, 55.0);
        // Gewinn 1,5 am Rand → auf der Anzeige-Achse ≈ 0,83 → ~14 Einheiten.
        assert!(o.devignette > 5.0 && o.devignette < 25.0, "war {}", o.devignette);
    }

    /// Telefon-Dateien OHNE eingebettete Korrektur duerfen nicht in der
    /// RAF-Heuristik landen (falscher Crop-Faktor), sondern bekommen die
    /// generische Modulkurve.
    #[test]
    fn telefon_ohne_opcodes_bekommt_modulkurve() {
        let mut bild = testbild(false);
        bild.meta.make = "Apple".into(); // Kennung: Herstellerkennung
        bild.meta.model = "Testtelefon".into();
        bild.meta.lens = "Testtelefon back triple camera 2.22mm f/2.2".into();
        bild.meta.focal_len = 2.22;
        bild.meta.aperture = 2.2;
        let o = erkennen(&bild);
        assert_eq!(o.quelle, ObjektivQuelle::Modul);
        assert_eq!(o.abc, TELEFON_UW);
        bild.meta.focal_len = 6.86;
        bild.meta.aperture = 1.78;
        assert_eq!(erkennen(&bild).abc, TELEFON_HAUPT);
        bild.meta.focal_len = 6.0;
        bild.meta.aperture = 2.8;
        assert_eq!(erkennen(&bild).abc, TELEFON_TELE);
    }

    /// Liegen Herstellerdaten in der Datei, gewinnen sie gegen alles andere.
    #[test]
    fn dateidaten_gewinnen_gegen_die_erkennung() {
        let mut bild = testbild(false);
        bild.meta.make = "Apple".into(); // Kennung: Herstellerkennung
        bild.meta.dng_warp = Some(warp([1.0, -0.09, 0.02, 0.0]));
        let o = erkennen(&bild);
        assert_eq!(o.quelle, ObjektivQuelle::Datei);
        assert_ne!(o.abc, [0.0; 3]);
    }

    /// Bayer-DNG mit Vignette-Opcode, aber ohne Warp (Muster der
    /// Telefon-Generation 2018): Vignette aus der Datei, Verzeichnung aus der
    /// Modulkurve — kombiniert.
    #[test]
    fn bayer_dng_kombiniert_datei_vignette_mit_modulkurve() {
        let mut bild = testbild(false);
        bild.meta.make = "Apple".into(); // Kennung: Herstellerkennung
        bild.meta.focal_len = 4.25;
        bild.meta.aperture = 1.8;
        bild.meta.dng_warp = Some(rawcore::dng::DngWarp {
            warp: None,
            vignette: Some([0.5, 0.0, 0.0, 0.0, 0.0]),
            linear: false,
        });
        let o = erkennen(&bild);
        assert_eq!(o.quelle, ObjektivQuelle::Datei);
        assert_eq!(o.abc, TELEFON_HAUPT, "Verzeichnung muss aus der Modulkurve kommen");
        assert!(o.devignette > 5.0, "Vignette muss aus der Datei kommen, war {}", o.devignette);
    }

    /// Die manuelle Wahl aus dem Menue ueberschreibt das Erkannte — und
    /// "" laesst es unangetastet.
    #[test]
    fn manuelle_wahl_ueberschreibt() {
        let auto = Objektiv {
            abc: [0.0; 3],
            skala: 1.0,
            devignette: 20.0,
            quelle: ObjektivQuelle::Keine,
            brennweite: 4.25,
            brennweite_kb: 26.0,
            radial: None,
        };
        assert_eq!(auto.gewaehlt("", 4000, 3000), auto);
        let haupt = auto.gewaehlt("iphone_haupt", 4000, 3000); // Kennung: gespeicherter Schluessel
        assert_eq!(haupt.abc, TELEFON_HAUPT);
        assert_eq!(haupt.quelle, ObjektivQuelle::Modul);
        // Randabdunklung bleibt die fuer DIESES Foto erkannte.
        assert_eq!(haupt.devignette, 20.0);
        // Unbekannte Kennungen fallen auf das Erkannte zurueck.
        assert_eq!(auto.gewaehlt("quatsch", 4000, 3000), auto);
    }

    /// Ein Szenenschatten (EINE dunkle Ecke, z. B. ein Baum) ist keine Vignette.
    /// Gemessen an DSCF1532: der Mittelwert aller Ecken haette +75 ergeben und
    /// helle Ecken ausgefressen — die Kamera laesst die Baumecke selbst dunkel.
    #[test]
    fn ein_szenenschatten_ist_keine_vignette() {
        let mut bild = testbild(false);
        let (w, h) = (bild.width as usize, bild.height as usize);
        for y in 0..h / 3 {
            for x in 0..w / 3 {
                let i = (y * w + x) * 3;
                bild.pixels[i] = 2000;
                bild.pixels[i + 1] = 2000;
                bild.pixels[i + 2] = 2000;
            }
        }
        assert_eq!(vignette_messen(&bild), 0.0);
    }
}
