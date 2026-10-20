// Zweiter Durchgang: alles, was die Nachbarpixel braucht — plus die Ausgabe.
//
// Eingang ist das fertig getonte Bild aus develop.wgsl (lineares ROMM-RGB,
// 0..1). Hier passieren Struktur, Rauschminderung und Schaerfe, die Geometrie
// (Objektiv, Geraderichten) und die Umrechnung in den Zielraum.
//
// Seit Prozess 2 sind alle Radien in SENSORPIXELN definiert und werden mit dem
// Massstab der Textur (`detail.w`: Texturpixel je Sensorpixel) umgerechnet.
// Vorher wirkte ein 1-Pixel-Radius auf der viermal verkleinerten Vorschau wie
// 4 Pixel im Original — die Vorschau war um ein Viertel bis zwei Drittel
// kontrastreicher als die exportierte Datei (gemessen, Sonde `gleichheit`).

struct OutUniforms {
    work_to_srgb: mat3x4<f32>,
    detail : vec4<f32>,   // Schaerfe, Radius, Maskierung, Texturpixel je Sensorpixel
    detail2: vec4<f32>,   // Struktur, Luminanzrauschen, Farbrauschen, Rauschpegel
    flags  : vec4<f32>,   // x: 1 = linear 16 Bit, y: 1 = Kennlinie aus der Tabelle (Monitorprofil)
};

@group(0) @binding(0) var src : texture_2d<f32>;
@group(0) @binding(1) var dst : texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> u : OutUniforms;
// Umkehrkurve des Bildschirms, drei Kanaele hintereinander (je TRC_LEN Werte).
@group(0) @binding(3) var<storage, read> trc : array<f32>;

const TRC_LEN: i32 = 1024;

fn mul3(m: mat3x4<f32>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(m[0].xyz, v), dot(m[1].xyz, v), dot(m[2].xyz, v));
}

// Helligkeit im Arbeitsraum (lineares ROMM-RGB).
fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2880402, 0.7118741, 0.0000857));
}

fn load_px(p: vec2<i32>, dims: vec2<i32>) -> vec3<f32> {
    let q = clamp(p, vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
    return textureLoad(src, q, 0).rgb;
}

// Die Geometrie (Objektiv, Geraderichten, Farbsaeume) passiert seit Prozess 2
// im ERSTEN Durchgang — hier ist das Bild schon entzerrt.
fn fetch(p: vec2<i32>, dims: vec2<i32>) -> vec3<f32> {
    return load_px(p, dims);
}

fn trc_lookup(kanal: i32, x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0) * f32(TRC_LEN - 1);
    let i0 = i32(floor(v));
    let i1 = min(i0 + 1, TRC_LEN - 1);
    let f = v - floor(v);
    let base = kanal * TRC_LEN;
    return mix(trc[base + i0], trc[base + i1], f);
}

fn srgb_encode(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.04045) { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

// Wahrnehmungsnahe Helligkeit (sRGB-Kennlinie) — Schaerfe und Struktur
// rechnen hier, sonst wirken helle Kanten-Saeume viel staerker als dunkle.
fn y_kod(c: vec3<f32>) -> f32 {
    return srgb_encode(luma(c));
}

// Gauss-Unschaerfe der kodierten Helligkeit mit Sigma in Texturpixeln.
fn gauss_y(xy: vec2<i32>, dims: vec2<i32>, sigma: f32) -> f32 {
    // Grosse Radien mit Schrittweite abtasten — der Export rechnet sonst
    // bei 40 MP hunderte Abrufe je Pixel fuer eine weiche Unschaerfe.
    let schritt = max(1, i32(floor(sigma / 2.0)));
    let r = (i32(ceil(sigma * 2.5)) / schritt) * schritt;
    let inv = 1.0 / (2.0 * sigma * sigma);
    var s = 0.0;
    var w = 0.0;
    for (var dy = -r; dy <= r; dy = dy + schritt) {
        for (var dx = -r; dx <= r; dx = dx + schritt) {
            let g = exp(-f32(dx * dx + dy * dy) * inv);
            s = s + y_kod(fetch(xy + vec2<i32>(dx, dy), dims)) * g;
            w = w + g;
        }
    }
    return s / max(w, 1e-6);
}

// In den Zielraum, Farbton erhalten: was ausserhalb liegt, wird zur Grauachse
// hin entsaettigt statt je Kanal abgeschnitten (das verschob bisher gesaettigtes
// Blau nach Violett und Rot nach Orange).
fn in_farbraum(o_in: vec3<f32>) -> vec3<f32> {
    var o = o_in;
    let y = clamp(dot(o, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let lo = min(o.r, min(o.g, o.b));
    if (lo < 0.0) {
        let t = y / max(y - lo, 1e-6);
        o = vec3<f32>(y) + (o - vec3<f32>(y)) * t;
    }
    let hi = max(o.r, max(o.g, o.b));
    if (hi > 1.0) {
        if (y >= 1.0) {
            o = vec3<f32>(1.0);
        } else {
            let t = (1.0 - y) / max(hi - y, 1e-6);
            o = vec3<f32>(y) + (o - vec3<f32>(y)) * t;
        }
    }
    return clamp(o, vec3<f32>(0.0), vec3<f32>(1.0));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dimsu = textureDimensions(dst);
    if (gid.x >= dimsu.x || gid.y >= dimsu.y) { return; }
    let dims = vec2<i32>(i32(dimsu.x), i32(dimsu.y));
    let xy = vec2<i32>(i32(gid.x), i32(gid.y));
    let massstab = max(u.detail.w, 1e-3);

    var c = fetch(xy, dims);

    // --- Rauschminderung: nicht-lokale Mittel (NLM) -------------------------
    // Vergleicht ganze 3x3-FLECKEN: nur Nachbarn mit aehnlicher Umgebung
    // mitteln mit. Filterbreite = Regler x GEMESSENER Rauschpegel dieser
    // Textur (u.detail2.w) — die Vorschau traegt weniger Rauschen als der
    // Export, deshalb wird je Textur gemessen.
    let n_lum = u.detail2.y;
    let n_col = u.detail2.z;
    if (n_lum > 1e-4 || n_col > 1e-4) {
        let l_c = luma(c);
        var mitte: array<f32, 9>;
        var mk = 0;
        for (var py = -1; py <= 1; py = py + 1) {
            for (var px = -1; px <= 1; px = px + 1) {
                mitte[mk] = luma(fetch(xy + vec2<i32>(px, py), dims));
                mk = mk + 1;
            }
        }
        let sigma = clamp(u.detail2.w, 0.002, 0.25);
        let h_lum = sigma * (1.2 + 4.4 * n_lum);
        let h_col = sigma * (2.5 + 6.5 * max(n_col, n_lum * 0.5));
        var acc_l = 0.0;
        var w_l = 0.0;
        var acc_c = vec3<f32>(0.0);
        var w_c = 0.0;
        for (var dy = -4; dy <= 4; dy = dy + 1) {
            for (var dx = -4; dx <= 4; dx = dx + 1) {
                let q = xy + vec2<i32>(dx, dy);
                // Fleckenabstand: mittleres Fehlerquadrat der Luminanz (3x3).
                var d2 = 0.0;
                var kk = 0;
                for (var py = -1; py <= 1; py = py + 1) {
                    for (var px = -1; px <= 1; px = px + 1) {
                        let dl = luma(fetch(q + vec2<i32>(px, py), dims)) - mitte[kk];
                        d2 = d2 + dl * dl;
                        kk = kk + 1;
                    }
                }
                d2 = d2 / 9.0;
                let s = fetch(q, dims);
                let sockel = 2.0 * sigma * sigma;
                let wl = exp(-max(d2 - sockel, 0.0) / (h_lum * h_lum));
                acc_l = acc_l + luma(s) * wl;
                w_l = w_l + wl;
                let wc = exp(-max(d2 - sockel, 0.0) / (h_col * h_col));
                acc_c = acc_c + (s - vec3<f32>(luma(s))) * wc;
                w_c = w_c + wc;
            }
        }
        let nlm_l = acc_l / max(w_l, 1e-5);
        let nlm_chroma = acc_c / max(w_c, 1e-5);
        let chroma_orig = c - vec3<f32>(l_c);
        // 9 % Feinzeichnung bleiben stehen: glattgebuegelt sieht nach Plastik aus.
        let glatt = nlm_l + (l_c - nlm_l) * 0.09;
        let lum = mix(l_c, glatt, clamp(n_lum * 2.5, 0.0, 1.0));
        let chroma = mix(chroma_orig, nlm_chroma, clamp(n_col * 4.0, 0.0, 1.0));
        c = max(vec3<f32>(lum) + chroma, vec3<f32>(0.0));
    }

    // --- Struktur: mittelfeine Zeichnung (Haut, Rinde, Haar) ----------------
    // Band zwischen ~1,5 und ~5 Sensorpixeln, nur in den Mitten, Rauschen
    // (feiner als das Band) bleibt unberuehrt — wie „Struktur" in der
    // Referenz-Entwicklung.
    let textur = u.detail2.x;
    if (abs(textur) > 1e-4) {
        let s_fein = max(1.5 * massstab, 0.6);
        let s_grob = max(5.0 * massstab, 1.2);
        let staerke = textur * clamp(5.0 * massstab / 1.2, 0.25, 1.0);
        let band = gauss_y(xy, dims, s_fein) - gauss_y(xy, dims, s_grob);
        let ye = y_kod(c);
        let mitt = 1.0 - smoothstep(0.35, 0.5, abs(ye - 0.5));
        let ye2 = clamp(ye + band * staerke * 1.4 * mitt, 0.0, 1.0);
        let l0 = max(luma(c), 1e-6);
        c = c * (srgb_decode(ye2) / l0);
    }

    // --- Schaerfen wie in der Referenz-Entwicklung: nur Helligkeit, Gauss-Radius
    //     in Sensorpixeln, Lichthof-Begrenzung („Detail 25") und Kantenmaske
    //     („Maskierung"). ----
    let sharp = u.detail.x;
    if (sharp > 1e-4) {
        let radius = 1.0 + u.detail.y * 2.0;        // 1,0 .. 3,0 Sensorpixel
        let s_tex = radius * massstab;
        // In der verkleinerten Vorschau ist ein 1-Pixel-Radius unsichtbar —
        // genau so wirkt er auch in der verkleinerten Datei.
        var menge = sharp * 1.6;
        var sigma = s_tex;
        if (sigma < 0.5) {
            menge = menge * (sigma / 0.5) * (sigma / 0.5);
            sigma = 0.5;
        }
        if (menge > 1e-3) {
            let ye = y_kod(c);
            let unsch = gauss_y(xy, dims, sigma);
            var detail = ye - unsch;
            // Lichthof-Begrenzung: nicht ueber die Nachbarschaft hinausschiessen.
            var mn = 1.0;
            var mx = 0.0;
            for (var dy = -1; dy <= 1; dy = dy + 1) {
                for (var dx = -1; dx <= 1; dx = dx + 1) {
                    let v = y_kod(fetch(xy + vec2<i32>(dx, dy), dims));
                    mn = min(mn, v);
                    mx = max(mx, v);
                }
            }
            var maske = 1.0;
            if (u.detail.z > 1e-4) {
                let kante = abs(detail) * 14.0;
                maske = smoothstep(0.0, 1.0, kante / max(u.detail.z, 1e-3));
            }
            var ye2 = ye + detail * menge * maske;
            let spiel = 0.25 * (mx - mn) + 0.004;
            ye2 = clamp(ye2, mn - spiel, mx + spiel);
            // Extreme Lichter und Tiefen nicht ueberschaerfen (kein Abschneiden).
            let rand = smoothstep(0.0, 0.06, ye) * (1.0 - smoothstep(0.94, 1.0, ye));
            ye2 = mix(ye, ye2, rand);
            let l0 = max(luma(c), 1e-6);
            c = c * (srgb_decode(clamp(ye2, 0.0, 1.0)) / l0);
            c = max(c, vec3<f32>(0.0));
        }
    }

    // --- Ausgabe ------------------------------------------------------------
    // Arbeitsraum (ROMM-RGB) -> Zielraum (sRGB bzw. Bildschirm), farbtontreu.
    var o = in_farbraum(mul3(u.work_to_srgb, c));
    if (u.flags.x < 0.5) {
        if (u.flags.y > 0.5) {
            o = vec3<f32>(trc_lookup(0, o.r), trc_lookup(1, o.g), trc_lookup(2, o.b));
        } else {
            o = vec3<f32>(srgb_encode(o.r), srgb_encode(o.g), srgb_encode(o.b));
        }
    }
    textureStore(dst, xy, vec4<f32>(o, 1.0));
}
