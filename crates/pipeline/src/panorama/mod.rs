//! Panorama: mehrere ueberlappende Aufnahmen zu einem Bild zusammensetzen.
//!
//! ## Warum auf linearen Daten
//! Zusammengesetzt wird VOR jeder Tonkurve, in den linearen Kameradaten. Das
//! Ergebnis ist dadurch selbst wieder ein "Rohbild": Belichtung, Weissabgleich,
//! Tonregler und Profil wirken darauf genau wie auf eine einzelne Aufnahme, und
//! ein Belichtungsausgleich zwischen den Bildern ist eine reine Verstaerkung —
//! nach einer Tonkurve waere er das nicht mehr.
//!
//! ## Ablauf — Brown & Lowe, "Automatic Panoramic Image Stitching using
//! Invariant Features", IJCV 2007 (B&L), Algorithmus am Ende von Abschnitt 7
//! 1. Merkmale auf verkleinerten Graustufenkopien (~1500 px), mehrstufig
//!    (`merkmale`; die Arbeit nimmt SIFT, hier Ecken mit Binaerbeschreibung).
//! 2. Zuordnung aller Paare; je Bild die m = 6 Bilder mit den meisten
//!    Zuordnungen als Kandidaten (B&L Abschnitt 3). Je Kandidatenpaar RANSAC-
//!    Homographie (r = 4, n = 500), gefuehrte Zuordnung, Annahme nach Gl. 13
//!    (`paare`, `geometrie`).
//! 3. Zusammenhaengende Gruppen; die groesste wird das Panorama, der Rest wird
//!    gemeldet.
//! 4. Buendelausgleich, Bild fuer Bild (B&L Abschnitt 4, `ausrichtung`):
//!    Drehung je Bild, Brennweite je Brennweitengruppe, k1 je Optik (nur ohne
//!    Objektivprofil).
//! 5. Feinausrichtung jedes Punktpaars auf voller Aufloesung (`verfeinern`),
//!    Endrunde des Ausgleichs auf diesen Punkten.
//! 6. Begradigung (B&L Abschnitt 5, Gl. 26), Projektion (Zylinder, Kugel, Ebene).
//! 7. Parallaxe nach Zhang & Liu, CVPR 2014 (`parallaxe`): nur fuer Paare mit
//!    messbarem Restversatz; oertliche Ausrichtung per Netzverformung.
//! 8. Belichtungsausgleich (Abschnitt 6, Gl. 29) und Naehte (Startaufteilung
//!    Gl. 31, Graphenschnitt nach Kwatra 2003) auf einer kleinen Leinwand (`naht`).
//! 9. Mehrband-Mischung (Abschnitt 7, Gl. 32–38) auf voller Aufloesung, Bild
//!    fuer Bild (`mischen`).
//!
//! Fortschritt, Fehler und Bericht liefert der Kern als Daten (`meldung`), die
//! Saetze setzt die Oberflaeche in ihrer Sprache zusammen.

mod ausrichtung;
mod direkt;
mod geometrie;
mod mathe;
mod meldung;
mod merkmale;
mod mischen;
mod naht;
mod paare;
mod parallaxe;
mod projektion;
mod verfeinern;
#[cfg(test)]
mod tests;

use ausrichtung::Paar;
pub use meldung::{bericht_deutsch, Meldung, PanoramaFehler, Schritt, Verwerfgrund};
pub use parallaxe::Netz;
pub use projektion::{Abbildung, Kamera, Leinwand};
use rayon::prelude::*;

/// Ein Eingangsbild: lineare Kameradaten, RGB verschraenkt.
#[derive(Default)]
pub struct Eingabe {
    pub w: u32,
    pub h: u32,
    /// Linear, RGB verschraenkt, etwa 0..1.
    pub rgb: Vec<f32>,
    /// Brennweite in Pixeln dieses Bildes (aus den Metadaten), falls bekannt.
    pub brennweite_px: Option<f32>,
    /// Die Objektivverzeichnung ist bereits herausgerechnet (Objektivprofil
    /// der App-Seite) — dann wird keine Verzeichnung geschaetzt (k1 = 0 fest).
    pub entzerrt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projektion {
    /// Senkrechte bleiben senkrecht; fuer waagrechte Reihen bis 360 Grad.
    Zylinder,
    /// Gleichwinklig; fuer mehrzeilige Raster und grosse senkrechte Winkel.
    Kugel,
    /// Gerade Linien bleiben gerade; nur fuer kleinere Bildwinkel (< ~120 Grad).
    Eben,
}

pub struct Optionen {
    pub projektion: Projektion,
    pub belichtung_angleichen: bool,
    /// Laengere Ausgabeseite hoechstens (z. B. 30000).
    pub max_ausgabe_seite: u32,
    /// Parallaxe ausgleichen: Restversatz naher Dinge in den Ueberlappungen
    /// (Freihand, Standortwechsel) oertlich angleichen. Standard: an.
    pub parallaxe_ausgleichen: bool,
}

impl Default for Optionen {
    fn default() -> Self {
        Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true }
    }
}

pub struct Ergebnis {
    pub w: u32,
    pub h: u32,
    /// Linear, RGB verschraenkt — wie die Eingaben.
    pub rgb: Vec<f32>,
    /// 255 = Bilddaten, 0 = leer.
    pub deckung: Vec<u8>,
    /// Bericht als Daten (verwendete/verworfene Bilder, Ausrichtung, Brennweite,
    /// Parallaxe …); deutscher Text ueber `bericht_deutsch`.
    pub bericht: Vec<Meldung>,
    /// Indizes der verwendeten Eingaben (in Eingabereihenfolge).
    pub verwendet: Vec<usize>,
    /// Belichtungs-Verstaerkung je verwendetem Bild (gleiche Reihenfolge wie `verwendet`).
    pub verstaerkungen: Vec<f32>,
    /// Ermittelte Brennweite in Pixeln der Eingaben (Median ueber die Bilder).
    pub brennweite_px: f32,
    /// Mittlerer Ausrichtungsfehler in Pixeln der Eingaben.
    pub fehler_px: f32,
    /// Ermittelte radiale Verzeichnung (x * (1 + k1 r^2), r relativ zur halben
    /// Bilddiagonale; negativ = Tonne). Sie ist im Ergebnis bereits ausgeglichen —
    /// eine Objektivkorrektur darf auf das Panorama NICHT noch einmal wirken.
    /// 0, wenn die Eingaben schon entzerrt waren.
    pub verzeichnung_k1: f32,
    /// Geometrie je verwendetem Bild (gleiche Reihenfolge wie `verwendet`),
    /// in Pixeln der Eingaben.
    pub kameras: Vec<Kamera>,
    /// Ausgabeflaeche (Projektion, Massstab, Ursprung, Groesse).
    pub leinwand: Leinwand,
    /// Aufteilung der Ausgabe auf die Bilder (Nahtverlauf), verkleinert.
    pub naht: Nahtkarte,
    /// Parallaxe-Verformung je verwendetem Bild (None = nur Teil-A-Geometrie).
    pub verformungen: Vec<Option<Netz>>,
}

impl Ergebnis {
    /// Stelle auf der Ausgabe (Pixel) des Bildpunkts (x, y) (Pixel, Mitte bei
    /// +0,5) des verwendeten Bildes `idx` — samt Parallaxe-Verformung.
    pub fn bild_zu_leinwand(&self, idx: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let p = self.leinwand.stelle(&self.kameras[idx].strahl(x, y))?;
        Some(match &self.verformungen[idx] {
            Some(n) => n.vor(p.0, p.1),
            None => p,
        })
    }

    /// Bildpunkt des verwendeten Bildes `idx` an der Ausgabestelle (x, y).
    pub fn leinwand_zu_bild(&self, idx: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let (x, y) = match &self.verformungen[idx] {
            Some(n) => n.zurueck(x, y),
            None => (x, y),
        };
        naht::quelle_f(&self.leinwand, &self.kameras[idx], x, y)
    }
}

/// Welches Bild jedes Pixel einer verkleinerten Ausgabe liefert: Index in
/// `Ergebnis::verwendet`, `u16::MAX` = leer. Pixel (x, y) der Karte entspricht
/// der Stelle ((x + 0,5) / massstab, (y + 0,5) / massstab) der Ausgabe.
#[derive(Clone, Debug, Default)]
pub struct Nahtkarte {
    pub w: usize,
    pub h: usize,
    pub massstab: f64,
    pub bild: Vec<u16>,
}

/// Merkmalskopie: laengere Kante etwa so viele Pixel.
const MERKMAL_KANTE: f64 = 1500.0;
const MERKMAL_ANZAHL: usize = 3000;
/// Obergrenze der Ausgabeflaeche. Beim Mischen braucht jedes Ausgabepixel etwa
/// 21 Byte (Summenpyramide mit Gewichten) — 200 MP sind rund 4 GB.
const MAX_PIXEL: f64 = 200.0e6;
/// Kleine Leinwand fuer Naht und Belichtung. Die Referenzumsetzung nimmt
/// 0,1 MP. Gemessen (6 Reihen, Farbunterschied entlang der Naht / Zeit des
/// Kerns): 1,2 MP am saubersten, aber bei fast deckungsgleichen Bildern
/// langsam (2565–2569: 4,4 bei 36,9 s); 0,3 MP: 6,0 bei 3,4 s; 0,1 MP: 7,5 bei
/// 2,0 s. Mit 0,3 MP bleibt der Nahtunterschied in allen sechs Reihen unter
/// dem bisherigen Stand (2565–2569 dort 10,8) bei einem Zehntel der Zeit.
const NAHT_PIXEL: f64 = 0.3e6;
/// Kandidaten je Bild fuer die Paarpruefung (B&L Abschnitt 3: m = 6).
const KANDIDATEN_JE_BILD: usize = 6;
/// Huber-Schwelle der Endloesung in Pixeln (B&L Abschnitt 4: σ = 2).
const HUBER_ENDE: f64 = 2.0;

/// Setzt die Bilder zu einem Panorama zusammen. `fortschritt(anteil 0..1, schritt)`.
pub fn zusammensetzen(bilder: &[Eingabe], opt: &Optionen, fortschritt: &dyn Fn(f32, Schritt)) -> Result<Ergebnis, PanoramaFehler> {
    zusammensetzen_intern(bilder, opt, fortschritt)
}

/// Messausgabe fuer die Fehlersuche an echten Reihen (Punkte, Zuordnungen,
/// Inlier je Paar) — nur mit gesetzter Umgebungsvariable `PANORAMA_DIAGNOSE`.
pub(super) fn diagnose(text: &str) {
    use std::sync::OnceLock;
    static AN: OnceLock<bool> = OnceLock::new();
    if *AN.get_or_init(|| std::env::var_os("PANORAMA_DIAGNOSE").is_some()) {
        eprintln!("[panorama] {text}");
    }
}

/// Leinwandstelle (volle Leinwand) -> Quellpunkt eines Bildes.
type Abbildungsfn = Box<dyn Fn(f64, f64) -> Option<(f64, f64)> + Sync>;

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

pub(crate) fn zusammensetzen_intern(bilder: &[Eingabe], opt: &Optionen, fortschritt: &dyn Fn(f32, Schritt)) -> Result<Ergebnis, PanoramaFehler> {
    let n = bilder.len();
    if n < 2 {
        return Err(PanoramaFehler::ZuWenigBilder);
    }
    for (k, b) in bilder.iter().enumerate() {
        if b.w < 64 || b.h < 64 || b.rgb.len() != b.w as usize * b.h as usize * 3 {
            return Err(PanoramaFehler::UngueltigeAbmessungen(k));
        }
    }

    // ---- 1. Merkmale ------------------------------------------------------
    fortschritt(0.0, Schritt::Merkmale);
    let max_kante = bilder.iter().map(|b| b.w.max(b.h)).max().unwrap_or(1) as f64;
    let s = (MERKMAL_KANTE / max_kante).min(1.0);
    struct Klein {
        rgb: Vec<f32>,
        w: usize,
        h: usize,
        grau: merkmale::Grau,
        merkmale: merkmale::Merkmale,
    }
    let klein: Vec<Klein> = bilder
        .par_iter()
        .map(|b| {
            let (rgb, w, h) = merkmale::verkleinern(&b.rgb, b.w as usize, b.h as usize, 3, s);
            let g = merkmale::grau_aus_rgb(&rgb, w, h);
            let m = merkmale::finden_mehrstufig(&g, MERKMAL_ANZAHL);
            Klein { rgb, w, h, grau: g, merkmale: m }
        })
        .collect();
    for (k, kl) in klein.iter().enumerate() {
        diagnose(&format!("Bild {}: {}x{} Merkmalskopie, {} Punkte", k + 1, kl.w, kl.h, kl.merkmale.punkte.len()));
    }
    // Zentrierte Koordinaten: Hauptpunkt = Bildmitte der VOLLEN Aufnahme, skaliert.
    let mitte = |k: usize| (s * bilder[k].w as f64 / 2.0, s * bilder[k].h as f64 / 2.0);

    // ---- 2. Paare ---------------------------------------------------------
    fortschritt(0.12, Schritt::Zuordnen);
    let seiten: Vec<paare::Seite> = (0..n)
        .map(|k| paare::Seite { m: &klein[k].merkmale, grau: &klein[k].grau, mitte: mitte(k), w: klein[k].w as f64, h: klein[k].h as f64 })
        .collect();
    let paar_idx: Vec<(usize, usize)> = (0..n).flat_map(|i| (i + 1..n).map(move |j| (i, j))).collect();
    let roh: Vec<Vec<(usize, usize)>> = paar_idx.par_iter().map(|&(i, j)| merkmale::zuordnen(&klein[i].merkmale, &klein[j].merkmale)).collect();
    let mut zuordnungen = vec![0usize; n * n];
    for (k, &(i, j)) in paar_idx.iter().enumerate() {
        zuordnungen[i * n + j] = roh[k].len();
        zuordnungen[j * n + i] = roh[k].len();
    }
    // Kandidaten: je Bild die m Bilder mit den meisten Zuordnungen (B&L Abschnitt 3).
    let mut kandidat = vec![false; paar_idx.len()];
    for i in 0..n {
        let mut partner: Vec<usize> = (0..n).filter(|&j| j != i && zuordnungen[i * n + j] > 0).collect();
        partner.sort_by_key(|&j| (std::cmp::Reverse(zuordnungen[i * n + j]), j));
        for &j in partner.iter().take(KANDIDATEN_JE_BILD) {
            let (a, b) = (i.min(j), i.max(j));
            let k = paar_idx.iter().position(|&p| p == (a, b)).unwrap();
            kandidat[k] = true;
        }
    }
    let pruefung: Vec<(usize, usize, paare::Pruefung)> = paar_idx
        .par_iter()
        .enumerate()
        .filter(|(k, _)| kandidat[*k])
        .map(|(k, &(i, j))| (i, j, paare::standard(i, j, &seiten[i], &seiten[j], &roh[k])))
        .collect();
    let mut bester_treffer = vec![0usize; n];
    let mut paare: Vec<Paar> = Vec::new();
    for (i, j, p) in pruefung {
        diagnose(&format!(
            "Paar {}-{}: {} Zuordnungen, {} Inlier, {} Merkmale in der Ueberlappung{}",
            i + 1, j + 1, p.zuordnungen, p.inlier, p.ueberlappung, if p.paar.is_some() { " -> angenommen" } else { "" }
        ));
        bester_treffer[i] = bester_treffer[i].max(p.inlier);
        bester_treffer[j] = bester_treffer[j].max(p.inlier);
        if let Some(p) = p.paar {
            paare.push(p);
        }
    }
    rueckfall(&paar_idx, &zuordnungen, &seiten, bilder, s, &klein.iter().map(|k| k.w.max(k.h)).collect::<Vec<_>>(), &mut paare, &mut bester_treffer);
    let mut verworfen: Vec<(usize, Verwerfgrund)> = Vec::new();
    let mitglieder = ausrichtung::groesste_gruppe(n, &paare);
    for (k, &treffer) in bester_treffer.iter().enumerate() {
        if mitglieder.contains(&k) {
            continue;
        }
        let hat_paar = paare.iter().any(|p| p.i == k || p.j == k);
        verworfen.push((k, if hat_paar { Verwerfgrund::ZweiteGruppe } else { Verwerfgrund::KeinPartner { hoechstens_punkte: treffer } }));
    }
    if mitglieder.len() < 2 {
        return Err(PanoramaFehler::KeineUeberlappung);
    }
    paare.retain(|p| mitglieder.contains(&p.i) && mitglieder.contains(&p.j));

    // ---- 3. Kameramodell und Buendelausgleich -----------------------------
    fortschritt(0.3, Schritt::Ausrichten);
    // Brennweitengruppen: gleiche Metadaten-Brennweite = eine Gruppe; ohne
    // Metadaten eine je Bild (B&L). Optikgruppen (k1): vorab entzerrt = fest 0;
    // sonst je Metadaten-Brennweite bzw. alle ohne Metadaten gemeinsam.
    let mut schluessel_f: Vec<Option<u32>> = Vec::new();
    let mut gruppe_f = vec![0usize; n];
    let mut schluessel_k: Vec<(bool, Option<u32>)> = Vec::new();
    let mut gruppe_k = vec![0usize; n];
    for k in 0..n {
        let meta = bilder[k].brennweite_px.filter(|f| *f > 0.0).map(|f| f.to_bits());
        gruppe_f[k] = match meta {
            Some(key) => match schluessel_f.iter().position(|s| *s == Some(key)) {
                Some(g) => g,
                None => {
                    schluessel_f.push(Some(key));
                    schluessel_f.len() - 1
                }
            },
            None => {
                schluessel_f.push(None);
                schluessel_f.len() - 1
            }
        };
        let key_k = (bilder[k].entzerrt, if bilder[k].entzerrt { None } else { meta });
        gruppe_k[k] = match schluessel_k.iter().position(|s| *s == key_k) {
            Some(g) => g,
            None => {
                schluessel_k.push(key_k);
                schluessel_k.len() - 1
            }
        };
    }
    // Startbrennweite (Merkmalsmassstab): Metadaten; sonst wie `estimateFocal`
    // der Referenzumsetzung: Median der Brennweiten aus den Paar-Homographien
    // (wenn mindestens n−1 Paare eine liefern), sonst Mittel von Breite + Hoehe.
    let mut aus_h: Vec<f64> = paare.iter().filter_map(|p| geometrie::brennweite_aus_h(&p.h)).filter(|f| f.is_finite()).collect();
    let f_geschaetzt = if aus_h.len() + 1 >= mitglieder.len() {
        median(&mut aus_h)
    } else {
        mitglieder.iter().map(|&k| (klein[k].w + klein[k].h) as f64).sum::<f64>() / mitglieder.len() as f64
    };
    let f_start: Vec<f64> = schluessel_f
        .iter()
        .enumerate()
        .map(|(g, key)| match key {
            Some(_) => {
                let k = gruppe_f.iter().position(|&x| x == g).unwrap();
                bilder[k].brennweite_px.unwrap() as f64 * s
            }
            None => f_geschaetzt,
        })
        .collect();
    let aus_metadaten = mitglieder.iter().all(|&k| schluessel_f[gruppe_f[k]].is_some());
    let diag_voll: Vec<f64> = bilder.iter().map(|b| (b.w as f64).hypot(b.h as f64) / 2.0).collect();
    let mut modell = ausrichtung::Modell {
        r: vec![mathe::EINHEIT; n],
        f: f_start,
        k1: vec![0.0; schluessel_k.len()],
        gruppe_f: gruppe_f.clone(),
        gruppe_k: gruppe_k.clone(),
        k1_frei: schluessel_k.iter().map(|k| !k.0).collect(),
        f_frei: vec![true; schluessel_f.len()],
        diag: diag_voll.iter().map(|d| d * s).collect(),
    };
    let f_start_kopie = modell.f.clone();
    let meta_klein: Vec<Option<f64>> = schluessel_f.iter().zip(&f_start_kopie).map(|(k, f)| k.map(|_| *f)).collect();
    let mut start = ausrichtung::buendelausgleich(&mut modell, &paare, &mitglieder, HUBER_ENDE);
    let mut f_festgelegt = false;
    if ausrichtung::brennweite_pruefen(&mut modell, &meta_klein) {
        diagnose("Brennweite unplausibel -> auf die Metadaten festgelegt, Ausgleich neu");
        f_festgelegt = true;
        start = ausrichtung::buendelausgleich(&mut modell, &paare, &mitglieder, HUBER_ENDE);
    }
    diagnose(&format!("Ausgleich (Merkmalsmassstab): Fehler {:.3} px, f {:?}, k1 {:?}", ausrichtung::mittlerer_fehler(&modell, &paare), modell.f, modell.k1));

    // ---- 4. Feinausrichtung auf voller Aufloesung -------------------------
    fortschritt(0.4, Schritt::Feinausrichtung);
    let voll: Vec<verfeinern::VollBild> = bilder.iter().map(|b| verfeinern::VollBild { rgb: &b.rgb, w: b.w as usize, h: b.h as usize }).collect();
    let mut mv = modell.skaliert(1.0 / s);
    let (paare_voll, nachgefuehrt, alle_punkte) = verfeinern::nachfuehren(&paare, &voll, &mv, s);
    let mv_vorher = mv.clone();
    ausrichtung::ausgleich_ende(&mut mv, &paare_voll, &mitglieder, start, HUBER_ENDE);
    let meta_voll: Vec<Option<f64>> = meta_klein.iter().map(|f| f.map(|v| v / s)).collect();
    if ausrichtung::brennweite_pruefen(&mut mv, &meta_voll) {
        f_festgelegt = true;
        let f = mv.f.clone();
        let frei = mv.f_frei.clone();
        mv = mv_vorher;
        mv.f = f.iter().zip(&mv.f).zip(&frei).map(|((neu, alt), fr)| if *fr { *alt } else { *neu }).collect();
        mv.f_frei = frei;
        ausrichtung::ausgleich_ende(&mut mv, &paare_voll, &mitglieder, start, HUBER_ENDE);
    }
    let fehler_voll = ausrichtung::mittlerer_fehler(&mv, &paare_voll);
    let punktpaare: usize = paare_voll.iter().map(|p| p.punkte.len()).sum();
    diagnose(&format!("Ausgleich (voll): Fehler {fehler_voll:.3} px, f {:?}, k1 {:?}", mv.f, mv.k1));

    // ---- 5./6. Begradigung, Leinwand -------------------------------------
    let mitte_bild = ausrichtung::spannbaum_mitte(n, &paare, &mitglieder);
    ausrichtung::auf_mitte_beziehen(&mut mv.r, &mitglieder, mitte_bild);
    let art = ausrichtung::begradigung_erkennen(&mv.r, &mitglieder);
    ausrichtung::begradigen(&mut mv.r, &mitglieder, art);
    diagnose(&format!("Begradigung: {art:?}"));
    let kameras: Vec<Kamera> = mitglieder
        .iter()
        .map(|&k| Kamera {
            r: mv.r[k],
            f: mv.optik(k).f,
            cx: bilder[k].w as f64 / 2.0,
            cy: bilder[k].h as f64 / 2.0,
            w: bilder[k].w as f64,
            h: bilder[k].h as f64,
            k1: mv.optik(k).k1,
            d: diag_voll[k],
        })
        .collect();
    // Massstab der Projektion: Median der Brennweiten (wie die Referenzumsetzung).
    let f_median = median(&mut kameras.iter().map(|k| k.f).collect::<Vec<_>>());
    let (leinwand, beschnitten) = projektion::leinwand(
        &kameras,
        opt.projektion,
        f_median,
        art == ausrichtung::Begradigung::Senkrecht,
        opt.max_ausgabe_seite.max(64),
        MAX_PIXEL,
    )
    .ok_or(PanoramaFehler::ProjektionUngeeignet)?;
    let rechtecke: Vec<(usize, usize, usize, usize)> = kameras.iter().map(|k| leinwand.rechteck(k).ok_or(PanoramaFehler::AusserhalbProjektion)).collect::<Result<_, _>>()?;
    for (idx, &k) in mitglieder.iter().enumerate() {
        let (x0, x1, y0, y1) = rechtecke[idx];
        diagnose(&format!("Bild {}: {}x{}, auf der Leinwand x {x0}..{x1}, y {y0}..{y1}", k + 1, bilder[k].w, bilder[k].h));
    }
    diagnose(&format!("Leinwand {} x {}", leinwand.w, leinwand.h));
    let dichte = leinwand.a.s / f_median; // Ausgabepixel je Quellpixel

    // ---- 7. Parallaxe (Zhang & Liu 2014) ----------------------------------
    let mut rechtecke = rechtecke;
    let mut netze: Vec<Option<parallaxe::Netz>> = vec![None; mitglieder.len()];
    let mut parallaxe_meldung = None;
    if opt.parallaxe_ausgleichen && mitglieder.len() >= 2 {
        fortschritt(0.45, Schritt::Parallaxe);
        let idx_von = |k: usize| mitglieder.iter().position(|&m| m == k).unwrap();
        let stelle = |idx: usize, x: f64, y: f64| {
            let k = &kameras[idx];
            leinwand.stelle(&k.strahl(x + k.cx, y + k.cy))
        };
        // Merkmale fuer Z&L: die nachgefuehrten Punktpaare aus Teil A. Gemessen
        // gegen ALLE Rohzuordnungen (auf voller Aufloesung von der Zuordnung aus
        // nachgefuehrt): mit diesen war der Nahtversatz nach Teil B in 3 Reihen
        // groesser (2631–2636: 0,81 gegen 0,24 px; 2620–2624: 2,66 gegen
        // 0,54 px) — Fehlzuordnungen ueberstehen die Korrelationspruefung
        // gelegentlich und ziehen das Netz.
        let p_paare: Vec<parallaxe::Paar> = paare_voll
            .iter()
            .map(|p| {
                let (i, j) = (idx_von(p.i), idx_von(p.j));
                let punkte = p.punkte.iter().filter_map(|q| Some(parallaxe::Punkt { a: stelle(i, q[0], q[1])?, b: stelle(j, q[2], q[3])? })).collect();
                parallaxe::Paar { i, j, punkte }
            })
            .collect();
        // Abstand zur Mitte des Spannbaums im Paargraphen (Breitensuche).
        let mut abstand = vec![usize::MAX; mitglieder.len()];
        let m0 = idx_von(mitte_bild);
        abstand[m0] = 0;
        let mut schlange = std::collections::VecDeque::from([m0]);
        while let Some(a) = schlange.pop_front() {
            for p in &p_paare {
                for (x, y) in [(p.i, p.j), (p.j, p.i)] {
                    if x == a && abstand[y] == usize::MAX {
                        abstand[y] = abstand[a] + 1;
                        schlange.push_back(y);
                    }
                }
            }
        }
        let p_bilder: Vec<parallaxe::Bild> = mitglieder
            .iter()
            .enumerate()
            .map(|(idx, &k)| parallaxe::Bild { kamera: kameras[idx], rgb: &klein[k].rgb, w: klein[k].w, h: klein[k].h, s, rechteck: rechtecke[idx] })
            .collect();
        let a = parallaxe::ausgleichen(&leinwand, &p_bilder, &p_paare, &abstand, dichte, s);
        if a.paare > 0 {
            let groesste = a.netze.iter().flatten().map(|n| n.groesste()).fold(0.0, f64::max);
            let neigung = a.netze.iter().flatten().map(|n| n.groesste_neigung_grad()).fold(0.0, f64::max);
            parallaxe_meldung = Some(Meldung::Parallaxe {
                vorher_px: (a.vorher.0 / dichte) as f32,
                nachher_px: (a.nachher.0 / dichte) as f32,
                vorher_90_px: (a.vorher.1 / dichte) as f32,
                nachher_90_px: (a.nachher.1 / dichte) as f32,
                groesste_px: (groesste / dichte) as f32,
                groesste_neigung_grad: neigung as f32,
            });
        }
        netze = a.netze;
        for (idx, n) in netze.iter().enumerate() {
            if let Some(n) = n {
                let r = n.groesste().ceil() as usize + 1;
                let (x0, x1, y0, y1) = rechtecke[idx];
                rechtecke[idx] = (x0.saturating_sub(r), (x1 + r).min(leinwand.w), y0.saturating_sub(r), (y1 + r).min(leinwand.h));
            }
        }
    }

    // ---- 8. Belichtung und Naehte auf kleiner Leinwand --------------------
    fortschritt(0.5, Schritt::Naehte);
    let sigma = (NAHT_PIXEL / (leinwand.w as f64 * leinwand.h as f64)).sqrt().min(s / dichte).min(1.0);
    let kl = leinwand.skaliert(sigma);
    let abbildungen: Vec<Abbildungsfn> = kameras
        .iter()
        .zip(&netze)
        .map(|(k, n)| {
            let k = *k;
            let n = n.clone();
            Box::new(move |x: f64, y: f64| {
                let (x, y) = match &n {
                    Some(n) => n.zurueck(x, y),
                    None => (x, y),
                };
                naht::quelle_f(&leinwand, &k, x, y)
            }) as Abbildungsfn
        })
        .collect();
    let klein_bilder: Vec<naht::KleinBild> = mitglieder
        .iter()
        .enumerate()
        .map(|(idx, &k)| {
            let (x0, x1, y0, y1) = rechtecke[idx];
            let r = (
                (x0 as f64 * sigma).floor() as usize,
                ((x1 as f64 * sigma).ceil() as usize).clamp(1, kl.w),
                (y0 as f64 * sigma).floor() as usize,
                ((y1 as f64 * sigma).ceil() as usize).clamp(1, kl.h),
            );
            naht::verzerren_klein(&kl, &kameras[idx], &klein[k].rgb, klein[k].w, klein[k].h, s, sigma, r, &*abbildungen[idx])
        })
        .collect();
    let gains = if opt.belichtung_angleichen { naht::verstaerkungen(&klein_bilder) } else { vec![1.0; mitglieder.len()] };
    let label = naht::aufteilen(kl.w, kl.h, &klein_bilder, &gains);
    drop(klein_bilder);
    drop(klein);

    // ---- 9. Mischen -------------------------------------------------------
    fortschritt(0.6, Schritt::Mischen);
    let quellen: Vec<mischen::Quelle> = mitglieder
        .iter()
        .enumerate()
        .map(|(idx, &k)| mischen::Quelle {
            rgb: &bilder[k].rgb,
            w: bilder[k].w as usize,
            h: bilder[k].h as usize,
            gain: gains[idx] as f32,
            abbildung: &*abbildungen[idx],
            rechteck: rechtecke[idx],
        })
        .collect();
    let (rgb, deckung) = mischen::mischen(&leinwand, &quellen, &label, kl.w, kl.h, sigma, &|a| fortschritt(0.6 + 0.38 * a, Schritt::Mischen));
    drop(quellen);

    // ---- Bericht ----------------------------------------------------------
    let mut bericht = vec![Meldung::Verwendet { genutzt: mitglieder.len(), von: n }];
    verworfen.sort_by_key(|v| v.0);
    for &(bild, grund) in &verworfen {
        bericht.push(Meldung::Verworfen { bild, grund });
    }
    bericht.push(Meldung::Ausrichtung {
        fehler_px: fehler_voll as f32,
        punktpaare,
        prozent_nachgefuehrt: (100 * nachgefuehrt).checked_div(alle_punkte).unwrap_or(0) as u32,
    });
    // Festgelegte Brennweite: "aus den Metadaten" (nicht nachgeregelt).
    bericht.push(Meldung::Brennweite { px: f_median as f32, aus_metadaten });
    if f_festgelegt {
        bericht.push(Meldung::BrennweiteFestgelegt);
    }
    // Eigene Brennweite je Bild (ohne Metadaten, B&L): deutlich andere
    // Brennweite = anderer Zoom. Meldegrenze 5 %.
    for (idx, &k) in mitglieder.iter().enumerate() {
        let faktor = kameras[idx].f / f_median;
        if faktor.ln().abs() > 0.05 {
            bericht.push(Meldung::Massstab { bild: k, faktor: (if faktor > 1.0 { faktor } else { 1.0 / faktor }) as f32, groesser: faktor > 1.0 });
        }
    }
    let k1_bericht = mitglieder.iter().find(|&&k| mv.k1_frei[gruppe_k[k]]).map(|&k| mv.k1[gruppe_k[k]]).unwrap_or(0.0);
    if k1_bericht.abs() >= 0.002 {
        bericht.push(Meldung::Verzeichnung { tonne: k1_bericht < 0.0, prozent: (k1_bericht.abs() * 100.0) as f32 });
    }
    if let Some(m) = parallaxe_meldung {
        bericht.push(m);
    }
    if opt.belichtung_angleichen {
        let (lo, hi) = gains.iter().fold((f64::MAX, f64::MIN), |a, g| (a.0.min(*g), a.1.max(*g)));
        bericht.push(Meldung::Belichtung { von: lo as f32, bis: hi as f32 });
    }
    bericht.push(Meldung::Projektion {
        projektion: opt.projektion,
        w: leinwand.w as u32,
        h: leinwand.h as u32,
        verkleinert_prozent: (dichte < 0.999).then_some((dichte * 100.0) as f32),
    });
    if beschnitten {
        bericht.push(Meldung::Beschnitten);
    }
    fortschritt(1.0, Schritt::Fertig);

    Ok(Ergebnis {
        w: leinwand.w as u32,
        h: leinwand.h as u32,
        rgb,
        deckung,
        bericht,
        verwendet: mitglieder.clone(),
        verstaerkungen: gains.iter().map(|g| *g as f32).collect(),
        brennweite_px: f_median as f32,
        fehler_px: fehler_voll as f32,
        verzeichnung_k1: k1_bericht as f32,
        kameras,
        leinwand,
        naht: Nahtkarte { w: kl.w, h: kl.h, massstab: sigma, bild: label },
        verformungen: netze,
    })
}

/// Rueckfallweg fuer Bilder, die nach dem Standardweg noch getrennt sind
/// (`paare::rueckfall`): Paare zwischen getrennten Gruppen, die
/// meistversprechenden (meiste Rohzuordnungen) zuerst.
#[allow(clippy::too_many_arguments)]
fn rueckfall(
    paar_idx: &[(usize, usize)],
    zuordnungen: &[usize],
    seiten: &[paare::Seite],
    bilder: &[Eingabe],
    s: f64,
    kante: &[usize],
    paare: &mut Vec<Paar>,
    bester_treffer: &mut [usize],
) {
    let n = bilder.len();
    let dim_klein = kante.iter().copied().max().unwrap_or(1) as f64;
    let mut exif_klein: Vec<f64> = bilder.iter().filter_map(|b| b.brennweite_px).map(|f| f as f64 * s).filter(|f| *f > 0.0).collect();
    exif_klein.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut versucht = vec![false; n * n];
    loop {
        let gruppe = ausrichtung::gruppen(n, paare);
        let mut offen: Vec<(usize, usize)> = paar_idx.iter().copied().filter(|&(i, j)| gruppe[i] != gruppe[j] && !versucht[i * n + j]).collect();
        if offen.is_empty() {
            break;
        }
        offen.sort_by_key(|&(i, j)| std::cmp::Reverse(zuordnungen[i * n + j]));
        offen.truncate((2 * n).max(12));
        let (brennweiten, bekannt) = if let Some(f) = exif_klein.get(exif_klein.len() / 2) {
            (vec![*f], true)
        } else {
            let mut aus_h: Vec<f64> = paare.iter().filter_map(|p| geometrie::brennweite_aus_h(&p.h)).filter(|f| *f > 0.2 * dim_klein && *f < 30.0 * dim_klein).collect();
            aus_h.sort_by(|a, b| a.partial_cmp(b).unwrap());
            match aus_h.get(aus_h.len() / 2) {
                Some(f) => (vec![*f], false),
                None => (paare::brennweiten_raten(dim_klein), false),
            }
        };
        let neu: Vec<(usize, usize, paare::Pruefung)> =
            offen.par_iter().map(|&(i, j)| (i, j, paare::rueckfall(i, j, &seiten[i], &seiten[j], &brennweiten, bekannt))).collect();
        let mut dazu = 0;
        for (i, j, p) in neu {
            versucht[i * n + j] = true;
            bester_treffer[i] = bester_treffer[i].max(p.inlier);
            bester_treffer[j] = bester_treffer[j].max(p.inlier);
            if let Some(p) = p.paar {
                diagnose(&format!("Rueckfall: Paar {}-{} angenommen", i + 1, j + 1));
                paare.push(p);
                dazu += 1;
            }
        }
        if dazu == 0 && offen.len() < (2 * n).max(12) {
            break;
        }
    }
}

/// Groesstes achsparalleles Rechteck ganz innerhalb der Deckung (fuer
/// "automatisch zuschneiden"): [x0, y0, x1, y1] in Pixeln, x1/y1 exklusiv
/// (Breite = x1 - x0). Ohne Deckung: [0, 0, 0, 0].
///
/// Verfahren: Zeile fuer Zeile die Hoehe der gedeckten Saeule ueber jedem Pixel
/// fuehren und darin per Stapel das groesste Rechteck unter dem "Histogramm"
/// suchen — linear in der Pixelzahl, auch bei 100 MP in Sekundenbruchteilen.
pub fn groesstes_rechteck(deckung: &[u8], w: u32, h: u32) -> [u32; 4] {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || deckung.len() < w * h {
        return [0, 0, 0, 0];
    }
    let mut hoehe = vec![0u32; w];
    let mut stapel: Vec<usize> = Vec::with_capacity(w + 1);
    let mut beste = (0u64, [0u32; 4]);
    for y in 0..h {
        let z = &deckung[y * w..(y + 1) * w];
        for x in 0..w {
            hoehe[x] = if z[x] >= 128 { hoehe[x] + 1 } else { 0 };
        }
        stapel.clear();
        for x in 0..=w {
            let hx = if x < w { hoehe[x] } else { 0 };
            while let Some(&oben) = stapel.last() {
                if hoehe[oben] <= hx {
                    break;
                }
                stapel.pop();
                let hh = hoehe[oben];
                let links = stapel.last().map_or(0, |&l| l + 1);
                let flaeche = hh as u64 * (x - links) as u64;
                if flaeche > beste.0 {
                    beste = (flaeche, [links as u32, (y + 1 - hh as usize) as u32, x as u32, (y + 1) as u32]);
                }
            }
            stapel.push(x);
        }
    }
    beste.1
}
