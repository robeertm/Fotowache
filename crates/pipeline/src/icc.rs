//! Ein sRGB-ICC-Profil erzeugen — zum **Einbetten in Exportdateien**.
//!
//! Ohne eingebettetes Profil ist eine Bilddatei eine Zahlenkolonne ohne Bedeutung.
//! Jedes Programm rät dann, und die meisten raten sRGB — die meisten, nicht alle.
//! Manche Bildbearbeitung fragt nach, Browser auf weiten Bildschirmen zeigen zu
//! kraeftig, und Druckereien nehmen im Zweifel CMYK-Annahmen. Mit Profil ist die
//! Datei eindeutig.
//!
//! Warum selbst erzeugen statt eine .icc-Datei mitliefern: ein mitgeliefertes Profil
//! muesste in alle drei Plattform-Bauten (Programmbuendel, Paket, Installer) — drei
//! Stellen, an denen es fehlen kann. Erzeugt ist es immer da, und die Werte stehen hier
//! nachlesbar statt in einer undurchsichtigen Binaerdatei.

/// Vollstaendiges sRGB-Profil (ICC v2, Matrix + Kurven), fertig zum Einbetten.
pub fn srgb_profil() -> Vec<u8> {
    // Primaerfarben und Weisspunkt in der D50-Verankerung, die ICC vorschreibt.
    // Das sind die Zahlen aus dem offiziellen sRGB-Profil.
    let r = [0.4360_7, 0.2225_0, 0.0139_2];
    let g = [0.3850_6, 0.7168_9, 0.0971_0];
    let b = [0.1430_6, 0.0606_1, 0.7141_0];
    let w = [0.9642_0, 1.0000_0, 0.8249_1];

    // Kennlinie als Tabelle statt als parametrische Form: von jedem Leseprogramm
    // verstanden, auch von aelteren.
    let n = 1024usize;
    let mut kurve = Vec::with_capacity(n);
    for i in 0..n {
        let x = i as f32 / (n - 1) as f32;
        let lin = if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        };
        kurve.push((lin * 65535.0).round().clamp(0.0, 65535.0) as u16);
    }

    let desc = tag_desc("sRGB (Fotowache)");
    let cprt = tag_text("Gemeinfrei — sRGB IEC 61966-2.1");
    let curv = tag_curv(&kurve);
    let wtpt = tag_xyz(w);
    let rx = tag_xyz(r);
    let gx = tag_xyz(g);
    let bx = tag_xyz(b);

    // Reihenfolge der Tabelle; rTRC/gTRC/bTRC zeigen bewusst auf denselben Block.
    let bloecke: Vec<(&[u8; 4], &[u8])> = vec![
        (b"desc", &desc),
        (b"wtpt", &wtpt),
        (b"rXYZ", &rx),
        (b"gXYZ", &gx),
        (b"bXYZ", &bx),
        (b"rTRC", &curv),
        (b"cprt", &cprt),
    ];
    // gTRC und bTRC teilen sich den Block von rTRC — so ist es auch im
    // mitgelieferten Bildschirmprofil eines verbreiteten Systems, und es spart
    // 4 KB in jeder Datei.
    let geteilt: [&[u8; 4]; 2] = [b"gTRC", b"bTRC"];

    let tag_anzahl = bloecke.len() + geteilt.len();
    let kopf = 128;
    let tabelle = 4 + tag_anzahl * 12;
    let mut daten = Vec::new();
    let mut eintraege: Vec<([u8; 4], u32, u32)> = Vec::new();
    let mut trc_pos = (0u32, 0u32);

    for (sig, body) in &bloecke {
        let off = (kopf + tabelle + daten.len()) as u32;
        eintraege.push((**sig, off, body.len() as u32));
        if *sig == b"rTRC" {
            trc_pos = (off, body.len() as u32);
        }
        daten.extend_from_slice(body);
        while daten.len() % 4 != 0 {
            daten.push(0);
        }
    }
    for sig in geteilt {
        eintraege.push((*sig, trc_pos.0, trc_pos.1));
    }

    let gesamt = kopf + tabelle + daten.len();
    let mut out = Vec::with_capacity(gesamt);
    out.extend_from_slice(&(gesamt as u32).to_be_bytes()); // Groesse
    out.extend_from_slice(b"FRAW"); // erzeugendes Programm
    out.extend_from_slice(&0x0230_0000u32.to_be_bytes()); // Version 2.3
    out.extend_from_slice(b"mntr"); // Geraeteklasse
    out.extend_from_slice(b"RGB "); // Datenfarbraum
    out.extend_from_slice(b"XYZ "); // Verbindungsfarbraum
    out.extend_from_slice(&[0u8; 12]); // Datum — bewusst leer: gleiche Eingabe,
                                       // gleiche Ausgabe, Byte fuer Byte.
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0u8; 4]); // Plattform
    out.extend_from_slice(&[0u8; 4]); // Kennzeichen
    out.extend_from_slice(&[0u8; 4]); // Hersteller Geraet
    out.extend_from_slice(&[0u8; 4]); // Modell Geraet
    out.extend_from_slice(&[0u8; 8]); // Eigenschaften
    out.extend_from_slice(&0u32.to_be_bytes()); // Wiedergabeabsicht: wahrnehmungsnah
    for v in w {
        out.extend_from_slice(&s15f16(v).to_be_bytes());
    }
    out.extend_from_slice(&[0u8; 4]); // Erzeuger
    while out.len() < 128 {
        out.push(0);
    }

    out.extend_from_slice(&(tag_anzahl as u32).to_be_bytes());
    for (sig, off, len) in &eintraege {
        out.extend_from_slice(sig);
        out.extend_from_slice(&off.to_be_bytes());
        out.extend_from_slice(&len.to_be_bytes());
    }
    out.extend_from_slice(&daten);
    out
}

fn s15f16(v: f32) -> i32 {
    (v * 65536.0).round() as i32
}

fn tag_xyz(v: [f32; 3]) -> Vec<u8> {
    let mut o = Vec::with_capacity(20);
    o.extend_from_slice(b"XYZ ");
    o.extend_from_slice(&[0u8; 4]);
    for x in v {
        o.extend_from_slice(&s15f16(x).to_be_bytes());
    }
    o
}

fn tag_curv(tab: &[u16]) -> Vec<u8> {
    let mut o = Vec::with_capacity(12 + tab.len() * 2);
    o.extend_from_slice(b"curv");
    o.extend_from_slice(&[0u8; 4]);
    o.extend_from_slice(&(tab.len() as u32).to_be_bytes());
    for v in tab {
        o.extend_from_slice(&v.to_be_bytes());
    }
    o
}

fn tag_text(s: &str) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(b"text");
    o.extend_from_slice(&[0u8; 4]);
    o.extend_from_slice(s.as_bytes());
    o.push(0);
    o
}

fn tag_desc(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let n = bytes.len() + 1;
    let mut o = Vec::new();
    o.extend_from_slice(b"desc");
    o.extend_from_slice(&[0u8; 4]);
    o.extend_from_slice(&(n as u32).to_be_bytes());
    o.extend_from_slice(bytes);
    o.push(0);
    // Der Rest des alten `desc`-Tags: Unicode- und Skriptcode-Felder, die niemand
    // mehr liest, aber ohne die manche Pruefprogramme meckern.
    o.extend_from_slice(&[0u8; 8]); // Unicode-Sprache + -Laenge
    o.extend_from_slice(&[0u8; 3]); // Skriptcode-Kennung + -Laenge
    o.extend_from_slice(&[0u8; 67]); // Skriptcode-Text
    o
}

#[cfg(test)]
mod tests {
    #[test]
    fn profil_ist_wohlgeformt() {
        let p = super::srgb_profil();
        assert_eq!(&p[36..40], b"acsp", "ICC-Kennung fehlt");
        assert_eq!(
            u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize,
            p.len(),
            "Groessenangabe im Kopf passt nicht zur Datei"
        );
        // Jeder Tag muss vollstaendig innerhalb der Datei liegen.
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(n, 9);
        for i in 0..n {
            let b = 132 + i * 12;
            let off = u32::from_be_bytes(p[b + 4..b + 8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(p[b + 8..b + 12].try_into().unwrap()) as usize;
            assert!(off + len <= p.len(), "Tag {i} zeigt aus der Datei heraus");
        }
    }
}
