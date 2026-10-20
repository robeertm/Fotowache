//! Grundeinstellungen wie in gaengigen Raw-Entwicklern: Profil, Weissabgleich in
//! Kelvin mit Vorgaben und Pipette, Belichtung in Blenden, dann die Tonregler und
//! die Praesenz — in genau der Reihenfolge, die ihre Nutzer kennen.

use crate::farbe;
use crate::sprache::{t, t_laufzeit, tf};
use eframe::egui;
use pipeline::params::Edit;

/// Die Weissabgleich-Vorgaben der Referenz-Entwicklung (Kelvin, Toenung).
///
/// Der deutsche Name ist der feste Schluessel der Vorgabe und wird nie direkt
/// gezeigt: wiedererkannt wird eine Vorgabe an Kelvin und Toenung, angezeigt
/// wird `t(name)`. So haengt nichts an der gerade gewaehlten Sprache.
pub const WB_VORGABEN: &[(&str, f64, f64)] = &[
    ("Tageslicht", 5500.0, 10.0),
    ("Bewölkt", 6500.0, 10.0),
    ("Schatten", 7500.0, 10.0),
    ("Kunstlicht", 2850.0, 0.0),
    ("Leuchtstoff", 3800.0, 21.0),
    ("Blitz", 5500.0, 0.0),
];

/// Was die Grundeinstellungen ueber das aktuelle Foto wissen muessen.
pub struct FotoInfo {
    /// Weissabgleich der Aufnahme (Kelvin, Toenung) auf der Skala der
    /// Referenz-Entwicklung.
    pub wb_aufnahme: (f32, f32),
    /// Waehlbare Profile (installiertes Standardprofil, Kamera-Profile …), ohne
    /// „fotoraw" (gespeicherter Schluessel aus der Zeit vor dem Namen Fotowache).
    pub profile: Vec<String>,
    /// Name des gerade wirksamen Profils.
    pub profil_name: String,
}

/// Was der Nutzer im Feld ausgeloest hat, das ueber Reglerwerte hinausgeht.
#[derive(Default)]
pub struct Aktionen {
    pub geaendert: bool,
    pub pipette_umschalten: bool,
    pub auto_wb: bool,
    pub auto_ton: bool,
    pub zuruecksetzen: bool,
}

/// Neutrale Anzeigenamen der Looks aus `pipeline::looks::ANGEBOTEN`, in derselben
/// Reihenfolge — deren Schluessel sind Dateinamen einer anderen Anwendung.
pub(crate) const LOOK_NAMEN: [&str; pipeline::looks::ANGEBOTEN.len()] =
    ["Look: Farbe", "Look: Porträt", "Look: Landschaft", "Look: Neutral", "Look: Lebendig"];

fn look_anzeige(schluessel: &str) -> Option<&'static str> {
    let i = pipeline::looks::ANGEBOTEN.iter().position(|a| *a == schluessel)?;
    Some(t(LOOK_NAMEN[i]))
}

/// Anzeigename eines Profilschluessels aus `Edit::profil`. Die Schluessel selbst
/// ("fotoraw", "dng", Namen installierter Profile) sind intern. Das installierte
/// Standardprofil (`dcp::STANDARDPROFIL`, so heisst es in seiner Datei und in
/// Begleitdateien) und die Looks bekommen neutrale Namen; alle anderen Namen
/// stammen aus Profildateien des Nutzers und bleiben, wie sie sind.
fn profil_anzeige(schluessel: &str) -> String {
    match schluessel {
        "fotoraw" => "Fotowache Standard".into(),
        pipeline::dcp::EINGEBETTET => t("Eingebettet (DNG)").into(),
        pipeline::dcp::STANDARDPROFIL => t("Standardprofil (installiert)").into(),
        n => look_anzeige(n).unwrap_or(n).into(),
    }
}

/// Name des gerade wirksamen Profils fuer „… (automatisch)": das installierte
/// Standardprofil und die Looks neutral, die Namen der Farbkette uebersetzt,
/// alle anderen (aus Profildateien) unveraendert.
fn wirksames_profil(name: &str) -> &str {
    if name == pipeline::dcp::STANDARDPROFIL {
        t("Standardprofil (installiert)")
    } else if let Some(look) = look_anzeige(name) {
        look
    } else {
        t_laufzeit(name)
    }
}

/// Zahl mit Vorzeichen und `stellen` Nachkommastellen — ohne „-0": ein
/// Wert knapp unter null soll nicht als negative Null erscheinen.
pub fn mit_vorzeichen(v: f32, stellen: usize) -> String {
    let faktor = 10f32.powi(stellen as i32);
    let gerundet = (v * faktor).round() / faktor;
    let gerundet = if gerundet == 0.0 { 0.0 } else { gerundet };
    format!("{:+.*}", stellen, gerundet)
}

fn zeile_wert(ui: &mut egui::Ui, text: String, betont: bool) {
    let t = egui::RichText::new(text).size(11.0);
    let t = if betont { t.strong() } else { t.weak() };
    ui.add_sized([58.0, 18.0], egui::Label::new(t));
}

/// Ein Regler mit eigener Anzeige (z. B. Kelvin oder Blenden).
fn regler_mit_anzeige(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    def: f32,
    anzeige: String,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let resp = ui.add_sized(
            [78.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(12.0))
                .sense(egui::Sense::click())
                .halign(egui::Align::LEFT),
        );
        if resp.double_clicked() {
            *value = def;
            changed = true;
        }
        resp.on_hover_text(t("Doppelklick setzt zurück"));
        // Platz lassen fuer die Wertanzeige (58) samt Abstand davor und
        // danach — sonst ist die Zeile ein paar Punkte breiter als das Feld,
        // und das Reglerfeld waechst mit jedem gezeichneten Bild weiter, bis
        // es das Foto verdraengt (am Bildschirmfoto-Pruefstand gesehen).
        let abstand = ui.spacing().item_spacing.x;
        ui.spacing_mut().slider_width = (ui.available_width() - 58.0 - 2.0 * abstand - 2.0).max(60.0);
        changed |= ui
            .add(egui::Slider::new(value, min..=max).show_value(false))
            .changed();
        zeile_wert(ui, anzeige, (*value - def).abs() > 1e-4);
    });
    changed
}

pub fn grundeinstellungen(ui: &mut egui::Ui, e: &mut Edit, info: &FotoInfo, pipette_aktiv: bool) -> Aktionen {
    let mut a = Aktionen::default();

    // --- Kopf: Auto / Zuruecksetzen ------------------------------------------
    ui.horizontal(|ui| {
        if ui
            .button(t("Auto"))
            .on_hover_text(t("Ton und Farbe automatisch einstellen (⌘U)"))
            .clicked()
        {
            a.auto_ton = true;
        }
        if ui
            .button(t("Zurücksetzen"))
            .on_hover_text(t("Zurück zur Grundausstattung dieses Fotos"))
            .clicked()
        {
            a.zuruecksetzen = true;
        }
    });

    // --- Profil -----------------------------------------------------------------
    ui.horizontal(|ui| {
        ui.add_sized([78.0, 18.0], egui::Label::new(egui::RichText::new(t("Profil")).size(12.0)));
        let vorher = e.profil.clone();
        let anzeige = match e.profil.as_str() {
            "" => tf("{} (automatisch)", &[&wirksames_profil(&info.profil_name)]),
            n => profil_anzeige(n),
        };
        egui::ComboBox::from_id_salt("profil")
            .selected_text(anzeige)
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut e.profil, String::new(), t("Automatisch"));
                for p in &info.profile {
                    ui.selectable_value(&mut e.profil, p.clone(), profil_anzeige(p));
                }
                ui.selectable_value(&mut e.profil, "fotoraw".to_string(), "Fotowache Standard");
            })
            .response
            .on_hover_text(t(
                "Farbwiedergabe der Kamera. Installierte Kameraprofile (DCP) stammen von \
                 anderen Anwendungen auf diesem Rechner; Fotowache Standard funktioniert immer.",
            ));
        a.geaendert |= e.profil != vorher;
    });

    ui.add_space(4.0);
    // --- Weissabgleich ------------------------------------------------------------
    let (k0, t0) = (info.wb_aufnahme.0 as f64, info.wb_aufnahme.1 as f64);
    let (k_jetzt, t_jetzt) = pipeline::wb_ziel(k0, t0, e.temp, e.tint).unwrap_or((k0, t0));
    ui.horizontal(|ui| {
        ui.add_sized([78.0, 18.0], egui::Label::new(egui::RichText::new(t("Weißabgleich")).size(12.0)));
        // Wiedererkannt wird an Kelvin und Toenung, gezeigt der uebersetzte Name.
        let aktuell = if e.temp == 0.0 && e.tint == 0.0 {
            t("Wie Aufnahme")
        } else {
            WB_VORGABEN
                .iter()
                .find(|(_, k, tn)| (k - k_jetzt).abs() < 25.0 && (tn - t_jetzt).abs() < 1.0)
                .map(|(schluessel, _, _)| t(*schluessel))
                .unwrap_or_else(|| t("Benutzerdefiniert"))
        };
        egui::ComboBox::from_id_salt("wb_vorgabe")
            .selected_text(aktuell)
            // Platz fuer den Pipetten-Knopf rechts daneben: Zeichen (~14),
            // Innenabstand links/rechts und der Abstand davor. Mit zu wenig
            // Reserve ist die Zeile breiter als das Feld, und weil die Box
            // sich nach der verfuegbaren Breite richtet, schob sie das ganze
            // Reglerfeld mit jedem Bild weiter auf (4,5 Punkte je Bild).
            .width(
                ui.available_width()
                    - (16.0 + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x + 2.0),
            )
            .show_ui(ui, |ui| {
                if ui.selectable_label(e.temp == 0.0 && e.tint == 0.0, t("Wie Aufnahme")).clicked() {
                    e.temp = 0.0;
                    e.tint = 0.0;
                    a.geaendert = true;
                }
                if ui.selectable_label(false, t("Automatisch")).clicked() {
                    a.auto_wb = true;
                }
                for (schluessel, k, tn) in WB_VORGABEN {
                    if ui.selectable_label(false, t(*schluessel)).clicked() {
                        let (temp, tint) = pipeline::wb_regler(k0, t0, *k, *tn);
                        e.temp = temp;
                        e.tint = tint;
                        a.geaendert = true;
                    }
                }
            });
        let pip = ui
            .add(egui::Button::new(egui::RichText::new("◎").size(13.0)).selected(pipette_aktiv))
            .on_hover_text(t("Pipette (W): auf eine neutrale Stelle (Grau/Weiß) klicken"));
        if pip.clicked() {
            a.pipette_umschalten = true;
        }
    });
    a.geaendert |= regler_mit_anzeige(
        ui,
        t("Temperatur"),
        &mut e.temp,
        -250.0,
        250.0,
        0.0,
        format!("{:.0} K", k_jetzt),
    );
    a.geaendert |= regler_mit_anzeige(
        ui,
        t("Tönung"),
        &mut e.tint,
        -150.0,
        150.0,
        0.0,
        mit_vorzeichen(t_jetzt as f32, 0),
    );

    ui.add_space(4.0);
    // --- Ton ------------------------------------------------------------------------
    // Belichtung in Blenden: Grundhelligkeit (Automatik/XMP-Einstellungen) + Regler.
    let mut gesamt = e.base_ev + e.exposure / 50.0;
    let alt = gesamt;
    let anzeige = mit_vorzeichen(gesamt, 2);
    if regler_mit_anzeige(ui, t("Belichtung"), &mut gesamt, -5.0, 5.0, e.base_ev, anzeige) {
        e.exposure = ((gesamt - e.base_ev) * 50.0 * 100.0).round() / 100.0;
        a.geaendert |= (gesamt - alt).abs() > 1e-6;
    }
    let ton = |ui: &mut egui::Ui, name: &str, v: &mut f32| {
        let w = mit_vorzeichen(*v, 0);
        regler_mit_anzeige(ui, name, v, -100.0, 100.0, 0.0, w)
    };
    a.geaendert |= ton(ui, t("Kontrast"), &mut e.contrast);
    a.geaendert |= ton(ui, t("Lichter"), &mut e.highlights);
    a.geaendert |= ton(ui, t("Tiefen"), &mut e.shadows);
    a.geaendert |= ton(ui, t("Weiß"), &mut e.whites);
    a.geaendert |= ton(ui, t("Schwarz"), &mut e.blacks);
    ui.add_space(4.0);
    a.geaendert |= ton(ui, t("Struktur"), &mut e.texture);
    a.geaendert |= ton(ui, t("Klarheit"), &mut e.clarity);
    a.geaendert |= ton(ui, t("Dunst entf."), &mut e.dehaze);
    a.geaendert |= ton(ui, t("Dynamik"), &mut e.vibrance);
    a.geaendert |= ton(ui, t("Sättigung"), &mut e.saturation);
    let bw = e.bw;
    ui.checkbox(&mut e.bw, t("Schwarzweiß (V)"));
    a.geaendert |= e.bw != bw;
    ui.label(
        egui::RichText::new(t("Doppelklick auf einen Namen setzt den Regler zurück"))
            .size(10.0)
            .color(farbe::TEXT_LEISE),
    );
    a
}

#[cfg(test)]
mod tests {
    use super::mit_vorzeichen;

    #[test]
    fn keine_negative_null() {
        assert_eq!(mit_vorzeichen(-0.3, 0), "+0");
        assert_eq!(mit_vorzeichen(-0.0, 2), "+0.00");
        assert_eq!(mit_vorzeichen(-0.6, 0), "-1");
        assert_eq!(mit_vorzeichen(0.154, 2), "+0.15");
    }
}
