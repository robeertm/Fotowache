// Entwicklungs-Shader, Prozess 2 — gebaut wie die Referenz-Entwicklung.
//
// Gerechnet wird in LINEAREM ROMM-RGB (D50), dem Raum, in dem die
// DNG-Spezifikation Kameraprofile definiert. Die Reihenfolge folgt der
// Standardentwicklung des DNG-SDK, erweitert um die lokalen Tonregler der
// Referenz-Entwicklung:
//
//   Kamera → Profilmatrix → Farbtabelle (HueSatMap) → Belichtung
//   → Lichter/Tiefen LOKAL (Basisebene) → Klarheit/Dunst → Weiss/Schwarz/Kontrast
//   → Schulter → Look-Tabelle → Grundkurve (farbtonerhaltend) → Farbe/Effekte
//
// Die frueheren globalen Fenster auf der Pixelhelligkeit (Lichter/Tiefen als
// Kurve) drueckten den Kontrast platt: „Tiefen +50" hob jedes dunkle Pixel an,
// auch die dunkle Zeichnung IN einem hellen Bereich. Jetzt wirken Lichter und
// Tiefen auf die Helligkeit der UMGEBUNG (kantenerhaltende Basis, basis.rs) —
// die Zeichnung bleibt erhalten, so wie in der Referenz-Entwicklung.

struct Uniforms {
    kamera_zu_pro : array<vec4<f32>, 3>, // Zeilen
    belichtung    : vec4<f32>,  // Gesamt-EV, Kontrast, Lichter, Tiefen
    ton2          : vec4<f32>,  // Weiss, Schwarz, Grauschleier, Dunst
    lokal         : vec4<f32>,  // Klarheit, Profil-Schwarz, Objektiv-Randaufhellung, -
    farbe         : vec4<f32>,  // Dynamik, Saettigung, S/W, Koernung
    grade_lo      : vec4<f32>,  // Farbton(rad), Saettigung, Balance
    grade_mi      : vec4<f32>,
    grade_hi      : vec4<f32>,
    vign          : vec4<f32>,  // Vignette (nach Zuschnitt), Mittelpunkt, Koernungsgroesse(Pixel), Texturpixel je Sensorpixel
    zuschnitt     : vec4<f32>,  // Zuschnitt in Anteilen des Sensorbilds (x, y, b, h)
    curve         : array<vec4<f32>, 2>,
    hsl_h         : array<vec4<f32>, 2>,
    hsl_s         : array<vec4<f32>, 2>,
    hsl_l         : array<vec4<f32>, 2>,
    lage          : vec4<f32>,  // Kachel-Versatz x/y, volle Breite/Hoehe — in Pixeln DIESER Textur
    tabellen      : vec4<u32>,  // HSM da, Look da, HSM-Kodierung, Look-Kodierung
    hsm_dims      : vec4<u32>,  // Farbtoene, Saettigungen, Werte, Versatz
    look_dims     : vec4<u32>,
    basis_dims    : vec4<u32>,  // Breite, Hoehe
    kal           : array<vec4<f32>, 6>,
    saat          : vec4<f32>,
    bild          : vec4<f32>,  // Bildumfang der Basis (log2, ohne Belichtung): p2, p50, p98, p99,5
    geo           : vec4<f32>,  // Feinwinkel(rad), manuelle Entzerrung k, Randmassstab, aktiv
    lens1         : vec4<f32>,  // Objektivprofil ptlens a, b, c, Fuellmassstab
    radial        : vec4<f32>,  // Kamera-Korrektur: Stuetzstellen, Fuellmassstab, Farbsaum an, Vignette an
    r_quelle      : array<vec4<f32>, 4>,
    r_ziel        : array<vec4<f32>, 4>,
    r_m           : array<vec4<f32>, 4>,
    r_ca_r        : array<vec4<f32>, 4>,
    r_ca_b        : array<vec4<f32>, 4>,
    r_gain        : array<vec4<f32>, 4>,
};

@group(0) @binding(0) var src : texture_2d<f32>;
@group(0) @binding(1) var dst : texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> u : Uniforms;
// Grundkurve (1025 Werte) + Farbtabelle + Look-Tabelle hintereinander.
@group(0) @binding(3) var<storage, read> tab : array<f32>;
// Basisebene: je Basispixel (a_ton, b_ton, a_klar, b_klar), siehe basis.rs.
@group(0) @binding(4) var basis : texture_2d<f32>;

const LOG_BODEN : f32 = 1.52587890625e-05; // 2^-16
const KURVE_N : i32 = 1024;

// Helligkeit in linearem ROMM-RGB (Y-Zeile der ROMM-RGB→XYZ-Matrix).
fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2880402, 0.7118741, 0.0000857));
}

fn zeilen(m: array<vec4<f32>, 3>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(m[0].xyz, v), dot(m[1].xyz, v), dot(m[2].xyz, v));
}

fn idx8(arr: array<vec4<f32>, 2>, i: i32) -> f32 {
    let a = arr;
    if (i < 4) { return a[0][i]; }
    return a[1][i - 4];
}

fn srgb_kod(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_dekod(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0);
    if (v <= 0.04045) { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

// ---------------------------------------------------------------------------
// HSV nach DNG-SDK: Farbton 0..6, Wert = groesster Kanal.
// ---------------------------------------------------------------------------
fn dng_hsv(c: vec3<f32>) -> vec3<f32> {
    let v = max(c.r, max(c.g, c.b));
    let gap = v - min(c.r, min(c.g, c.b));
    var h = 0.0;
    var s = 0.0;
    if (gap > 0.0) {
        if (c.r == v) {
            h = (c.g - c.b) / gap;
            if (h < 0.0) { h = h + 6.0; }
        } else if (c.g == v) {
            h = 2.0 + (c.b - c.r) / gap;
        } else {
            h = 4.0 + (c.r - c.g) / gap;
        }
        s = gap / v;
    }
    return vec3<f32>(h, s, v);
}

fn dng_rgb(hsv: vec3<f32>) -> vec3<f32> {
    var h = hsv.x;
    let s = hsv.y;
    let v = hsv.z;
    if (s <= 0.0) { return vec3<f32>(v); }
    h = h - floor(h / 6.0) * 6.0;
    let i = floor(h);
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    if (i < 1.0) { return vec3<f32>(v, t, p); }
    if (i < 2.0) { return vec3<f32>(q, v, p); }
    if (i < 3.0) { return vec3<f32>(p, v, t); }
    if (i < 4.0) { return vec3<f32>(p, q, v); }
    if (i < 5.0) { return vec3<f32>(t, p, v); }
    return vec3<f32>(v, p, q);
}

// Ein Eintrag (Farbtonverschiebung°, Saettigungsfaktor, Wertfaktor).
fn eintrag(basis_i: u32, idx: u32) -> vec3<f32> {
    let o = basis_i + idx * 3u;
    return vec3<f32>(tab[o], tab[o + 1u], tab[o + 2u]);
}

// Farbton/Saettigung/Wert-Tabelle anwenden (RefHueSatMap des DNG-SDK).
// `linear_ohne_deckel`: die Farbtabelle VOR der Belichtung arbeitet auf
// Werten ueber 1 — dort darf der Wertfaktor nicht bei 1 kappen.
fn farbtabelle(c: vec3<f32>, dims: vec4<u32>, kodierung: u32, linear_ohne_deckel: bool) -> vec3<f32> {
    let hn = dims.x;
    let sn = dims.y;
    let vn = dims.z;
    let off = dims.w;
    var hsv = dng_hsv(c);
    if (hsv.y <= 0.0 && hn > 0u) {
        // Neutrales Grau: der Tabelleneintrag bei Saettigung 0 kann trotzdem
        // einen Wertfaktor tragen — durchreichen wie das SDK.
    }
    var v_kod = hsv.z;
    if (kodierung == 1u) { v_kod = srgb_kod(hsv.z); }
    let h_skal = hsv.x * f32(hn) / 6.0;
    let s_skal = hsv.y * f32(sn - 1u);
    var h0 = u32(floor(h_skal));
    let s0 = min(u32(floor(s_skal)), sn - 2u);
    var h1 = h0 + 1u;
    if (h0 >= hn - 1u) { h0 = hn - 1u; h1 = 0u; }
    let hf = h_skal - f32(h0);
    let sf = s_skal - f32(s0);
    let schritt_h = sn;
    let schritt_v = hn * sn;
    var aenderung = vec3<f32>(0.0);
    if (vn < 2u) {
        let e00 = eintrag(off, h0 * schritt_h + s0);
        let e10 = eintrag(off, h1 * schritt_h + s0);
        let e01 = eintrag(off, h0 * schritt_h + s0 + 1u);
        let e11 = eintrag(off, h1 * schritt_h + s0 + 1u);
        aenderung = mix(mix(e00, e10, hf), mix(e01, e11, hf), sf);
    } else {
        let v_skal = clamp(v_kod, 0.0, 1.0) * f32(vn - 1u);
        let v0 = min(u32(floor(v_skal)), vn - 2u);
        let vf = v_skal - f32(v0);
        let vb0 = v0 * schritt_v;
        let vb1 = (v0 + 1u) * schritt_v;
        let a0 = mix(mix(eintrag(off, vb0 + h0 * schritt_h + s0), eintrag(off, vb0 + h1 * schritt_h + s0), hf),
                     mix(eintrag(off, vb0 + h0 * schritt_h + s0 + 1u), eintrag(off, vb0 + h1 * schritt_h + s0 + 1u), hf), sf);
        let a1 = mix(mix(eintrag(off, vb1 + h0 * schritt_h + s0), eintrag(off, vb1 + h1 * schritt_h + s0), hf),
                     mix(eintrag(off, vb1 + h0 * schritt_h + s0 + 1u), eintrag(off, vb1 + h1 * schritt_h + s0 + 1u), hf), sf);
        aenderung = mix(a0, a1, vf);
    }
    hsv.x = hsv.x + aenderung.x * (6.0 / 360.0);
    hsv.y = min(hsv.y * aenderung.y, 1.0);
    if (linear_ohne_deckel && kodierung == 0u) {
        hsv.z = hsv.z * aenderung.z;
    } else {
        v_kod = clamp(v_kod * aenderung.z, 0.0, 1.0);
        if (kodierung == 1u) { hsv.z = srgb_dekod(v_kod); } else { hsv.z = v_kod; }
    }
    return dng_rgb(hsv);
}

// ---------------------------------------------------------------------------
// Grundkurve, farbtonerhaltend (RefBaselineRGBTone): die Kurve trifft den
// groessten und den kleinsten Kanal, der mittlere wird so nachgezogen, dass
// der Farbton bleibt. Mittlere Toene saettigen dadurch massvoll — genau
// das Verhalten der Referenz-Entwicklung.
// ---------------------------------------------------------------------------
fn kurve(x: f32) -> f32 {
    let v = clamp(x, 0.0, 1.0) * f32(KURVE_N);
    let i = min(i32(floor(v)), KURVE_N - 1);
    let f = v - f32(i);
    return mix(tab[i], tab[i + 1], f);
}

fn rgb_ton(c: vec3<f32>) -> vec3<f32> {
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let th = kurve(hi);
    let tl = kurve(lo);
    var erhaltend = vec3<f32>(th);
    if (hi - lo >= 1e-7) {
        let k = (th - tl) / (hi - lo);
        erhaltend = vec3<f32>(tl + (c.r - lo) * k, tl + (c.g - lo) * k, tl + (c.b - lo) * k);
    }
    // Die Prozessversionen der Referenz-Entwicklung saettigen die Mitten etwas
    // staerker als das DNG-SDK — Anteil einer Kurve je Kanal, kalibriert (kal[4].x).
    if (u.kal[4].x > 1e-4) {
        let je_kanal = vec3<f32>(kurve(c.r), kurve(c.g), kurve(c.b));
        return mix(erhaltend, je_kanal, clamp(u.kal[4].x, 0.0, 1.0));
    }
    return erhaltend;
}

// ---------------------------------------------------------------------------
// Lokale Tonregler auf der Basis (log2, 0 = Sensor-Weiss nach Belichtung).
// ---------------------------------------------------------------------------
// Bildabhaengig wie in der Referenz-Entwicklung: liegen die hellsten Bereiche eines Fotos
// deutlich unter Weiss (Nebel, Innenraum), rutscht das Lichter-Fenster mit;
// ebenso das Tiefen-Fenster mit den dunkelsten Bereichen. Wie stark, ist
// kalibriert (kal[4].y / kal[4].z).
fn lichter_tiefen(b: f32, ev: f32) -> f32 {
    let lichter = u.belichtung.z;
    let tiefen = u.belichtung.w;
    var g = 0.0;
    if (abs(tiefen) > 1e-5) {
        let dunkel = u.bild.x + ev;
        let sv = clamp(dunkel - u.kal[0].y, -3.0, 3.0) * u.kal[4].z;
        let w = 1.0 - smoothstep(u.kal[0].y + sv, u.kal[0].z + sv, b);
        g = g + tiefen * u.kal[0].x * w;
    }
    if (abs(lichter) > 1e-5) {
        let hell = u.bild.w + ev;
        let hv = clamp(hell, -3.0, 0.0) * u.kal[4].y;
        let w = smoothstep(u.kal[1].x + hv, u.kal[1].y + hv, b);
        g = g + lichter * u.kal[0].w * w;
    }
    return g;
}

// Kontrast als S-Kurve mit festen Endpunkten: im log-Raum von -9 EV (Schwarz)
// bis 0 EV (Weiss), Drehpunkt bei den Mitten. k > 1 steiler.
fn kontrast(l: f32, k: f32, pivot: f32) -> f32 {
    if (l > 0.0) { return l * k; }
    let lo = -9.0;
    let x = clamp((l - lo) / (0.0 - lo), 0.0, 1.0);
    let p = clamp((pivot - lo) / (0.0 - lo), 0.05, 0.95);
    var y = x;
    if (x < p) {
        y = p * pow(x / p, k);
    } else {
        y = 1.0 - (1.0 - p) * pow((1.0 - x) / (1.0 - p), k);
    }
    if (l < lo) { return l + (lo + y * (0.0 - lo) - lo); }
    return lo + y * (0.0 - lo);
}

// Katmull-Rom durch die fuenf Stuetzstellen der Nutzer-Tonwertkurve.
fn curve_eval(x_in: f32) -> f32 {
    let x = clamp(x_in, 0.0, 1.0);
    let seg = clamp(x * 4.0, 0.0, 3.9999);
    let i = i32(floor(seg));
    let t = seg - f32(i);
    let p0 = idx8(u.curve, max(i - 1, 0));
    let p1 = idx8(u.curve, i);
    let p2 = idx8(u.curve, min(i + 1, 4));
    let p3 = idx8(u.curve, min(i + 2, 4));
    let t2 = t * t;
    let t3 = t2 * t;
    return 0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
}

fn rgb2hsv(c: vec3<f32>) -> vec3<f32> {
    let h = dng_hsv(c);
    return vec3<f32>(h.x / 6.0, h.y, h.z);
}

fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    return dng_rgb(vec3<f32>(fract(c.x) * 6.0, c.y, c.z));
}

// HSL nach acht Farbbereichen (weiche 45°-Fenster, wie bisher — KI-Vertrag).
// Der Farbton wird auf gamma-kodierten Werten bestimmt: so liegen die Baender
// dort, wo man die Farben sieht.
fn hsl_bands(c: vec3<f32>) -> vec3<f32> {
    var d_h = 0.0;
    var d_s = 0.0;
    var d_l = 0.0;
    let enc = vec3<f32>(srgb_kod(c.r), srgb_kod(c.g), srgb_kod(c.b));
    var hsv = rgb2hsv(enc);
    let hue_deg = hsv.x * 360.0;
    for (var b = 0; b < 8; b = b + 1) {
        let center = f32(b) * 45.0;
        var dist = abs(hue_deg - center);
        if (dist > 180.0) { dist = 360.0 - dist; }
        let w = max(0.0, 1.0 - dist / 45.0);
        if (w > 0.0) {
            let ww = w * w * (3.0 - 2.0 * w);
            d_h = d_h + idx8(u.hsl_h, b) * ww;
            d_s = d_s + idx8(u.hsl_s, b) * ww;
            d_l = d_l + idx8(u.hsl_l, b) * ww;
        }
    }
    if (abs(d_h) + abs(d_s) + abs(d_l) < 1e-5) { return c; }
    let strength = smoothstep(0.02, 0.15, hsv.y);
    hsv.x = fract(hsv.x + d_h * 0.08 * strength);
    hsv.y = clamp(hsv.y * (1.0 + d_s * strength), 0.0, 1.0);
    var out = hsv2rgb(hsv);
    out = out * (1.0 + d_l * 0.5 * strength);
    return vec3<f32>(srgb_dekod(out.r), srgb_dekod(out.g), srgb_dekod(out.b));
}

fn split_tone(c: vec3<f32>) -> vec3<f32> {
    let amount0 = u.grade_lo.y + u.grade_mi.y + u.grade_hi.y;
    if (amount0 < 1e-5) { return c; }
    let l = clamp(luma(c), 0.0, 1.0);
    let bal = u.grade_lo.z;
    let piv = clamp(0.5 - bal * 0.3, 0.15, 0.85);
    let lw = srgb_kod(l);
    let w_lo = 1.0 - smoothstep(0.0, piv, lw);
    let w_hi = smoothstep(piv, 1.0, lw);
    let w_mi = max(0.0, 1.0 - w_lo - w_hi);
    var tint = vec3<f32>(0.0);
    tint = tint + hsv2rgb(vec3<f32>(u.grade_lo.x / 6.2831853, 1.0, 1.0)) * u.grade_lo.y * w_lo;
    tint = tint + hsv2rgb(vec3<f32>(u.grade_mi.x / 6.2831853, 1.0, 1.0)) * u.grade_mi.y * w_mi;
    tint = tint + hsv2rgb(vec3<f32>(u.grade_hi.x / 6.2831853, 1.0, 1.0)) * u.grade_hi.y * w_hi;
    let amount = (u.grade_lo.y * w_lo + u.grade_mi.y * w_mi + u.grade_hi.y * w_hi);
    if (amount < 1e-5) { return c; }
    let mixed = mix(c, c * (0.5 + tint), clamp(amount * 0.6, 0.0, 1.0));
    let l2 = max(luma(mixed), 1e-6);
    return mixed * (max(luma(c), 1e-6) / l2);
}

// Deterministischer Zufall fuer die Koernung.
fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// Weiches Rauschen auf einem Raster (Wertrauschen, bilinear).
fn rausch(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash21(i + u.saat.x);
    let b = hash21(i + vec2<f32>(1.0, 0.0) + u.saat.x);
    let c = hash21(i + vec2<f32>(0.0, 1.0) + u.saat.x);
    let d = hash21(i + vec2<f32>(1.0, 1.0) + u.saat.x);
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y) - 0.5;
}

fn basis_abtasten(p: vec2<f32>) -> vec4<f32> {
    let bw = i32(u.basis_dims.x);
    let bh = i32(u.basis_dims.y);
    let q = clamp(p * vec2<f32>(f32(bw), f32(bh)) - 0.5, vec2<f32>(0.0), vec2<f32>(f32(bw - 1), f32(bh - 1)));
    let i0 = vec2<i32>(i32(floor(q.x)), i32(floor(q.y)));
    let i1 = min(i0 + vec2<i32>(1, 1), vec2<i32>(bw - 1, bh - 1));
    let f = q - floor(q);
    let a = textureLoad(basis, i0, 0);
    let b = textureLoad(basis, vec2<i32>(i1.x, i0.y), 0);
    let c = textureLoad(basis, vec2<i32>(i0.x, i1.y), 0);
    let d = textureLoad(basis, i1, 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// ---------------------------------------------------------------------------
// Geometrie: Objektivkorrektur, Geraderichten, Farbsaeume.
//
// Sitzt hier im ERSTEN Durchgang: jedes Ausgabepixel holt sich seine Quelle
// genau einmal, und zwar aus den linearen Kameradaten — Rot und Blau jeweils
// an ihrer eigenen, um den Farbsaum korrigierten Stelle. (Frueher entzerrte der
// zweite Durchgang bei JEDEM der Nachbarzugriffe der Rauschminderung neu.)
// ---------------------------------------------------------------------------
fn tab12(a: array<vec4<f32>, 4>, i: i32) -> f32 {
    let b = a;
    return b[i / 4][i % 4];
}

// Lineare Interpolation in einer Stuetzstellen-Tabelle (n Eintraege).
fn stuetz(knoten: array<vec4<f32>, 4>, werte: array<vec4<f32>, 4>, n: i32, x: f32) -> f32 {
    if (x <= tab12(knoten, 0)) { return tab12(werte, 0); }
    for (var i = 1; i < n; i = i + 1) {
        let k1 = tab12(knoten, i);
        if (x <= k1) {
            let k0 = tab12(knoten, i - 1);
            let t = (x - k0) / max(k1 - k0, 1e-9);
            return mix(tab12(werte, i - 1), tab12(werte, i), t);
        }
    }
    return tab12(werte, n - 1);
}

fn geometrie_aktiv() -> bool {
    return u.geo.w > 0.5 || u.radial.x > 1.5 || abs(u.lens1.x) + abs(u.lens1.y) + abs(u.lens1.z) > 1e-7;
}

// Ausgabeposition (Pixel im VOLLEN Bild, diese Aufloesung) → Quellposition (Gruen).
fn quelle_gruen(p_aus: vec2<f32>) -> vec2<f32> {
    let mitte = u.lage.zw * 0.5;
    var d = p_aus - mitte;
    let halbdiag = max(length(mitte), 1.0);
    // Kamera-Korrektur (RAF): Quelle = m(r) · Ziel, mit Fuellmassstab.
    let n = i32(u.radial.x);
    if (n > 1) {
        d = d * u.radial.y;
        d = d * stuetz(u.r_ziel, u.r_m, n, length(d) / halbdiag);
    }
    // Objektivprofil (ptlens, lensfun-Konvention: Radius auf die kleinere Halbseite).
    let abc = u.lens1.xyz;
    if (abs(abc.x) + abs(abc.y) + abs(abc.z) > 1e-7) {
        let norm_k = max(min(mitte.x, mitte.y), 1.0);
        let r = length(d) / norm_k;
        let dk = 1.0 - abc.x - abc.y - abc.z;
        d = d * (abc.x * r * r * r + abc.y * r * r + abc.z * r + dk) * u.lens1.w;
    }
    // Manuelle Entzerrung (Tonne/Kissen).
    let k = u.geo.y;
    if (abs(k) > 1e-5) {
        let norm = max(length(mitte), 1.0);
        let r2 = dot(d, d) / (norm * norm);
        d = d * (1.0 + k * r2) / (1.0 + k * u.geo.z);
    }
    // Geraderichten.
    let a = u.geo.x;
    if (abs(a) > 1e-6) {
        let sn = sin(a);
        let cs = cos(a);
        d = vec2<f32>(d.x * cs - d.y * sn, d.x * sn + d.y * cs);
    }
    return d + mitte;
}

// Lanczos-3 aus der Quelltextur, Position in Pixeln der Textur (Mitte = +0,5).
//
// Frueher bilinear — und das kostete bei JEDEM entzerrten Foto (Objektiv-
// korrektur ist bei RAF immer an) sichtbar Schaerfe: an einem halben Pixel
// Versatz laesst lineare Interpolation 0,25 Zyklen/Pixel nur noch mit 71 %
// durch. Gemessen an DSCF1489 gegen die Referenz-Entwicklung: 0,79–0,88 der
// Feinzeichnung, ohne Entzerrung 1,0. Lanczos-3 haelt das Band bis ~0,4
// Zyklen/Pixel nahezu verlustfrei.
fn lanczos3(x: f32) -> f32 {
    let a = abs(x);
    if (a < 1e-5) { return 1.0; }
    if (a >= 3.0) { return 0.0; }
    let pi_x = 3.14159265 * x;
    return 3.0 * sin(pi_x) * sin(pi_x / 3.0) / (pi_x * pi_x);
}

fn quelle_abtasten(q: vec2<f32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(src));
    let p = q - 0.5;
    let basis = vec2<i32>(i32(floor(p.x)), i32(floor(p.y)));
    let f = p - floor(p);
    var wx: array<f32, 6>;
    var wy: array<f32, 6>;
    var sx = 0.0;
    var sy = 0.0;
    for (var i = 0; i < 6; i = i + 1) {
        wx[i] = lanczos3(f32(i - 2) - f.x);
        wy[i] = lanczos3(f32(i - 2) - f.y);
        sx = sx + wx[i];
        sy = sy + wy[i];
    }
    var acc = vec3<f32>(0.0);
    for (var j = 0; j < 6; j = j + 1) {
        let yy = clamp(basis.y + j - 2, 0, dims.y - 1);
        var zeile = vec3<f32>(0.0);
        for (var i = 0; i < 6; i = i + 1) {
            let xx = clamp(basis.x + i - 2, 0, dims.x - 1);
            zeile = zeile + textureLoad(src, vec2<i32>(xx, yy), 0).rgb * wx[i];
        }
        acc = acc + zeile * wy[j];
    }
    // Die Ueberschwinger eines Kerns mit negativen Keulen duerfen keine
    // negative Lichtmenge erzeugen.
    return max(acc / (sx * sy), vec3<f32>(0.0));
}

fn rampe(x: f32, schw: f32) -> f32 {
    let rad = schw * 0.5;
    let steig = 1.0 / (1.0 - schw);
    if (x <= schw - rad) { return 0.0; }
    if (x >= schw + rad) { return (x - schw) * steig; }
    let y = x - (schw - rad);
    return steig / (4.0 * rad) * y * y;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(dst);
    if (gid.x >= dims.x || gid.y >= dims.y) { return; }
    let xy = vec2<i32>(i32(gid.x), i32(gid.y));
    // Lage im GANZEN Bild (0..1) — Vorschau, 1:1-Kachel und Export sehen
    // an derselben Bildstelle dieselbe Basis, Vignette und Randaufhellung.
    let p_voll = (vec2<f32>(f32(gid.x), f32(gid.y)) + 0.5 + u.lage.xy) / u.lage.zw;

    // 0) Geometrie: woher kommt dieses Pixel? Rot/Gruen/Blau getrennt, damit
    //    die Farbsaeume des Objektivs verschwinden.
    var cam = vec3<f32>(0.0);
    var p_quelle = p_voll;
    if (geometrie_aktiv()) {
        let p_aus = vec2<f32>(f32(gid.x), f32(gid.y)) + 0.5 + u.lage.xy;
        let q_g = quelle_gruen(p_aus);
        var q_r = q_g;
        var q_b = q_g;
        let n = i32(u.radial.x);
        if (n > 1 && u.radial.z > 0.5) {
            let mitte = u.lage.zw * 0.5;
            let rq = length(q_g - mitte) / max(length(mitte), 1.0);
            q_r = mitte + (q_g - mitte) * (1.0 + stuetz(u.r_quelle, u.r_ca_r, n, rq));
            q_b = mitte + (q_g - mitte) * (1.0 + stuetz(u.r_quelle, u.r_ca_b, n, rq));
        }
        cam = vec3<f32>(
            quelle_abtasten(q_r - u.lage.xy).r,
            quelle_abtasten(q_g - u.lage.xy).g,
            quelle_abtasten(q_b - u.lage.xy).b
        );
        p_quelle = q_g / u.lage.zw;
    } else {
        cam = textureLoad(src, xy, 0).rgb;
    }

    // 1) Kamera → lineares ROMM-RGB (Weissabgleich + Profilmatrix, Sensor-Weiss = 1).
    var c = max(zeilen(u.kamera_zu_pro, cam), vec3<f32>(0.0));

    // 2) Objektiv-Randaufhellung — im Sensorbezug, um die optische Mitte:
    //    aus den Kameradaten (RAF) exakt, dazu der Regler „Vignette entfernen".
    var rand_ev = 0.0;
    let n_r = i32(u.radial.x);
    if (n_r > 1 && u.radial.w > 0.5) {
        let rq = length((p_quelle - 0.5) * u.lage.zw) / max(length(u.lage.zw * 0.5), 1.0);
        rand_ev = log2(max(stuetz(u.r_quelle, u.r_gain, n_r, rq), 0.1));
    }
    if (u.lokal.z > 1e-5) {
        let d = distance(p_quelle, vec2<f32>(0.5, 0.5)) / 0.7071;
        rand_ev = rand_ev + log2(1.0 + u.lokal.z * smoothstep(0.35, 1.0, d) * 0.8);
    }

    // 3) Farbtabelle des Profils (vor der Belichtung, wie im DNG-SDK).
    if (u.tabellen.x == 1u) {
        c = max(farbtabelle(c, u.hsm_dims, u.tabellen.z, true), vec3<f32>(0.0));
    }

    // 4) Belichtung + lokale Tonregler. Die Basis ist auf den Pixeln OHNE
    //    Belichtung und Randaufhellung gerechnet; beides sind (nahezu)
    //    konstante Faktoren und verschieben die Basis exakt mit.
    let ev = u.belichtung.x;
    let l_roh = log2(max(luma(c), LOG_BODEN));
    let k = basis_abtasten(p_quelle);
    let versatz = ev + rand_ev;
    let b_ton = k.x * l_roh + k.y + versatz;
    let b_klar = k.z * l_roh + k.w + versatz;
    let l_px = l_roh + versatz;
    c = c * exp2(versatz);

    var g = lichter_tiefen(b_ton, versatz);
    // Detail-Erhalt: wo Lichter/Tiefen die Umgebung verschieben, bleibt die
    // Zeichnung kraeftiger stehen (lokale Laplace-Filter tun dasselbe).
    if (abs(u.kal[4].w) > 1e-5) {
        g = g + u.kal[4].w * abs(g) * clamp(l_px - b_ton, -1.0, 1.0);
    }
    // Klarheit: Mittelton-Lokalkontrast auf mittlerer Groessenordnung.
    let klarheit = u.lokal.x;
    if (abs(klarheit) > 1e-5) {
        let detail = clamp(l_px - b_klar, -1.5, 1.5);
        let mitte = 1.0 - smoothstep(0.0, 3.5, abs(b_klar - u.kal[2].z));
        g = g + klarheit * u.kal[2].w * detail * mitte;
    }
    // Dunst: grossflaechiger Lokalkontrast + helle Schleier absenken.
    let dunst = u.ton2.w;
    if (abs(dunst) > 1e-5) {
        let detail = clamp(l_px - b_ton, -2.0, 2.0);
        let hell = smoothstep(-5.0, -1.0, b_ton);
        g = g + dunst * u.kal[3].w * (detail * 0.6 - 0.35 * hell);
    }
    c = c * exp2(g);

    // 5) Weiss und Kontrast — global auf der Helligkeit, Farbton bleibt.
    let l1 = log2(max(luma(c), LOG_BODEN));
    var l2 = l1;
    let weiss = u.ton2.x;
    if (abs(weiss) > 1e-5) {
        l2 = l2 + weiss * u.kal[1].z * smoothstep(u.kal[1].w, 0.0, l2);
    }
    if (abs(u.belichtung.y) > 1e-5) {
        l2 = kontrast(l2, 1.0 + u.belichtung.y * u.kal[2].y, u.kal[2].z);
    }
    c = c * exp2(l2 - l1);

    // 6) Schwarz: negativ setzt den Schwarzpunkt (Tiefen satt), positiv hebt ihn.
    let bp = -u.ton2.y * u.kal[2].x + u.ton2.z * 0.0005;
    if (bp > 1e-7) {
        c = max(c - vec3<f32>(bp), vec3<f32>(0.0)) / (1.0 - bp);
    } else if (bp < -1e-7) {
        c = c + vec3<f32>(-bp) * (vec3<f32>(1.0) - smoothstep(vec3<f32>(0.0), vec3<f32>(0.05), c));
    }

    // 7) Vignette nach Zuschnitt — im Linearen wie die „Lichterprioritaet" der
    //    Referenz-Entwicklung: abgedunkelt wie mit weniger Belichtung.
    if (abs(u.vign.x) > 1e-5) {
        let z = u.zuschnitt;
        let mitte = z.xy + z.zw * 0.5;
        let rel = (p_voll - mitte) / max(z.zw * 0.5, vec2<f32>(1e-4));
        let d = length(rel) / 1.4142;
        let w = smoothstep(u.vign.y, 1.0, d);
        c = c * exp2(-u.vign.x * 1.6 * w);
    }

    // 8) Schulter: weich auf Weiss zulaufen (groesster Kanal, Farbton bleibt),
    //    nahe Weiss zur Neutralen hin entsaettigen wie ein Film.
    let knie = u.kal[3].z;
    let mx = max(c.r, max(c.g, c.b));
    if (mx > knie) {
        let t = (mx - knie) / (1.0 - knie);
        let neu = knie + (1.0 - knie) * (1.0 - exp(-t));
        c = c * (neu / mx);
        let w = smoothstep(0.3, 2.5, t) * 0.6;
        c = mix(c, vec3<f32>(neu), w);
    }

    // 9) Schwarz-Abzug der Standardentwicklung des DNG-SDK („Schatten 5") mit weichem Fuss.
    let schw = u.lokal.y;
    if (schw > 0.0) {
        c = vec3<f32>(rampe(c.r, schw), rampe(c.g, schw), rampe(c.b, schw));
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));

    // 10) Look-Tabelle des Profils.
    if (u.tabellen.y == 1u) {
        c = clamp(farbtabelle(c, u.look_dims, u.tabellen.w, false), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    // 11) Grundkurve (Standard-Grundkurve des DNG-SDK oder Profilkurve), farbtonerhaltend.
    c = rgb_ton(c);

    // 12) Nutzer-Tonwertkurve (KI-Vertrag: 5 Punkte, neutral = Diagonale),
    //     auf der wahrnehmungsnahen Achse.
    let lg = max(luma(c), 1e-6);
    let enc = srgb_kod(lg);
    let mapped = curve_eval(enc);
    if (abs(mapped - enc) > 1e-6) {
        c = c * (srgb_dekod(max(mapped, 0.0)) / lg);
    }

    // 13) HSL, Dynamik, Saettigung.
    c = hsl_bands(c);
    let lv = luma(c);
    if (abs(u.farbe.x) > 1e-5) {
        let enc_c = vec3<f32>(srgb_kod(c.r), srgb_kod(c.g), srgb_kod(c.b));
        let hsv = rgb2hsv(enc_c);
        var boost = u.farbe.x * u.kal[3].x * (1.0 - hsv.y) * (1.0 - hsv.y);
        let grad = hsv.x * 360.0;
        let haut = smoothstep(8.0, 18.0, grad) * (1.0 - smoothstep(45.0, 60.0, grad));
        boost = boost * (1.0 - 0.5 * haut);
        c = mix(vec3<f32>(lv), c, 1.0 + boost);
    }
    var sat = u.farbe.y * u.kal[3].y;
    if (abs(dunst) > 1e-5) { sat = sat + dunst * 0.25; }
    if (abs(sat) > 1e-5) {
        c = mix(vec3<f32>(lv), c, 1.0 + sat);
    }
    c = max(c, vec3<f32>(0.0));

    // 14) Farbverlauf (Split-Toning) und Schwarzweiss.
    c = split_tone(c);
    if (u.farbe.z > 0.5) {
        c = vec3<f32>(luma(c));
    }

    // 15) Koernung — in Sensorpixeln definiert, damit Vorschau und Export
    //     dasselbe Korn zeigen (die Vorschau mittelt mehrere Sensorpixel).
    if (u.farbe.w > 1e-5) {
        let px_sensor = p_voll * u.lage.zw / max(u.vign.w, 1e-4);
        let groesse = max(u.vign.z, 1.0);
        let n = rausch(px_sensor / groesse) + 0.5 * rausch(px_sensor / (groesse * 0.5) + 17.0);
        let mittelung = clamp(groesse * u.vign.w, 0.15, 1.0);
        let mitt = 1.0 - abs(clamp(srgb_kod(luma(c)), 0.0, 1.0) - 0.5) * 2.0;
        c = c * (1.0 + n * u.farbe.w * 0.35 * mitt * mittelung);
        c = max(c, vec3<f32>(0.0));
    }

    textureStore(dst, xy, vec4<f32>(c, 1.0));
}
