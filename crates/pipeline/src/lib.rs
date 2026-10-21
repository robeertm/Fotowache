//! GPU-Bildverarbeitung: haelt das Bild auf der Grafikkarte und wendet die Regler an.
//!
//! Aufbau seit Prozess 2 (Fotowache 2.0): zwei Rechendurchgaenge (`develop.wgsl`,
//! `output.wgsl`) ueber eine Zwischentextur in `rgba16float`. Der erste rechnet
//! wie die Referenz-Entwicklung in linearem ROMM-RGB mit dem Kameraprofil
//! (installiertes DCP, wenn vorhanden) und lokalen Tonreglern auf einer
//! kantenerhaltenden Basisebene; der zweite macht Detail (Struktur, Rauschen,
//! Schaerfe), Geometrie und Ausgabe.
//!
//! **Warum eine verkleinerte Arbeitskopie fuers Regeln?** Ein 40-MP-Bild belegt als
//! Gleitkomma-Textur ueber 300 MB — pro Zwischenstufe. Waehrend man Regler zieht,
//! braucht man das nicht: der Bildschirm zeigt ohnehin nur ein paar Millionen Pixel.
//! Exportiert wird dagegen aus voller Aufloesung. Alles, was Nachbarpixel braucht,
//! ist seit Prozess 2 in SENSORPIXELN definiert (oder ueber die feste Basisebene) —
//! Vorschau, 1:1-Ausschnitt und Export zeigen dadurch dasselbe Bild.

pub mod grundkurve;
pub mod looks;
pub mod auto;
pub mod basis;
pub mod color;
pub mod dcp;
pub mod exif;
pub mod heal;
pub mod heal_ki;
pub mod zusatz;
pub mod icc;
pub mod xmp;
pub mod monitor;
pub mod objektiv;
pub mod objektivdaten;
pub mod panorama;
pub mod params;

use params::Edit;
use std::sync::{Arc, Mutex, RwLock};

/// Wohin die zweite Stufe rechnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ziel {
    /// Auf den erkannten Bildschirm (Monitorprofil), sonst sRGB.
    Bildschirm,
    /// sRGB — fuer alles, was als Datei das Haus verlaesst.
    Datei,
}

/// Die abgestimmten Konstanten von Prozess 2 — gegen echte
/// Referenz-Exporte gemessen (Kalibriersatz `testdata/referenz`, Sonde
/// `--example kalibrieren`). An EINER Stelle, damit Schader, Basisebene und
/// Automatik dieselben Zahlen sehen.
#[derive(Debug, Clone, PartialEq)]
pub struct Kalibrierung {
    /// Schader-Konstanten, siehe `kal` in develop.wgsl:
    /// [0] Tiefen-Hub (EV bei +100), Tiefen-Fenster von/bis (EV unter Weiss), Lichter-Hub
    /// [1] Lichter-Fenster von/bis, Weiss-Hub, Weiss-Fenster ab
    /// [2] Schwarz-Abzug bei −100, Kontrast-Staerke, Kontrast-Drehpunkt (EV), Klarheit-Staerke
    /// [3] Dynamik-Staerke, Saettigungs-Staerke, Schulter-Knie, Dunst-Staerke
    /// [4] Kurve je Kanal (0 = farbtonerhaltend, 1 = je Kanal), Lichter-Fenster
    ///     folgt dem Bildumfang, Tiefen-Fenster folgt dem Bildumfang, Detail-Erhalt
    /// [5] frei
    pub kal: [f32; 24],
    pub basis: basis::BasisParameter,
    /// Grundbelichtung der Standardentwicklung (DNG-Spezifikation: BaselineExposure) —
    /// der Abstand zwischen „Sensor-Weiss = 1" und dem, was die Referenz-Entwicklung
    /// bei Belichtung 0 zeigt.
    pub grund_ev: f32,
    /// Schwarz-Abzug der Standardentwicklung („Schatten 5" im DNG-SDK).
    pub profil_schwarz: f32,
    /// Wie stark der Belichtungs-Versatz der RAF-Kameras ausgeglichen wird
    /// (1 = ganz). Die Kamera belichtet das RAW absichtlich knapper (Testkamera:
    /// −0,72 EV, mit DR200 −1,72 EV) — die Referenz-Entwicklung gleicht das aus,
    /// also wir auch.
    pub versatz_faktor: f32,
}

impl Default for Kalibrierung {
    fn default() -> Self {
        Self {
            kal: [
                2.6, -8.75, -3.75, 1.4, //
                -5.25, 0.05, 1.35, -5.75, //
                0.03, 1.75, -2.47, 0.7, //
                3.375, 0.625, 0.384375, 0.6, //
                1.0, -0.8, -0.2, 0.0, //
                0.0, 0.0, 0.0, 0.0, //
            ],
            // Gegen Referenz-Exporte kalibriert (24 Fotos, zwei RAF-Kameras,
            // Sonde `kalibrieren`, Stand 2026-10-20).
            basis: basis::BasisParameter {
                radius_ton: 0.004687,
                eps_ton: 1.125,
                radius_klar: 0.007,
                eps_klar: 0.03,
            },
            grund_ev: -0.35,
            profil_schwarz: 0.0,
            versatz_faktor: 1.0,
        }
    }
}

static KALIBRIERUNG: RwLock<Option<Kalibrierung>> = RwLock::new(None);

/// Die gueltige Kalibrierung (Standard, ausser eine Messsonde hat sie gesetzt).
pub fn kalibrierung() -> Kalibrierung {
    KALIBRIERUNG
        .read()
        .ok()
        .and_then(|k| k.clone())
        .unwrap_or_default()
}

/// Nur fuer Messsonden: Konstanten zur Laufzeit tauschen.
pub fn setze_kalibrierung(k: Kalibrierung) {
    if let Ok(mut g) = KALIBRIERUNG.write() {
        *g = Some(k);
    }
}

/// Wie viele Mired (Skala der Referenz-Entwicklung) eine Einheit des
/// Temperatur-Reglers verschiebt. Der Regler bleibt relativ zur Aufnahme
/// (KI-Vertrag: −100..100), gerechnet wird aber wie in der Referenz-Entwicklung
/// auf der Planck-Kurve.
pub const MIRED_JE_EINHEIT: f64 = 1.25;
/// Eine Toenungs-Einheit = eine Toenungs-Einheit der Referenz-Entwicklung.
pub const TOENUNG_JE_EINHEIT: f64 = 1.0;

/// Ziel-Weissabgleich (Kelvin, Toenung) aus den relativen Reglern;
/// `None` = wie aufgenommen.
pub fn wb_ziel(kelvin_aufnahme: f64, toenung_aufnahme: f64, temp: f32, tint: f32) -> Option<(f64, f64)> {
    if temp == 0.0 && tint == 0.0 {
        return None;
    }
    // Waermer (temp > 0) heisst: hoehere Kelvin-Einstellung = weniger Mired.
    let mired = (1.0e6 / kelvin_aufnahme.max(1000.0) - temp as f64 * MIRED_JE_EINHEIT)
        .clamp(1.0e6 / 50000.0, 1.0e6 / 2000.0);
    Some((1.0e6 / mired, toenung_aufnahme + tint as f64 * TOENUNG_JE_EINHEIT))
}

/// Umkehrung: welche Reglerwerte ergeben (Kelvin, Toenung)?
pub fn wb_regler(kelvin_aufnahme: f64, toenung_aufnahme: f64, kelvin: f64, toenung: f64) -> (f32, f32) {
    let temp = (1.0e6 / kelvin_aufnahme.max(1000.0) - 1.0e6 / kelvin.max(1000.0)) / MIRED_JE_EINHEIT;
    let tint = (toenung - toenung_aufnahme) / TOENUNG_JE_EINHEIT;
    (temp as f32, tint as f32)
}

pub struct Gpu {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub backend: String,
    /// Erkanntes Bildschirmprofil. `None` = wir nehmen sRGB an.
    pub monitor: Option<monitor::MonitorProfil>,
    develop: wgpu::ComputePipeline,
    output: wgpu::ComputePipeline,
    /// Zweite Ausgabestufe, die nach `rgba16float` schreibt. Gleicher Shader,
    /// nur ein anderes Ziel — dadurch faellt beim Export die 8-Bit-Stufe weg,
    /// die vorher jede Tonwertabstufung gekostet hat.
    output16: wgpu::ComputePipeline,
    /// Umkehrkurve des Bildschirms auf der Grafikkarte. Liegt immer bereit, auch
    /// ohne erkanntes Profil — dann ungenutzt. Das spart eine zweite Variante
    /// jeder Bindegruppe.
    trc_buf: wgpu::Buffer,
}

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("keine geeignete Grafikkarte gefunden")]
    NoAdapter,
    #[error("Grafikkarte abgelehnt: {0}")]
    Device(String),
    #[error("Bild ist leer")]
    Empty,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stufe {
    Entwickeln,
    Ausgabe,
}

impl Gpu {
    pub async fn new() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::default();
        // Erst die echte Grafikkarte; gibt es keine (virtuelle Maschinen,
        // Bauknechte, Remote-Desktop), den Software-Renderer des Systems —
        // langsamer, aber das Programm laeuft statt mit „keine Grafikkarte"
        // stehenzubleiben.
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
        {
            Ok(a) => a,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
                .map_err(|_| GpuError::NoAdapter)?,
        };
        let info = adapter.get_info();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fotowache"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    // 40-MP-Bilder sind breiter als die Vorgabe von 8192.
                    max_texture_dimension_2d: 16384,
                    ..wgpu::Limits::defaults()
                },
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let develop = make_pipeline(
            &device,
            include_str!("develop.wgsl"),
            "develop",
            wgpu::TextureFormat::Rgba16Float,
            Stufe::Entwickeln,
        );
        let output = make_pipeline(
            &device,
            include_str!("output.wgsl"),
            "output",
            wgpu::TextureFormat::Rgba8Unorm,
            Stufe::Ausgabe,
        );

        // Der Shader gibt sein Ziel im Quelltext an (`texture_storage_2d<...>`).
        // Fuer die 16-Bit-Stufe tauschen wir genau diese eine Zeile aus, statt den
        // ganzen Shader ein zweites Mal zu pflegen — sonst laufen beide auseinander.
        let src16 = include_str!("output.wgsl").replace(
            "texture_storage_2d<rgba8unorm, write>",
            "texture_storage_2d<rgba16float, write>",
        );
        let output16 = make_pipeline(
            &device,
            &src16,
            "output16",
            wgpu::TextureFormat::Rgba16Float,
            Stufe::Ausgabe,
        );

        // Bildschirmprofil einmal beim Start lesen. Wechselt man den Bildschirm
        // oder kalibriert neu, greift es beim naechsten Start.
        let monitor = monitor::hauptbildschirm();
        let trc = match &monitor {
            Some(p) => p.trc_flat(),
            None => vec![0f32; monitor::TRC_LEN * 3],
        };
        let trc_buf = speicher_puffer(&device, bytemuck::cast_slice(&trc), "bildschirmkurve");

        Ok(Self {
            device,
            queue,
            backend: format!("{} ({:?})", info.name, info.backend),
            monitor,
            develop,
            output,
            output16,
            trc_buf,
        })
    }

    /// Wie der Bildschirm in der Fusszeile beschrieben wird.
    pub fn monitor_text(&self) -> String {
        match &self.monitor {
            Some(p) => format!("{} · {:.0} % sRGB-Fläche", p.name, p.gamut_vs_srgb * 100.0),
            None => "Bildschirm: sRGB angenommen".into(),
        }
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    src: &str,
    label: &str,
    store_format: wgpu::TextureFormat,
    stufe: Stufe,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let textur = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let speicher = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![
        textur(0),
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: store_format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        speicher(3),
    ];
    if stufe == Stufe::Entwickeln {
        entries.push(textur(4));
    }
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// Uniform-Block des zweiten Durchgangs (Layout wie `OutUniforms` in output.wgsl).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OutUniform {
    work_to_srgb: [[f32; 4]; 3],
    detail: [f32; 4],
    detail2: [f32; 4],
    flags: [f32; 4],
}

/// Rauschpegel einer f16-Textur schaetzen — und zwar genau in der Groesse,
/// die die NLM-Rauschminderung vergleicht: als typischer **Flecken-Abstand**
/// (mittleres Fehlerquadrat der Luminanz ueber 3x3) zwischen benachbarten
/// Flecken. Zurueck kommt sigma = sqrt(d2_flach / 2).
///
/// Warum nicht die Theorie (MAD der zweiten Differenz)? Echtes Rauschen nach
/// dem Demosaicing ist raeumlich KORRELIERT (die Interpolation des 6x6-Farbmusters schmiert
/// es ueber 2–3 Pixel) — die Hochfrequenz-Schaetzung sah davon nur einen
/// Bruchteil, der Sockel im Shader blieb zu klein, alle Gewichte kollabierten
/// und auf echten Fotos blieb das Rauschen einfach stehen (auf dem
/// unkorrelierten Kunstbild der Sonde dagegen nicht — fiese Falle).
/// Das 20. Perzentil nimmt die flachen Paare; Kanten liegen weit darueber.
pub fn rauschpegel_messen(data: &[u16], w: u32, h: u32) -> f32 {
    let (w, h) = (w as usize, h as usize);
    if w < 16 || h < 16 || data.len() < w * h * 4 {
        return 0.0;
    }
    let luma = |x: usize, y: usize| -> f32 {
        let i = (y * w + x) * 4;
        0.2126 * half::f16::from_bits(data[i]).to_f32()
            + 0.7152 * half::f16::from_bits(data[i + 1]).to_f32()
            + 0.0722 * half::f16::from_bits(data[i + 2]).to_f32()
    };
    // Fleckenabstand wie im Shader: 3x3, Versatz (3,2) — ein typischer
    // NLM-Nachbar, weit genug, dass auch korreliertes Rauschen entkoppelt.
    let ziel = 15_000usize;
    let zeilen = ((h * w) / (ziel * 8)).max(1);
    let mut abstaende: Vec<f32> = Vec::with_capacity(ziel + 1024);
    let mut y = 4;
    while y + 8 < h {
        let mut x = 4;
        while x + 8 < w {
            let mut d2 = 0.0f32;
            for py in 0..3usize {
                for px in 0..3usize {
                    let dl = luma(x + px, y + py) - luma(x + px + 3, y + py + 2);
                    d2 += dl * dl;
                }
            }
            let d2 = d2 / 9.0;
            if d2.is_finite() {
                abstaende.push(d2);
            }
            x += 8;
        }
        y += zeilen.max(3);
    }
    if abstaende.len() < 64 {
        return 0.0;
    }
    // 35. Perzentil: tief genug, dass Kanten und Zeichnung (weit oben in der
    // Verteilung) nicht hineinspielen — hoch genug, dass der Sockel den
    // typischen flachen Paar-Abstand wirklich abdeckt. Beim 20. blieb die
    // Glaettung auf korreliertem Rauschen zu zaghaft (1,7x statt >2,5x).
    let k = abstaende.len() * 35 / 100;
    let (_, wert, _) = abstaende
        .select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (*wert / 2.0).max(0.0).sqrt().clamp(0.0, 0.25)
}

/// Zwischenspeicher eines Bildes: was nur vom Weissabgleich abhaengt, wird
/// nicht bei jedem Reglerzug neu gerechnet.
#[derive(Default)]
struct Zwischen {
    farb_schluessel: Option<(i64, i64)>,
    farbstand: Option<dcp::Farbstand>,
    tab_buf: Option<wgpu::Buffer>,
    tabellen: [u32; 4],
    hsm_dims: [u32; 4],
    look_dims: [u32; 4],
    basis_schluessel: Option<([i64; 3], basis::BasisParameter)>,
    basis_tex: Option<(wgpu::Texture, u32, u32, [f32; 4])>,
}

/// Ein geladenes Bild auf der GPU.
/// Ergebnis von `GpuImage::kachelweise`.
pub enum Gerendert {
    /// 8 Bit, RGBA verschraenkt (wie `render`).
    Rgba8 { w: u32, h: u32, px: Vec<u8> },
    /// 16 Bit, RGB verschraenkt (wie `render16`).
    Rgb16 { w: u32, h: u32, px: Vec<u16> },
}

/// Bis zu dieser Kante wird ein Bild in einem Stueck entwickelt; darueber in
/// Kacheln (Texturgrenze 16384, Rueckholpuffer, Speicher der Grafikkarte).
pub const KACHEL_KANTE: u32 = 8192;

pub struct GpuImage {
    pub width: u32,
    pub height: u32,
    src: wgpu::Texture,
    mid: wgpu::Texture,
    dst: wgpu::Texture,
    meta: rawcore::RawMeta,
    /// Das Kameraprofil (das installierte Standardprofil, sonst Fotowaches
    /// Rueckfall auf LibRaws Matrix).
    pub profil: Arc<dcp::Kameraprofil>,
    /// Fotowaches eigene Wiedergabe (LibRaw-Matrix + Grundkurve) — waehlbar.
    pub profil_eigen: Arc<dcp::Kameraprofil>,
    /// Erkanntes Objektiv samt gemessener Randabdunklung. Wird vom Lader
    /// gesetzt (`erkennen` braucht die Rohpixel); Standard = neutral.
    pub objektiv: objektiv::Objektiv,
    /// Lage dieser Textur im vollen Bild (fuer den 1:1-Ausschnitt) und die
    /// volle Bildgroesse im selben Massstab. Beim ganzen Bild: (0,0,w,h).
    warp_offset: [f32; 2],
    warp_voll: [f32; 2],
    /// Gemessener Rauschpegel DIESER Textur (Standardabweichung der
    /// Luminanz, linear). Die verkleinerte Vorschau traegt nur einen Bruchteil
    /// des Sensorrauschens, der 1:1-Ausschnitt und der Export das volle.
    pub rauschpegel: f32,
    /// Pixel dieser Textur je Sensorpixel (Vorschau < 1, Kachel/Export = 1).
    /// Alles mit Nachbarpixeln (Schaerfe, Struktur, Korn) ist in Sensorpixeln
    /// definiert und wird hiermit umgerechnet — sonst sieht die Vorschau
    /// anders aus als die Datei.
    pub massstab: f32,
    /// Verkleinerung des GANZEN Bildes fuer die Basisebene — geteilt zwischen
    /// Vorschau und 1:1-Kacheln, damit beide dieselbe Basis sehen.
    basis_quelle: Option<Arc<basis::BasisQuelle>>,
    zwischen: Mutex<Zwischen>,
}

impl GpuImage {
    /// Laedt ein Bild aus `rawcore` auf die GPU. `max_dim` verkleinert fuers Regeln;
    /// `None` = volle Aufloesung (Export).
    pub fn upload(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        max_dim: Option<u32>,
    ) -> Result<Self, GpuError> {
        Self::upload_mit(gpu, img, max_dim, &Retusche::LEER)
    }

    /// Wie `upload`, aber schliesst vorher die markierten Stellen.
    ///
    /// Die Retusche sitzt hier und nicht spaeter im Shader, weil sie sich sonst bei
    /// jedem Reglerzug neu berechnen muesste — und weil ein reparierter Fleck sich
    /// wie echtes Bild verhalten soll: Belichtung, Kontrast und Schaerfe greifen
    /// darauf genauso wie ueberall sonst.
    pub fn upload_mit(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        max_dim: Option<u32>,
        retusche: &Retusche,
    ) -> Result<Self, GpuError> {
        let (w, h, data) = Self::vorbereiten(img, max_dim, retusche)?;
        let quelle = Arc::new(basis::BasisQuelle::aus_f16(&data, w, h));
        let massstab = w as f32 / img.width.max(1) as f32;
        Self::from_f16(gpu, w, h, &data, &img.meta, Some(quelle), massstab)
    }

    /// Der CPU-Teil von `upload_mit`: verkleinern und Retuschestellen schliessen.
    ///
    /// Getrennt herausgefuehrt, damit ein Lade-Thread diese Arbeit uebernehmen
    /// kann — sie kostet bei 40 MP spuerbar Zeit, und auf dem UI-Thread heisst
    /// das: das Programm steht und „macht gar nichts".
    pub fn vorbereiten(
        img: &rawcore::RawImage,
        max_dim: Option<u32>,
        retusche: &Retusche,
    ) -> Result<(u32, u32, Vec<u16>), GpuError> {
        if img.width == 0 || img.height == 0 {
            return Err(GpuError::Empty);
        }
        let (w, h, mut data) = to_rgba_f16(img, max_dim);
        retusche.anwenden(&mut data, w, h);
        Ok((w, h, data))
    }

    /// Der GPU-Teil von `upload_mit`: nimmt das Ergebnis von `vorbereiten`
    /// (immer das GANZE Bild). `sensor_breite` = Breite des Sensorbilds.
    pub fn aus_vorbereitet(
        gpu: &Gpu,
        w: u32,
        h: u32,
        data: &[u16],
        meta: &rawcore::RawMeta,
        sensor_breite: u32,
    ) -> Result<Self, GpuError> {
        let quelle = Arc::new(basis::BasisQuelle::aus_f16(data, w, h));
        let massstab = w as f32 / sensor_breite.max(1) as f32;
        Self::from_f16(gpu, w, h, data, meta, Some(quelle), massstab)
    }

    fn from_f16(
        gpu: &Gpu,
        w: u32,
        h: u32,
        data: &[u16],
        meta: &rawcore::RawMeta,
        basis_quelle: Option<Arc<basis::BasisQuelle>>,
        massstab: f32,
    ) -> Result<Self, GpuError> {
        let src = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("quelle"),
            size: extent(w, h),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            src.as_image_copy(),
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            extent(w, h),
        );
        let mid = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zwischen"),
            size: extent(w, h),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dst = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ausgabe"),
            size: extent(w, h),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let profil = dcp::fuer_meta(meta);
        let profil_eigen = Arc::new(dcp::Kameraprofil::rueckfall(
            &meta.cam_to_xyz,
            &format!("{} {}", meta.make, meta.model),
        ));
        Ok(Self {
            width: w,
            height: h,
            src,
            mid,
            dst,
            meta: meta.clone(),
            profil,
            profil_eigen,
            objektiv: objektiv::Objektiv::default(),
            warp_offset: [0.0, 0.0],
            warp_voll: [w as f32, h as f32],
            rauschpegel: rauschpegel_messen(data, w, h),
            massstab: massstab.clamp(1e-3, 1.0),
            basis_quelle,
            zwischen: Mutex::new(Zwischen::default()),
        })
    }

    /// Traegt die Lage eines 1:1-Ausschnitts im vollen Bild ein — samt dem
    /// Objektiv des ganzen Bildes, damit der Ausschnitt gleich entzerrt wird.
    pub fn als_ausschnitt(
        &mut self,
        x: u32,
        y: u32,
        voll_breite: u32,
        voll_hoehe: u32,
        objektiv: objektiv::Objektiv,
    ) {
        self.warp_offset = [x as f32, y as f32];
        self.warp_voll = [voll_breite as f32, voll_hoehe as f32];
        self.objektiv = objektiv;
    }

    /// Uebernimmt die Basisebene eines anderen Bildes (des GANZEN Bildes).
    /// Pflicht fuer 1:1-Ausschnitte: aus dem Ausschnitt allein laesst sich die
    /// Umgebungshelligkeit am Rand nicht bestimmen — Vorschau und Zoom saehen
    /// verschieden aus.
    pub fn basis_von(&mut self, ganzes: &GpuImage) {
        self.basis_quelle = ganzes.basis_quelle.clone();
        if let Ok(mut z) = self.zwischen.lock() {
            *z = Zwischen::default();
        }
    }

    /// Laedt einen **Ausschnitt in voller Aufloesung**. Das ist der Unterschied
    /// zwischen „Vorschau vergroessert" (matschig) und echtem 100%-Zoom: statt die
    /// verkleinerte Arbeitskopie aufzublasen, holen wir die Originalpixel des
    /// sichtbaren Bereichs. Danach `als_ausschnitt` + `basis_von` aufrufen.
    pub fn upload_region(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> Result<Self, GpuError> {
        Self::upload_region_mit(gpu, img, x, y, w, h, &Retusche::LEER)
    }

    /// Wie `upload_region`, schliesst aber die markierten Stellen mit.
    /// Die Anteile beziehen sich hier auf den **Ausschnitt**, nicht aufs ganze Bild.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_region_mit(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        retusche: &Retusche,
    ) -> Result<Self, GpuError> {
        let w = w.min(img.width.saturating_sub(x)).max(1);
        let h = h.min(img.height.saturating_sub(y)).max(1);
        if w == 0 || h == 0 {
            return Err(GpuError::Empty);
        }
        let mut data = vec![0u16; (w as usize) * (h as usize) * 4];
        let one = half::f16::from_f32(1.0).to_bits();
        let scale = 1.0f32 / 65535.0;
        for row in 0..h {
            let sy = (y + row) as usize;
            for col in 0..w {
                let sx = (x + col) as usize;
                let i = (sy * img.width as usize + sx) * 3;
                let o = ((row as usize) * (w as usize) + col as usize) * 4;
                for c in 0..3 {
                    data[o + c] = half::f16::from_f32(img.pixels[i + c] as f32 * scale).to_bits();
                }
                data[o + 3] = one;
            }
        }
        retusche.anwenden(&mut data, w, h);
        Self::from_f16(gpu, w, h, &data, &img.meta, None, 1.0)
    }

    /// Ein Ausschnitt fuer den Bildschirm: der Bereich `x, y, w, h` des vollen
    /// Bildes, flaechentreu auf genau `ziel_b` × `ziel_h` verkleinert (jedes
    /// Zielpixel mittelt die Sensorflaeche, die es abdeckt — auch Bruchteile).
    ///
    /// Wozu: die eingepasste Ansicht. Die Arbeitskopie fuers Regeln ist bewusst
    /// klein (ganzzahlig geteilt: bei 7752 px nur 1938 px breit); ein HiDPI-
    /// Bildschirm zeigt aber doppelt so viele Pixel je Punkt — aufgeblasen sah
    /// jedes Foto weich aus (gemessen gegen die Referenz-Entwicklung bei
    /// 0,2–0,3 Zyklen/px: 27 % der Feinzeichnung am 5K-Schirm, 32 % am
    /// eingebauten). In Bildschirmaufloesung gerechnet: 95–98 %.
    ///
    /// Danach `als_ausschnitt_skaliert` + `basis_von` aufrufen.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_region_verkleinert(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        ziel_b: u32,
        ziel_h: u32,
        retusche: &Retusche,
    ) -> Result<Self, GpuError> {
        let w = w.min(img.width.saturating_sub(x)).max(1);
        let h = h.min(img.height.saturating_sub(y)).max(1);
        let (ob, oh) = (ziel_b.clamp(1, w), ziel_h.clamp(1, h));
        let mut data = flaechentreu_verkleinern(img, x, y, w, h, ob, oh);
        retusche.anwenden(&mut data, ob, oh);
        Self::from_f16(gpu, ob, oh, &data, &img.meta, None, ob as f32 / w as f32)
    }

    /// Wie `als_ausschnitt`, fuer einen verkleinerten Ausschnitt: Lage und
    /// Bildgroesse in Pixeln DIESER Aufloesung (Sensorpixel × Massstab).
    pub fn als_ausschnitt_skaliert(
        &mut self,
        x: f32,
        y: f32,
        voll_breite: f32,
        voll_hoehe: f32,
        objektiv: objektiv::Objektiv,
    ) {
        self.warp_offset = [x, y];
        self.warp_voll = [voll_breite, voll_hoehe];
        self.objektiv = objektiv;
    }

    /// Das Objektiv, das fuer diesen Bearbeitungsstand wirklich gilt:
    /// automatisch erkannt, oder die feste Wahl aus dem Menue. Der
    /// Fuellmassstab wird auf die Masse des GANZEN Bildes gerechnet, damit
    /// der 1:1-Ausschnitt identisch korrigiert wie die Vorschau.
    pub fn objektiv_wirksam(&self, edit: &Edit) -> objektiv::Objektiv {
        self.objektiv.gewaehlt(
            &edit.lens_profil,
            self.warp_voll[0].max(1.0) as u32,
            self.warp_voll[1].max(1.0) as u32,
        )
    }

    /// Das Profil, das fuer diesen Bearbeitungsstand gilt: "" = automatisch
    /// (das installierte Standardprofil, wenn vorhanden, sonst das der DNG),
    /// "fotoraw" = eigene Wiedergabe (Schluessel aus der Zeit vor dem Namen Fotowache), "dng" = das in der DNG eingebettete Profil,
    /// sonst ein benanntes installiertes Kameraprofil („Camera Vivid" …).
    pub fn profil_fuer(&self, edit: &Edit) -> Arc<dcp::Kameraprofil> {
        match edit.profil.as_str() {
            "" | dcp::STANDARDPROFIL => self.profil.clone(),
            "fotoraw" => self.profil_eigen.clone(),
            dcp::EINGEBETTET => dcp::eingebettet(&self.meta).unwrap_or_else(|| self.profil.clone()),
            name => dcp::benannt(&self.meta.make, &self.meta.model, name).unwrap_or_else(|| self.profil.clone()),
        }
    }

    /// Welche Profile fuer diese Kamera zur Wahl stehen (ohne „fotoraw"):
    /// die installierten Kameraprofile (DCP) und — bei DNGs — das eingebettete ("dng").
    pub fn profile_zur_wahl(&self) -> Vec<String> {
        let mut v: Vec<String> = dcp::profile_fuer_kamera(&self.meta.make, &self.meta.model)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        if dcp::eingebettet(&self.meta).is_some() {
            v.push(dcp::EINGEBETTET.to_string());
        }
        v
    }

    /// Farbstand (Profilmatrix + gemischte Farbtabelle) fuer den
    /// Weissabgleich dieses Bearbeitungsstands.
    pub fn farbstand(&self, edit: &Edit) -> dcp::Farbstand {
        // Profilwahl in den Schluessel: anderer Name = anderer Farbstand.
        let wahl = edit.profil.bytes().fold(7i64, |h, b| h.wrapping_mul(31).wrapping_add(b as i64));
        let schluessel = (
            (edit.temp * 1000.0) as i64,
            ((edit.tint * 1000.0) as i64).wrapping_mul(1_000_003).wrapping_add(wahl),
        );
        if let Ok(z) = self.zwischen.lock() {
            if z.farb_schluessel == Some(schluessel) {
                if let Some(f) = &z.farbstand {
                    return f.clone();
                }
            }
        }
        let profil = self.profil_fuer(edit);
        let wie_aufgenommen = profil.farbstand(&self.meta.wb_mul, &self.meta.wb_aufnahme, None);
        let ziel = wb_ziel(
            wie_aufgenommen.kelvin_aufnahme as f64,
            wie_aufgenommen.toenung_aufnahme as f64,
            edit.temp,
            edit.tint,
        );
        let f = match ziel {
            None => wie_aufgenommen,
            Some(z) => profil.farbstand(&self.meta.wb_mul, &self.meta.wb_aufnahme, Some(z)),
        };
        if let Ok(mut z) = self.zwischen.lock() {
            z.farb_schluessel = Some(schluessel);
            z.farbstand = Some(f.clone());
            z.tab_buf = None;
            z.basis_schluessel = None;
        }
        f
    }

    /// Grundkurve + Farbtabelle + Look-Tabelle als ein Speicherpuffer.
    fn tabellen(&self, gpu: &Gpu, fs: &dcp::Farbstand, profil: &dcp::Kameraprofil) -> (wgpu::Buffer, [u32; 4], [u32; 4], [u32; 4]) {
        if let Ok(z) = self.zwischen.lock() {
            if let Some(b) = &z.tab_buf {
                return (b.clone(), z.tabellen, z.hsm_dims, z.look_dims);
            }
        }
        let mut daten: Vec<f32> = profil.kurve.clone();
        daten.resize(1025, 1.0);
        let mut tabellen = [0u32; 4];
        let mut hsm_dims = [0u32; 4];
        let mut look_dims = [0u32; 4];
        if let Some(t) = &fs.hsm {
            hsm_dims = [t.farbtoene, t.saettigungen, t.werte, daten.len() as u32];
            tabellen[0] = 1;
            tabellen[2] = t.kodierung;
            daten.extend_from_slice(&t.daten);
        }
        if let Some(t) = &profil.look {
            look_dims = [t.farbtoene, t.saettigungen, t.werte, daten.len() as u32];
            tabellen[1] = 1;
            tabellen[3] = t.kodierung;
            daten.extend_from_slice(&t.daten);
        }
        let buf = speicher_puffer(&gpu.device, bytemuck::cast_slice(&daten), "profiltabellen");
        if let Ok(mut z) = self.zwischen.lock() {
            z.tab_buf = Some(buf.clone());
            z.tabellen = tabellen;
            z.hsm_dims = hsm_dims;
            z.look_dims = look_dims;
        }
        (buf, tabellen, hsm_dims, look_dims)
    }

    /// Die Basisebene als Textur (rgba32float, a/b fuer zwei Massstaebe).
    fn basis_textur(&self, gpu: &Gpu, fs: &dcp::Farbstand, p: &basis::BasisParameter) -> (wgpu::Texture, u32, u32, [f32; 4]) {
        // Helligkeit der Kamerawerte = Y-Zeile von ROMM-RGB · Kamera→ROMM-RGB.
        let m = fs.kamera_zu_pro;
        let y = [0.2880402f32, 0.7118741, 0.0000857];
        let zeile = [
            y[0] * m[0][0] + y[1] * m[1][0] + y[2] * m[2][0],
            y[0] * m[0][1] + y[1] * m[1][1] + y[2] * m[2][1],
            y[0] * m[0][2] + y[1] * m[1][2] + y[2] * m[2][2],
        ];
        let schluessel = (
            [
                (zeile[0] * 1e5) as i64,
                (zeile[1] * 1e5) as i64,
                (zeile[2] * 1e5) as i64,
            ],
            *p,
        );
        if let Ok(z) = self.zwischen.lock() {
            if z.basis_schluessel.as_ref() == Some(&schluessel) {
                if let Some(t) = &z.basis_tex {
                    return t.clone();
                }
            }
        }
        let b = match &self.basis_quelle {
            Some(q) => basis::Basis::berechnen(q, zeile, p),
            None => basis::Basis::neutral(),
        };
        let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("basis"),
            size: extent(b.breite, b.hoehe),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            tex.as_image_copy(),
            bytemuck::cast_slice(&b.koeff),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(b.breite * 16),
                rows_per_image: Some(b.hoehe),
            },
            extent(b.breite, b.hoehe),
        );
        let fertig = (tex, b.breite, b.hoehe, b.perzentile);
        if let Ok(mut z) = self.zwischen.lock() {
            z.basis_schluessel = Some(schluessel);
            z.basis_tex = Some(fertig.clone());
        }
        fertig
    }

    /// Blenden, die die Standardentwicklung zusaetzlich zu den Reglern gibt:
    /// Grundbelichtung + Profilversatz + DNG-Grundbelichtung + Ausgleich des
    /// Kamera-Versatzes (RAF). Eine Stelle — Schader und Rauschpegel nutzen sie.
    pub fn zusatz_ev(&self) -> f32 {
        zusatz_ev_fuer(&self.meta, &self.profil)
    }

    /// Weissabgleich der Aufnahme in den Einheiten der Referenz-Entwicklung (Kelvin, Toenung).
    pub fn weissabgleich_aufnahme(&self) -> (f32, f32) {
        let f = self.profil.farbstand(&self.meta.wb_mul, &self.meta.wb_aufnahme, None);
        (f.kelvin_aufnahme, f.toenung_aufnahme)
    }

    /// Erster-Durchgang-Uniform samt Profiltabellen und Basisebene.
    fn dev_uniform(&self, gpu: &Gpu, edit: &Edit) -> (params::EditUniform, wgpu::Buffer, wgpu::Texture) {
        let kal = kalibrierung();
        let fs = self.farbstand(edit);
        let profil = self.profil_fuer(edit);
        let (tab, tabellen, hsm_dims, look_dims) = self.tabellen(gpu, &fs, &profil);
        let (btex, bw, bh, perz) = self.basis_textur(gpu, &fs, &kal.basis);
        let o = self.objektiv_wirksam(edit);
        let mut rand = edit.lens_devignette / 100.0;
        // Gemessene Randabdunklung nur ohne Kameradaten — die Kamera kennt
        // ihre Vignettierung genau (RAF), dann wirkt die Tabelle im Schader.
        if edit.lens_auto && o.devignette > 0.0 && o.radial.is_none() {
            rand += o.devignette / 100.0;
        }
        let mut geo = geo_uniform(edit);
        let lens1 = if edit.lens_auto {
            let [a, b, c] = o.abc;
            if o.abc != [0.0; 3] {
                geo[3] = 1.0;
            }
            [a, b, c, o.skala]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        let radial = if edit.lens_auto { o.radial } else { None };
        let k = params::UniformKontext {
            kamera_zu_pro: fs.kamera_zu_pro,
            // Das GEWAEHLTE Profil bringt seinen eigenen Belichtungsversatz mit
            // (Kamera-Abgleich-Profile z. B. −0,35 EV).
            zusatz_ev: zusatz_ev_fuer(&self.meta, &profil),
            profil_schwarz: if profil.schwarz_auto { kal.profil_schwarz } else { 0.0 },
            randaufhellung: rand.clamp(0.0, 1.0),
            lage: [self.warp_offset[0], self.warp_offset[1], self.warp_voll[0], self.warp_voll[1]],
            tabellen,
            hsm_dims,
            look_dims,
            basis_dims: [bw, bh, 0, 0],
            kal: kal.kal,
            bild: perz,
            zuschnitt_sensor: zuschnitt_im_sensor(edit),
            textur_je_sensor: self.massstab,
            geo,
            lens1,
            radial,
        };
        (edit.to_uniform(&k), tab, btex)
    }

    /// Laeuft der erste Durchgang ueber die Entzerrung (Objektivkorrektur,
    /// Geraderichten, Verzeichnung)? Dieselbe Bedingung wie `geometrie_aktiv()`
    /// im Schader.
    fn geometrie_aktiv(&self, edit: &Edit) -> bool {
        if geo_uniform(edit)[3] > 0.5 {
            return true;
        }
        if !edit.lens_auto {
            return false;
        }
        let o = self.objektiv_wirksam(edit);
        o.abc != [0.0; 3] || o.radial.is_some()
    }

    /// Baut den Uniform-Block der zweiten Stufe fuer ein bestimmtes Ziel.
    ///
    /// Hier faellt die Entscheidung sRGB-oder-Bildschirm: eine Datei bekommt immer
    /// sRGB (sie muss auf fremden Geraeten stimmen), die Vorschau das erkannte
    /// Monitorprofil (sie muss auf *diesem* Geraet stimmen).
    fn out_uniform(&self, gpu: &Gpu, edit: &Edit, ziel: Ziel, linear16: bool) -> OutUniform {
        let (m, aus_tabelle) = match (ziel, &gpu.monitor) {
            (Ziel::Bildschirm, Some(p)) => (p.work_to_display, true),
            _ => (color::working_to_srgb(), false),
        };
        OutUniform {
            work_to_srgb: [
                [m[0][0], m[0][1], m[0][2], 0.0],
                [m[1][0], m[1][1], m[1][2], 0.0],
                [m[2][0], m[2][1], m[2][2], 0.0],
            ],
            detail: [
                edit.sharpen / 100.0,
                edit.sharpen_radius / 100.0,
                edit.sharpen_mask / 100.0,
                self.massstab,
            ],
            detail2: [
                edit.texture / 100.0,
                edit.noise / 100.0,
                edit.color_noise / 100.0,
                // Wirksamer Rauschpegel im Arbeitsraum: Belichtung und
                // Sensor-Weiss-Normierung verstaerken das Rauschen mit. Daran
                // haengt die Filterbreite der Rauschminderung — je Textur
                // gemessen, damit Vorschau, 1:1-Ausschnitt und Export gleich
                // GRUENDLICH entrauschen.
                (self.rauschpegel
                    * (edit.base_ev + edit.exposure / 50.0 + self.zusatz_ev()).exp2()
                    / self.meta.weiss_pegel.max(0.05)
                    * if self.geometrie_aktiv(edit) { NLM_NACH_ENTZERRUNG } else { 1.0 })
                .clamp(0.0, 0.25),
            ],
            flags: [
                if linear16 { 1.0 } else { 0.0 },
                if aus_tabelle && !linear16 { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        }
    }

    /// Wendet die Regler an und gibt das fertige Bild als RGBA8 in **sRGB** zurueck.
    /// Das ist der Weg fuer Dateien.
    pub fn render(&self, gpu: &Gpu, edit: &Edit) -> Vec<u8> {
        self.render_to(gpu, edit, Ziel::Datei)
    }

    /// Wie `render`, aber fuer den Bildschirm: rechnet ins erkannte Monitorprofil.
    pub fn render_screen(&self, gpu: &Gpu, edit: &Edit) -> Vec<u8> {
        self.render_to(gpu, edit, Ziel::Bildschirm)
    }

    /// Beide Durchgaenge in einen Befehlspuffer schreiben.
    fn durchgaenge(
        &self,
        gpu: &Gpu,
        edit: &Edit,
        ziel: Ziel,
        linear16: bool,
        ausgabe: &wgpu::TextureView,
    ) -> wgpu::CommandEncoder {
        let (u1, tab, btex) = self.dev_uniform(gpu, edit);
        let u2 = self.out_uniform(gpu, edit, ziel, linear16);
        let b1 = uniform_buffer(&gpu.device, bytemuck::bytes_of(&u1));
        let b2 = uniform_buffer(&gpu.device, bytemuck::bytes_of(&u2));
        let v_src = self.src.create_view(&Default::default());
        let v_mid = self.mid.create_view(&Default::default());
        let v_basis = btex.create_view(&Default::default());
        let g1 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entwickeln"),
            layout: &gpu.develop.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&v_src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&v_mid) },
                wgpu::BindGroupEntry { binding: 2, resource: b1.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: tab.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&v_basis) },
            ],
        });
        let pipe2 = if linear16 || ziel == Ziel::Datei && ausgabe_ist_16(linear16) { &gpu.output16 } else { &gpu.output };
        let pipe2 = if linear16 { &gpu.output16 } else { pipe2 };
        let g2 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ausgabe"),
            layout: &pipe2.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&v_mid) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(ausgabe) },
                wgpu::BindGroupEntry { binding: 2, resource: b2.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: gpu.trc_buf.as_entire_binding() },
            ],
        });
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // Zwei getrennte Durchgaenge: die Ausgabe liest die Zwischentextur, die
        // die Entwicklung schreibt. Im selben Durchgang lasen einzelne
        // Arbeitsgruppen gelegentlich noch alte Werte — gemessen: zwei Laeufe
        // desselben Fotos unterschieden sich in duennen Zeilenbaendern um bis zu
        // 20/255. Mit Durchgangsgrenze sind sie bitgleich.
        let gx = self.width.div_ceil(8);
        let gy = self.height.div_ceil(8);
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&gpu.develop);
            pass.set_bind_group(0, &g1, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe2);
            pass.set_bind_group(0, &g2, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        enc
    }

    fn render_to(&self, gpu: &Gpu, edit: &Edit, ziel: Ziel) -> Vec<u8> {
        let v_dst = self.dst.create_view(&Default::default());
        let enc = self.durchgaenge(gpu, edit, ziel, false, &v_dst);

        gpu.queue.submit(Some(enc.finish()));

        // Zurueckholen in Baendern (Zeilenlaenge auf 256 Byte ausgerichtet): ein
        // Panorama ueber ~67 MP braucht als RGBA mehr als die 256 MB, die die
        // Grafikkarte in EINEM Puffer erlaubt — gemessen: Absturz bei 274 MB.
        let padded = (self.width * 4).div_ceil(256) * 256;
        let max_bytes: u64 = 192 << 20;
        let rows_per_band = ((max_bytes / padded.max(1) as u64).max(1) as u32).min(self.height);
        let band_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rueckhol"),
            size: padded as u64 * rows_per_band as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let row = (self.width * 4) as usize;
        let mut out = vec![0u8; row * self.height as usize];
        let mut y0 = 0u32;
        while y0 < self.height {
            let rows = rows_per_band.min(self.height - y0);
            let mut e = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            e.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.dst,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &band_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(rows),
                    },
                },
                extent(self.width, rows),
            );
            gpu.queue.submit(Some(e.finish()));
            let slice = band_buf.slice(..(padded as u64 * rows as u64));
            slice.map_async(wgpu::MapMode::Read, |_| {});
            let _ = gpu.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            let mapped = slice.get_mapped_range();
            for r in 0..rows as usize {
                let s = r * padded as usize;
                let z = (y0 as usize + r) * row;
                out[z..z + row].copy_from_slice(&mapped[s..s + row]);
            }
            drop(mapped);
            band_buf.unmap();
            y0 += rows;
        }
        out
    }

    /// Entwickelt ein ganzes Bild in Kacheln von hoechstens `kante` Pixeln —
    /// fuer Bilder, die nicht in EINE Textur passen (Kante ueber 16384, z. B.
    /// Panoramen) oder deren Rueckholung zu gross wird.
    ///
    /// Gleich wie ein Stueck entwickelt, weil alles Bildweite gemeinsam bleibt:
    /// die Daten werden EINMAL fuer das ganze Bild vorbereitet (Retusche
    /// eingeschlossen), Basisebene und Rauschpegel stammen aus dem ganzen Bild,
    /// und jede Kachel entzerrt im vollen Bildbezug (`als_ausschnitt`). Der Rand
    /// um jede Kachel deckt Nachbarschaftsfilter und Entzerrung ab — wie beim
    /// 1:1-Ausschnitt der Oberflaeche.
    #[allow(clippy::too_many_arguments)]
    pub fn kachelweise(
        gpu: &Gpu,
        img: &rawcore::RawImage,
        retusche: &Retusche,
        edit: &Edit,
        objektiv: objektiv::Objektiv,
        kante: u32,
        sechzehn: bool,
    ) -> Result<Gerendert, GpuError> {
        let (w, h, data) = Self::vorbereiten(img, None, retusche)?;
        let quelle = Arc::new(basis::BasisQuelle::aus_f16(&data, w, h));
        let rausch = rauschpegel_messen(&data, w, h);
        let halbdiag = (w as f32).hypot(h as f32) / 2.0;
        let winkel = edit.straighten.to_radians().abs();
        let rand = (128.0 + objektiv.max_verschiebung(w, h) + 2.0 * (winkel / 2.0).sin() * halbdiag).ceil() as u32;
        let kante = kante.max(256);
        let kanaele = if sechzehn { 3 } else { 4 };
        let mut aus8 = if sechzehn { Vec::new() } else { vec![0u8; w as usize * h as usize * 4] };
        let mut aus16 = if sechzehn { vec![0u16; w as usize * h as usize * 3] } else { Vec::new() };
        for ky in (0..h).step_by(kante as usize) {
            for kx in (0..w).step_by(kante as usize) {
                let (kw, kh) = (kante.min(w - kx), kante.min(h - ky));
                let (rx, ry) = (kx.saturating_sub(rand), ky.saturating_sub(rand));
                let (rw, rh) = ((kx + kw + rand).min(w) - rx, (ky + kh + rand).min(h) - ry);
                let mut teil = vec![0u16; rw as usize * rh as usize * 4];
                for z in 0..rh as usize {
                    let q = ((ry as usize + z) * w as usize + rx as usize) * 4;
                    teil[z * rw as usize * 4..(z + 1) * rw as usize * 4].copy_from_slice(&data[q..q + rw as usize * 4]);
                }
                let mut stueck = Self::from_f16(gpu, rw, rh, &teil, &img.meta, Some(quelle.clone()), 1.0)?;
                stueck.rauschpegel = rausch;
                stueck.als_ausschnitt(rx, ry, w, h, objektiv);
                let (ox, oy) = ((kx - rx) as usize, (ky - ry) as usize);
                if sechzehn {
                    let px = stueck.render16(gpu, edit);
                    for z in 0..kh as usize {
                        let q = ((oy + z) * rw as usize + ox) * kanaele;
                        let d = ((ky as usize + z) * w as usize + kx as usize) * kanaele;
                        aus16[d..d + kw as usize * kanaele].copy_from_slice(&px[q..q + kw as usize * kanaele]);
                    }
                } else {
                    let px = stueck.render(gpu, edit);
                    for z in 0..kh as usize {
                        let q = ((oy + z) * rw as usize + ox) * kanaele;
                        let d = ((ky as usize + z) * w as usize + kx as usize) * kanaele;
                        aus8[d..d + kw as usize * kanaele].copy_from_slice(&px[q..q + kw as usize * kanaele]);
                    }
                }
            }
        }
        Ok(if sechzehn { Gerendert::Rgb16 { w, h, px: aus16 } } else { Gerendert::Rgba8 { w, h, px: aus8 } })
    }

    /// Wie `render`, gibt aber **16 Bit je Kanal** zurueck (RGB, verschraenkt, sRGB-kodiert).
    ///
    /// Der Unterschied ist nicht akademisch: Verlaeufe wie Himmel oder Haut zeigen
    /// bei 8 Bit sichtbare Stufen, sobald man sie spaeter noch einmal anfasst.
    pub fn render16(&self, gpu: &Gpu, edit: &Edit) -> Vec<u16> {
        let dst16 = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ausgabe16"),
            size: extent(self.width, self.height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let v_dst = dst16.create_view(&Default::default());
        // `false` heisst hier: die sRGB-Kennlinie **wird** angewandt. 16-Bit-TIFF
        // ist ein Bildschirmformat, kein Szenenformat — es traegt sRGB-Werte, nur
        // eben fein genug abgestuft, dass nichts absaeuft.
        let enc = self.durchgaenge16(gpu, edit, &v_dst);
        gpu.queue.submit(Some(enc.finish()));

        // Streifenweise zurueckholen. Ein 40-MP-Bild braucht als 16-Bit-RGBA
        // ueber 300 MB — mehr als die Grafikkarte in EINEM Puffer erlaubt
        // (256 MB). Also in Baendern, die sicher darunter bleiben.
        let padded = (self.width * 8).div_ceil(256) * 256;
        let max_bytes: u64 = 192 << 20;
        let rows_per_band = ((max_bytes / padded.max(1) as u64).max(1) as u32).min(self.height);
        let band_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rueckhol16"),
            size: padded as u64 * rows_per_band as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut out = vec![0u16; (self.width as usize) * (self.height as usize) * 3];
        let mut y0 = 0u32;
        while y0 < self.height {
            let rows = rows_per_band.min(self.height - y0);
            let mut e = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            e.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &dst16,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &band_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(rows),
                    },
                },
                extent(self.width, rows),
            );
            gpu.queue.submit(Some(e.finish()));
            let slice = band_buf.slice(..(padded as u64 * rows as u64));
            slice.map_async(wgpu::MapMode::Read, |_| {});
            let _ = gpu.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            let mapped = slice.get_mapped_range();
            for r in 0..rows as usize {
                let row = &mapped[r * padded as usize..];
                let dst_row = (y0 as usize + r) * self.width as usize;
                for x in 0..self.width as usize {
                    let sx = x * 8;
                    for c in 0..3 {
                        let bits = u16::from_le_bytes([row[sx + c * 2], row[sx + c * 2 + 1]]);
                        let v = half::f16::from_bits(bits).to_f32().clamp(0.0, 1.0);
                        out[(dst_row + x) * 3 + c] = (v * 65535.0 + 0.5) as u16;
                    }
                }
            }
            drop(mapped);
            band_buf.unmap();
            y0 += rows;
        }
        out
    }

    fn durchgaenge16(&self, gpu: &Gpu, edit: &Edit, ausgabe: &wgpu::TextureView) -> wgpu::CommandEncoder {
        let (u1, tab, btex) = self.dev_uniform(gpu, edit);
        let u2 = self.out_uniform(gpu, edit, Ziel::Datei, false);
        let b1 = uniform_buffer(&gpu.device, bytemuck::bytes_of(&u1));
        let b2 = uniform_buffer(&gpu.device, bytemuck::bytes_of(&u2));
        let v_src = self.src.create_view(&Default::default());
        let v_mid = self.mid.create_view(&Default::default());
        let v_basis = btex.create_view(&Default::default());
        let g1 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entwickeln16"),
            layout: &gpu.develop.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&v_src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&v_mid) },
                wgpu::BindGroupEntry { binding: 2, resource: b1.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: tab.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&v_basis) },
            ],
        });
        let g2 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ausgabe16"),
            layout: &gpu.output16.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&v_mid) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(ausgabe) },
                wgpu::BindGroupEntry { binding: 2, resource: b2.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: gpu.trc_buf.as_entire_binding() },
            ],
        });
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // Getrennte Durchgaenge wie in `durchgaenge` (Lesen nach Schreiben).
        let gx = self.width.div_ceil(8);
        let gy = self.height.div_ceil(8);
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&gpu.develop);
            pass.set_bind_group(0, &g1, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&gpu.output16);
            pass.set_bind_group(0, &g2, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        enc
    }
}

/// Zusaetzliche Blenden der Standardentwicklung fuer ein Foto (ohne GPU) —
/// dieselbe Rechnung wie `GpuImage::zusatz_ev`.
pub fn zusatz_ev_fuer(meta: &rawcore::RawMeta, profil: &dcp::Kameraprofil) -> f32 {
    let k = kalibrierung();
    k.grund_ev + profil.belichtung_versatz + meta.grundbelichtung_dng - k.versatz_faktor * meta.belichtungsversatz
}

/// Schnelle Entwicklung auf der CPU — fuer Miniaturen, wenn die Datei keine
/// eingebettete Vorschau traegt. Kameraprofil, Belichtung und Grundkurve wie
/// auf der GPU; lokale Regler und Tabellen bleiben aussen vor.
pub fn schnellbild(raw: &rawcore::RawImage, max: u32, base_ev: f32) -> (u32, u32, Vec<u8>) {
    let step = (raw.width.max(raw.height) as f32 / max as f32).ceil().max(1.0) as u32;
    let w = (raw.width / step).max(1);
    let h = (raw.height / step).max(1);
    let profil = dcp::fuer_meta(&raw.meta);
    let fs = profil.farbstand(&raw.meta.wb_mul, &raw.meta.wb_aufnahme, None);
    let m = fs.kamera_zu_pro;
    let aus = color::working_to_srgb();
    let gain = (base_ev + zusatz_ev_fuer(&raw.meta, &profil)).exp2();
    let kurve = |x: f32| -> f32 {
        let v = x.clamp(0.0, 1.0) * 1024.0;
        let i = (v as usize).min(1023);
        let f = v - i as f32;
        profil.kurve[i] + (profil.kurve[i + 1] - profil.kurve[i]) * f
    };
    let kod = |x: f32| -> f32 {
        let v = x.clamp(0.0, 1.0);
        if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    let mut out = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let sx = (x * step).min(raw.width - 1);
            let sy = (y * step).min(raw.height - 1);
            let i = ((sy as usize) * (raw.width as usize) + sx as usize) * 3;
            let c = [
                raw.pixels[i] as f32 / 65535.0,
                raw.pixels[i + 1] as f32 / 65535.0,
                raw.pixels[i + 2] as f32 / 65535.0,
            ];
            let mut p = [0f32; 3];
            for r in 0..3 {
                p[r] = ((0..3).map(|k| m[r][k] * c[k]).sum::<f32>() * gain).max(0.0);
            }
            // Weiche Schulter auf den groessten Kanal, dann Grundkurve je Kanal.
            let mx = p[0].max(p[1]).max(p[2]);
            if mx > 0.8 {
                let t = (mx - 0.8) / 0.2;
                let neu = 0.8 + 0.2 * (1.0 - (-t).exp());
                for v in p.iter_mut() {
                    *v *= neu / mx;
                }
            }
            for v in p.iter_mut() {
                *v = kurve(*v);
            }
            let d = ((y * w + x) * 4) as usize;
            for r in 0..3 {
                let s: f32 = (0..3).map(|k| aus[r][k] * p[k]).sum();
                out[d + r] = (kod(s) * 255.0 + 0.5) as u8;
            }
        }
    }
    (w, h, out)
}

/// Zuschnitt (in Anzeige-Lage, nach Drehung/Spiegelung) zurueck in die Lage
/// des Sensorbilds — dort rechnet der Schader die Vignette.
pub fn zuschnitt_im_sensor(edit: &Edit) -> [f32; 4] {
    let Some([x, y, w, h]) = edit.crop else {
        return [0.0, 0.0, 1.0, 1.0];
    };
    let (ax, ay) = anzeige_zu_sensor(x, y, edit);
    let (bx, by) = anzeige_zu_sensor(x + w, y + h, edit);
    let (x0, x1) = (ax.min(bx), ax.max(bx));
    let (y0, y1) = (ay.min(by), ay.max(by));
    [x0, y0, (x1 - x0).max(1e-3), (y1 - y0).max(1e-3)]
}

/// Ein Punkt (Anteile) im angezeigten Bild → derselbe Punkt im Sensorbild.
/// Umkehrung von `ausrichten`: erst die Spiegelung, dann die Drehung zurueck.
pub fn anzeige_zu_sensor(u: f32, v: f32, edit: &Edit) -> (f32, f32) {
    let u = if edit.flip_h { 1.0 - u } else { u };
    let v = if edit.flip_v { 1.0 - v } else { v };
    match edit.rotate.rem_euclid(360) {
        90 => (v, 1.0 - u),
        180 => (1.0 - u, 1.0 - v),
        270 => (1.0 - v, u),
        _ => (u, v),
    }
}

fn ausgabe_ist_16(linear16: bool) -> bool {
    linear16
}

fn speicher_puffer(device: &wgpu::Device, bytes: &[u8], label: &str) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytes,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

#[cfg(test)]
mod schwarzpunkt_tests {
    use super::*;

    fn testraw(pixel: Vec<u16>, w: u32, h: u32) -> rawcore::RawImage {
        rawcore::RawImage {
            width: w,
            height: h,
            pixels: pixel,
            meta: rawcore::RawMeta {
                make: "Test".into(),
                model: "Schwarz".into(),
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

    /// Dunkle Bildanteile muessen den Schwarzpunkt verankern — genau der
    /// Unterschied zwischen dem satten Schwarz der Referenz-Entwicklung und unserem
    /// flachen, verwaschenen Bild (Vergleich einer Innenaufnahme).
    #[test]
    fn dunkle_tiefen_verankern_das_schwarz() {
        let (w, h) = (64u32, 64u32);
        let mut px = vec![20000u16; (w * h * 3) as usize];
        // 10 % der Pixel sind fast schwarz (angehobene Tiefen einer Szene).
        for p in 0..(w * h) as usize / 10 {
            for c in 0..3 {
                px[p * 3 + c] = 300;
            }
        }
        let raw = testraw(px, w, h);
        // Mit +2 EV Grundhelligkeit landen die Tiefen deutlich ueber dem
        // Ziel (0,011) und muessen zurueckverankert werden.
        let b = auto_blackpoint(&raw, 2.0);
        assert!(b > 2.0, "dunkle Tiefen muessen verankert werden, war {b}");
        assert!(b <= 30.0);
        // Ohne Anhebung liegen sie schon unterm Ziel — nichts anfassen.
        assert_eq!(auto_blackpoint(&raw, 0.0), 0.0);
    }

    /// Ein grosser dunkelgrauer Bildanteil (Anzug, Jacke im Schatten) ist
    /// KEIN Schwarzpunkt — ein Gruppenportraet soff ab, weil das
    /// 2. Perzentil mitten im Anzug lag und mit verankert wurde. Der
    /// Schattenmasse-Schutz muss den Eingriff hart begrenzen.
    #[test]
    fn dunkler_anzug_ist_kein_schwarzpunkt() {
        let (w, h) = (64u32, 64u32);
        let mut px = vec![30000u16; (w * h * 3) as usize];
        // Ein Drittel des Bildes: gleichmaessig dunkelgrau (kein echtes Schwarz).
        for p in 0..(w * h) as usize / 3 {
            for c in 0..3 {
                px[p * 3 + c] = 1500;
            }
        }
        let raw = testraw(px, w, h);
        let b = auto_blackpoint(&raw, 1.0);
        // Erlaubt ist hoechstens der Schutz-Deckel (20 % der p10-Helligkeit,
        // hier rechnerisch 18); die ungebremste Formel haette weit ueber 70
        // gesetzt. Der Wert selbst ist gegen fertige Referenz-Exporte kalibriert —
        // die Referenz-Entwicklung verankert auch Anzug-Fotos tiefer als die Kamera.
        assert!(b <= 19.0, "Anzug wurde verankert: {b}");
    }

    /// Ohne echte Tiefen (High-Key, Nebel) bleibt der Schwarzpunkt stehen —
    /// sonst wuerden absichtlich helle Bilder zugeschnuert.
    #[test]
    fn hochton_bleibt_unangetastet() {
        let (w, h) = (32u32, 32u32);
        let px = vec![30000u16; (w * h * 3) as usize];
        let raw = testraw(px, w, h);
        assert_eq!(auto_blackpoint(&raw, 0.0), 0.0);
    }

    /// Bildschirm-Ausschnitt: trifft die Zielgroesse genau (auch krumme
    /// Teilungen), mittelt flaechentreu und laesst Helligkeit unveraendert.
    #[test]
    fn flaechentreu_trifft_jede_groesse() {
        let (w, h) = (101u32, 67u32);
        // Senkrechte Streifen 0 / 60000 im Wechsel: Mittel 30000.
        let px: Vec<u16> = (0..w * h)
            .flat_map(|i| {
                let v = if (i % w) % 2 == 0 { 0 } else { 60000 };
                [v, v, v]
            })
            .collect();
        let raw = testraw(px, w, h);
        let ziel = (37u32, 25u32);
        let d = flaechentreu_verkleinern(&raw, 0, 0, w, h, ziel.0, ziel.1);
        assert_eq!(d.len(), (ziel.0 * ziel.1 * 4) as usize);
        let werte: Vec<f32> = d.chunks_exact(4).map(|p| half::f16::from_bits(p[1]).to_f32()).collect();
        let mittel = werte.iter().sum::<f32>() / werte.len() as f32;
        // Gesamtmittel bleibt (Rand: 101 Spalten = 51 dunkle, 50 helle).
        let soll = 50.0 * 60000.0 / 101.0 / 65535.0;
        assert!((mittel - soll).abs() < 0.01, "Mittel {mittel} statt {soll}");
        // Jede Zelle deckt ~2,7 Spalten ab: nirgends reines Schwarz/Weiss.
        assert!(werte.iter().all(|v| *v > 0.2 && *v < 0.75), "nicht flaechentreu gemittelt");
        // Teilausschnitt mit Versatz: dieselbe Stelle, dieselbe Groesse.
        let t = flaechentreu_verkleinern(&raw, 10, 5, 50, 40, 25, 20);
        assert_eq!(t.len(), 25 * 20 * 4);
    }
}

/// Automatischer Schwarzpunkt — was die Automatik der Referenz-Entwicklung den
/// Bildern gibt und unseren bisher fehlte: die Grundhelligkeit hebt dunkle
/// Szenen an, aber nichts zog das Schwarz nach. Ergebnis war im Vergleich mit
/// der Referenz-Entwicklung ein flaches, verwaschenes Bild (Anzuege grau statt
/// schwarz).
///
/// Gemessen wird das 2. Perzentil der Luminanz im Arbeitsraum; nach dem
/// Belichtungsgewinn wird es ueber den Grauschleier-Regler (`blackpoint`,
/// zieht B·0,0005 linear ab und normiert neu) auf ~0,002 verankert.
/// Hochton-Szenen ohne echte Tiefen (Nebel, High-Key) bleiben unangetastet.
pub fn auto_blackpoint(img: &rawcore::RawImage, ev: f32) -> f32 {
    let mut lum = arbeitsraum_luma(img);
    if lum.len() < 64 {
        return 0.0;
    }
    let perzentil = |lum: &mut Vec<f32>, q: f32| -> f32 {
        let idx = ((lum.len() as f32 * q) as usize).min(lum.len() - 1);
        let (_, w, _) = lum
            .select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        *w
    };
    // Nur die WIRKLICH dunkelsten 0,5 % zaehlen als Schwarzpunkt-Kandidat.
    // Das 2. Perzentil lag bei einem Gruppenportraet mitten im dunklen
    // Anzug — der wurde mit verankert und das Bild soff ab.
    let p05 = perzentil(&mut lum, 0.005);
    let p10 = perzentil(&mut lum, 0.10);
    let verst = ev.exp2();
    let gained05 = p05 * verst;
    let gained10 = p10 * verst;
    if gained05 >= 0.14 {
        return 0.0; // keine echten Tiefen im Bild — nichts verankern
    }
    // Ziel 0,009: die Grundgradation senkt den Fuss danach noch einmal.
    // Erst gegen die Kamera-JPEGs kalibriert (0,011, Abschnitt 16), dann gegen
    // fertige Referenz-Exporte nachgezogen: die Referenz-Entwicklung verankert
    // ueberall TIEFER als die Kamera (p1 durchweg 3-13) — der Grauschleier kam daher,
    // dass unser Anker zu zaghaft war (p1 bis 22 bei hellen Innenaufnahmen).
    let mut abzug = (gained05 - 0.009).max(0.0);
    // Schattenmasse-Schutz: nie mehr abziehen als ein Fuenftel der Helligkeit,
    // in der die dunkelsten 10 % des Bildes leben. Ein dunkler Anzug, eine
    // Jacke im Schatten sind KEIN Schwarzpunkt — die Kamera laesst sie
    // dunkelgrau, wir jetzt auch. (0,12 → 0,20 mit den Referenz-Exporten: auch
    // beim Anzug-Foto liegt deren p1 unter unserem.)
    abzug = abzug.min(0.20 * gained10);
    (abzug / 0.0005).clamp(0.0, 32.0).round()
}

/// Luminanz-Stichprobe im Arbeitsraum — die gemeinsame Messbasis von
/// Grundhelligkeit und Schwarzpunkt (erst Matrix, dann messen; siehe
/// `auto_exposure` fuer das Warum).
fn arbeitsraum_luma(img: &rawcore::RawImage) -> Vec<f32> {
    let m = color::camera_to_working(&img.meta.cam_to_xyz, &color::REC2020_TO_XYZ);
    let step = ((img.pixel_count() / 200_000).max(1) as f32).sqrt().ceil() as usize;
    let mut lum: Vec<f32> = Vec::with_capacity(200_000);
    let w = img.width as usize;
    let h = img.height as usize;
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = (y * w + x) * 3;
            let cam = [
                img.pixels[i] as f32 / 65535.0,
                img.pixels[i + 1] as f32 / 65535.0,
                img.pixels[i + 2] as f32 / 65535.0,
            ];
            let g = m[1][0] * cam[0] + m[1][1] * cam[1] + m[1][2] * cam[2];
            let r = m[0][0] * cam[0] + m[0][1] * cam[1] + m[0][2] * cam[2];
            let b = m[2][0] * cam[0] + m[2][1] * cam[1] + m[2][2] * cam[2];
            lum.push((0.2627 * r + 0.6780 * g + 0.0593 * b).max(0.0));
            x += step;
        }
        y += step;
    }
    lum
}

/// Belichtungsvorschlag in Blendenstufen.
///
/// Ein lineares RAW ist von Natur aus dunkel — der Sensor misst Licht, das Auge
/// sieht logarithmisch. Jeder Entwickler legt deshalb eine Grundhelligkeit an;
/// ohne sie sieht jedes Foto beim Oeffnen zu duester aus.
pub fn auto_exposure(img: &rawcore::RawImage) -> f32 {
    // **Erst umrechnen, dann messen.**
    //
    // Der Sensor sieht anders als das Auge: bei RAF-Kameras ist der Gruenkanal deutlich
    // empfindlicher. Misst man die Helligkeit auf den rohen Sensorwerten, faellt sie
    // bei gruenen Motiven (Wiese, Laub) viel zu hoch aus — die Belichtung wird dann
    // zu knapp angesetzt, und genau dieselbe Matrix, die spaeter das Gruen wieder
    // einfaengt, macht das Bild dunkel. Gemessen wird deshalb dort, wo auch das
    // fertige Bild lebt: im Arbeitsraum.
    let m = color::camera_to_working(&img.meta.cam_to_xyz, &color::REC2020_TO_XYZ);
    let step = ((img.pixel_count() / 200_000).max(1) as f32).sqrt().ceil() as usize;
    let mut lum: Vec<f32> = Vec::with_capacity(200_000);
    let w = img.width as usize;
    let h = img.height as usize;
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = (y * w + x) * 3;
            let cam = [
                img.pixels[i] as f32 / 65535.0,
                img.pixels[i + 1] as f32 / 65535.0,
                img.pixels[i + 2] as f32 / 65535.0,
            ];
            let g = m[1][0] * cam[0] + m[1][1] * cam[1] + m[1][2] * cam[2];
            let r = m[0][0] * cam[0] + m[0][1] * cam[1] + m[0][2] * cam[2];
            let b = m[2][0] * cam[0] + m[2][1] * cam[1] + m[2][2] * cam[2];
            lum.push((0.2627 * r + 0.6780 * g + 0.0593 * b).max(0.0));
            x += step;
        }
        y += step;
    }
    if lum.is_empty() {
        return 0.0;
    }
    lum.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((lum.len() as f32 * 0.60) as usize).min(lum.len() - 1);
    let p = lum[idx];
    if p <= 1e-5 {
        return 0.0;
    }
    // Zielhelligkeit: erst 0,20 gegen die Kamera-JPEGs kalibriert, dann auf
    // 0,23 gegen fertige REFERENZ-Exporte nachgezogen — die Referenz-Entwicklung
    // legt die Mitten durchweg heller an als die Kamera, und daran ist das Auge
    // gewoehnt.
    let ziel = (0.23f32 / p).log2();
    // Lichter-Wache: die Anhebung darf die hellsten Bildteile nicht ueber den
    // Sensorweiss-Bereich schieben. Ohne sie klippten bei Aussenaufnahmen mit
    // Himmel 17-18 % der Pixel (Referenz-Entwicklung: ~1 %) — Weiss wirkte viel zu
    // stark aufgedreht. Gemessen am 99,5. Perzentil: was danach noch drueber
    // liegt, sind echte Spitzlichter, um die sich die Lichter-Schulter kuemmert.
    // Die Wache bremst nur EXTREME Anhebungen: bis 1,8 im Linearen faengt die
    // Lichter-Schulter (Dach 2,0) die Spitzen weich ab, und die automatische
    // Lichter-Ruecknahme (auto_highlights) holt grosse helle Flaechen zurueck.
    // Eine engere Wache (1,05) drueckte in Runde 1 der Referenz-Kalibrierung die
    // MITTEN um -26 — das Gegenteil des Ziels.
    // 2,4 statt 1,8: seit die Lichter-Ruecknahme mit RICHTIGEM Vorzeichen
    // arbeitet (tone_shape-Portierungsfehler), darf die Wache lockerer sein —
    // sie soll nur den Extremfall bremsen, nicht die Mitten deckeln.
    let idx_hell = ((lum.len() as f32 * 0.995) as usize).min(lum.len() - 1);
    let p995 = lum[idx_hell].max(1e-5);
    let wache = (2.1f32 / p995).log2();
    ziel.min(wache).clamp(-1.5, 3.0)
}

/// Automatische Lichter-Ruecknahme fuer die Startwerte.
///
/// Die Referenz-Entwicklung macht beides zugleich: Mitten anheben UND Lichter
/// zurueckziehen — mit reiner Belichtung geht das nicht (Gruppenfotos im Freien
/// brauchten +19 in den Mitten und -20 in den Lichtern GLEICHZEITIG). Deshalb bekommt
/// der Lichter-Regler einen gemessenen Startwert: je naeher die hellsten
/// Bildteile nach der Grundhelligkeit am Weiss liegen, desto staerker die
/// Ruecknahme. Bilder ohne helle Partien bleiben unangetastet.
pub fn auto_highlights(img: &rawcore::RawImage, ev: f32) -> f32 {
    let mut lum = arbeitsraum_luma(img);
    if lum.len() < 64 {
        return 0.0;
    }
    let idx = ((lum.len() as f32 * 0.98) as usize).min(lum.len() - 1);
    let (_, w, _) = lum
        .select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let gained = *w * ev.exp2();
    if gained <= 0.60 {
        return 0.0;
    }
    // Tief ziehen ist erlaubt, WEIL auto_shadows gleichzeitig die unteren
    // Mitten haelt — nur so entsteht die Kompression der Referenz-Entwicklung
    // (Himmel gehalten, Personen im Schatten trotzdem hell).
    (-(gained - 0.60) / 0.40 * 58.0).clamp(-70.0, 0.0).round()
}

/// Automatische Schatten-Anhebung fuer kontrastreiche Szenen.
///
/// Aussenaufnahmen mit hellem Himmel und Personen im Schatten brauchen beides
/// zugleich — Lichter zurueck UND Schatten auf: die fertigen Referenz-Exporte zeigen dort
/// p5 von 10-14, wo unsere rein globale Belichtung nur 4-6 liess. Angehoben
/// wird nur, wenn das Bild WIRKLICH beides hat (helle Partien UND gedrueckte
/// Schatten) — flache oder bewusst dunkle Szenen bleiben unangetastet.
pub fn auto_shadows(img: &rawcore::RawImage, ev: f32) -> f32 {
    let mut lum = arbeitsraum_luma(img);
    if lum.len() < 64 {
        return 0.0;
    }
    let perzentil = |lum: &mut Vec<f32>, q: f32| -> f32 {
        let idx = ((lum.len() as f32 * q) as usize).min(lum.len() - 1);
        let (_, w, _) = lum.select_nth_unstable_by(idx, |a, b| {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        });
        *w
    };
    let verst = ev.exp2();
    let g15 = perzentil(&mut lum, 0.15) * verst;
    let g95 = perzentil(&mut lum, 0.95) * verst;
    // Tor auf p95: der helle Himmel ist oft nur das oberste Zehntel des
    // Bildes — ein p85-Tor (Gras, Wiese) liess die Automatik nie feuern.
    if g95 <= 0.50 || g15 >= 0.09 {
        return 0.0;
    }
    (((0.09 - g15) / 0.09) * 38.0).clamp(0.0, 38.0).round()
}

/// Drehen in 90er-Schritten und Spiegeln — **verlustfrei**, auf der fertigen
/// Pixelmatrix.
///
/// Bewusst hier und nicht auf der Grafikkarte: eine Vierteldrehung ist eine reine
/// Umsortierung. Ueber eine Textur gerechnet, wuerde sie jedes Pixel neu
/// interpolieren und damit Schaerfe kosten — fuer nichts. `n_kanaele` ist 4 fuer
/// RGBA-Bytes und 3 fuer 16-Bit-RGB, sonst ist der Ablauf identisch.
pub fn ausrichten<T: Copy + Default>(
    src: &[T],
    w: u32,
    h: u32,
    n_kanaele: usize,
    rotate: i32,
    flip_h: bool,
    flip_v: bool,
) -> (Vec<T>, u32, u32) {
    let drehung = rotate.rem_euclid(360);
    if drehung == 0 && !flip_h && !flip_v {
        return (src.to_vec(), w, h);
    }
    let quer = drehung == 90 || drehung == 270;
    let (dw, dh) = if quer { (h, w) } else { (w, h) };
    let mut out = vec![T::default(); (dw as usize) * (dh as usize) * n_kanaele];
    for y in 0..h as usize {
        for x in 0..w as usize {
            // Erst drehen …
            let (mut tx, mut ty) = match drehung {
                90 => (h as usize - 1 - y, x),
                180 => (w as usize - 1 - x, h as usize - 1 - y),
                270 => (y, w as usize - 1 - x),
                _ => (x, y),
            };
            // … dann spiegeln, im Zielbild. Diese Reihenfolge entspricht dem, was
            // man auf dem Bildschirm erwartet: die Spiegelachse ist die, die man
            // gerade sieht, nicht die des Originals.
            if flip_h {
                tx = dw as usize - 1 - tx;
            }
            if flip_v {
                ty = dh as usize - 1 - ty;
            }
            let s = (y * w as usize + x) * n_kanaele;
            let d = (ty * dw as usize + tx) * n_kanaele;
            out[d..d + n_kanaele].copy_from_slice(&src[s..s + n_kanaele]);
        }
    }
    (out, dw, dh)
}

fn extent(w: u32, h: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    }
}

fn uniform_buffer(device: &wgpu::Device, bytes: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("regler"),
        contents: bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

/// Geometrie-Werte fuer den zweiten Durchgang.
///
/// `rotate` in 90er-Schritten und die Spiegelungen stehen bewusst **nicht** hier:
/// die sind reine Umsortierung von Pixeln und werden nach dem Rueckholen exakt
/// umgestellt. Ueber die Grafikkarte gedreht, muesste jedes Bild neu abgetastet
/// werden — Schaerfeverlust fuer eine Operation, die verlustfrei sein kann.
/// Filterbreite der Rauschminderung hinter der Entzerrung.
///
/// Die Rauschminderung ist an RAF-Fotos eingemessen worden, und die liefen
/// immer durch die Entzerrung — damals bilinear. Bilineares Nachrechnen glaettet
/// das Rauschen schon mit; seit Lanczos-3 kommt es ungeglaettet an. Damit die
/// Rauschminderung so gruendlich bleibt wie eingemessen, waechst ihre
/// Filterbreite hinter der Entzerrung. Gemessen an DSCF1632 (ISO 4000,
/// Rauschminderung 40/45, 60 flache Kacheln, Hochpass-Streuung in 8-Bit-Stufen):
/// bilinear 4,58 · Lanczos ohne Ausgleich 7,78 · Faktor 1,33 → 5,76 ·
/// **1,6 → 4,39** (Farbgesprenkel 2,70 → 2,47), und die Zeichnung in den
/// strukturierten Kacheln bleibt dabei ueber dem alten Stand (0,94/1,10/1,16).
/// Ohne Entzerrung gilt die alte Eichung unveraendert.
const NLM_NACH_ENTZERRUNG: f32 = 1.6;

fn geo_uniform(edit: &Edit) -> [f32; 4] {
    let winkel = edit.straighten.to_radians();
    let k = edit.lens_distort / 100.0 * 0.35;
    let aktiv = if winkel.abs() > 1e-6 || k.abs() > 1e-5 {
        1.0
    } else {
        0.0
    };
    // Massstab am Bildrand: ohne ihn schiebt die Entzerrung schwarze Raender ins
    // Bild (k > 0) oder verschenkt Bildflaeche (k < 0).
    [winkel, k, 1.0, aktiv]
}

/// Alles, was vor den Reglern in die Rohdaten eingreift: kopierte Stellen
/// (`heal`), vom Retusche-Modell gefuellte (`heal_ki`) und eingesetzte Stuecke
/// (`zusatz`). Koordinaten jeweils als Anteile des Bildes, in das sie gehoeren.
#[derive(Debug, Clone, Copy)]
pub struct Retusche<'a> {
    pub heal: &'a [[f32; 3]],
    pub heal_ki: &'a [[f32; 3]],
    pub zusaetze: &'a [zusatz::Stueck],
}

impl Retusche<'_> {
    pub const LEER: Retusche<'static> = Retusche { heal: &[], heal_ki: &[], zusaetze: &[] };

    /// Erst kopierte Stellen, dann KI-Fuellung, dann eingesetzte Stuecke. Fehlt
    /// das Retusche-Modell oder rechnet es nicht, werden die KI-Stellen klassisch
    /// geschlossen — eine markierte Stelle bleibt nie einfach stehen.
    pub fn anwenden(&self, data: &mut [u16], w: u32, h: u32) {
        if !self.heal.is_empty() {
            heal::anwenden(data, w, h, &heal::aus_edit(self.heal, w, h));
        }
        if !self.heal_ki.is_empty() {
            let flecken = heal::aus_edit(self.heal_ki, w, h);
            if let Err(e) = heal_ki::anwenden(data, w, h, &flecken) {
                eprintln!("KI-Fuellung nicht moeglich ({e}) — klassisch geschlossen");
                heal::anwenden(data, w, h, &flecken);
            }
        }
        for z in self.zusaetze {
            zusatz::einsetzen(data, w, h, z);
        }
    }
}

/// Wandelt die 16-Bit-Ganzzahlen aus `rawcore` in halbe Gleitkommazahlen und
/// verkleinert dabei optional. Die Verkleinerung mittelt echte Pixelbloecke
/// (Kastenfilter) statt Pixel wegzulassen — sonst flimmern feine Strukturen und
/// Rauschen wird schlimmer statt besser.
/// Bereich `x, y, w, h` flaechentreu auf `ob` × `oh` verkleinern (RGBA f16,
/// Werte 0..1). Jedes Zielpixel mittelt die Quellflaeche, die es abdeckt —
/// angeschnittene Randpixel mit ihrem Anteil. Anders als die ganzzahlige
/// Teilung in `to_rgba_f16` trifft das jede Zielgroesse genau.
fn flaechentreu_verkleinern(img: &rawcore::RawImage, x: u32, y: u32, w: u32, h: u32, ob: u32, oh: u32) -> Vec<u16> {
    // Je Zielspalte bzw. -zeile: (erstes Quellpixel, Gewichte) — einmal
    // ausgerechnet, fuer alle Zeilen gleich.
    let anteile = |quelle: u32, ziel: u32, versatz: u32| -> Vec<(usize, Vec<f32>)> {
        let f = quelle as f64 / ziel as f64;
        (0..ziel)
            .map(|o| {
                let a = o as f64 * f;
                let b = ((o + 1) as f64 * f).min(quelle as f64);
                let erstes = a.floor() as u32;
                let letztes = (b.ceil() as u32).min(quelle).max(erstes + 1);
                let gew: Vec<f32> = (erstes..letztes)
                    .map(|q| ((q as f64 + 1.0).min(b) - (q as f64).max(a)).max(0.0) as f32)
                    .collect();
                let summe: f32 = gew.iter().sum::<f32>().max(1e-9);
                ((versatz + erstes) as usize, gew.into_iter().map(|g| g / summe).collect())
            })
            .collect()
    };
    let spalten = anteile(w, ob, x);
    let zeilen = anteile(h, oh, y);
    let breite = img.width as usize;
    let mut out = vec![0u16; (ob as usize) * (oh as usize) * 4];
    let skala = 1.0f32 / 65535.0;
    let eins = half::f16::from_f32(1.0).to_bits();
    let kerne = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(oh as usize).max(1);
    let zeilen_je = (oh as usize).div_ceil(kerne);
    std::thread::scope(|s| {
        for (b, band) in out.chunks_mut(zeilen_je * ob as usize * 4).enumerate() {
            let (spalten, zeilen) = (&spalten, &zeilen);
            s.spawn(move || {
                let mut zeile_acc = vec![[0f32; 3]; ob as usize];
                for (dy, ziel) in band.chunks_exact_mut(ob as usize * 4).enumerate() {
                    let (y0, gy) = &zeilen[b * zeilen_je + dy];
                    zeile_acc.iter_mut().for_each(|a| *a = [0.0; 3]);
                    for (j, wy) in gy.iter().enumerate() {
                        let reihe = (y0 + j) * breite;
                        for (o, (x0, gx)) in spalten.iter().enumerate() {
                            let mut acc = [0f32; 3];
                            for (i, wx) in gx.iter().enumerate() {
                                let p = (reihe + x0 + i) * 3;
                                acc[0] += img.pixels[p] as f32 * wx;
                                acc[1] += img.pixels[p + 1] as f32 * wx;
                                acc[2] += img.pixels[p + 2] as f32 * wx;
                            }
                            let z = &mut zeile_acc[o];
                            z[0] += acc[0] * wy;
                            z[1] += acc[1] * wy;
                            z[2] += acc[2] * wy;
                        }
                    }
                    for (o, a) in zeile_acc.iter().enumerate() {
                        for c in 0..3 {
                            ziel[o * 4 + c] = half::f16::from_f32(a[c] * skala).to_bits();
                        }
                        ziel[o * 4 + 3] = eins;
                    }
                }
            });
        }
    });
    out
}

fn to_rgba_f16(img: &rawcore::RawImage, max_dim: Option<u32>) -> (u32, u32, Vec<u16>) {
    let (sw, sh) = (img.width, img.height);
    let step = match max_dim {
        Some(m) if sw.max(sh) > m => (sw.max(sh) as f32 / m as f32).ceil() as u32,
        _ => 1,
    };
    let w = (sw / step).max(1);
    let h = (sh / step).max(1);
    let mut out = vec![0u16; (w as usize) * (h as usize) * 4];
    let scale = 1.0f32 / 65535.0;
    let one = half::f16::from_f32(1.0).to_bits();

    // Ueber Zeilenbaender auf alle Kerne verteilt: ein 40-MP-Bild einkernig zu
    // mitteln kostet sonst um die halbe Sekunde — und diese Funktion liegt auf
    // dem Weg jedes Bildwechsels.
    let kerne = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(h as usize)
        .max(1);
    let zeilen_je = h.div_ceil(kerne as u32);
    let baender: Vec<&mut [u16]> = out.chunks_mut((zeilen_je * w * 4) as usize).collect();
    std::thread::scope(|s| {
        for (b, band) in baender.into_iter().enumerate() {
            let y_start = b as u32 * zeilen_je;
            s.spawn(move || {
                for (dy, zeile) in band.chunks_exact_mut((w * 4) as usize).enumerate() {
                    let y = y_start + dy as u32;
                    for x in 0..w {
                        let mut acc = [0f32; 3];
                        let mut n = 0f32;
                        for sy in (y * step)..((y + 1) * step).min(sh) {
                            for sx in (x * step)..((x + 1) * step).min(sw) {
                                let i =
                                    ((sy as usize) * (sw as usize) + sx as usize) * 3;
                                acc[0] += img.pixels[i] as f32;
                                acc[1] += img.pixels[i + 1] as f32;
                                acc[2] += img.pixels[i + 2] as f32;
                                n += 1.0;
                            }
                        }
                        let o = (x * 4) as usize;
                        for c in 0..3 {
                            zeile[o + c] =
                                half::f16::from_f32(acc[c] / n.max(1.0) * scale).to_bits();
                        }
                        zeile[o + 3] = one;
                    }
                }
            });
        }
    });
    (w, h, out)
}

#[cfg(test)]
mod belichtung_tests {
    use super::*;

    /// Baut ein einfarbiges Testbild im Kamerafarbraum.
    fn grau(wert: f32, cam_to_xyz: color::Mat3) -> rawcore::RawImage {
        let v = (wert * 65535.0) as u16;
        rawcore::RawImage {
            width: 64,
            height: 64,
            pixels: vec![v; 64 * 64 * 3],
            meta: rawcore::RawMeta {
                make: "Test".into(),
                model: "Grau".into(),
                muster_6x6: false,
                cam_to_xyz,
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

    /// Eine Flaeche, die schon auf der Zielhelligkeit liegt, darf nicht mehr
    /// angehoben werden. (Ziel 0,23 — gegen fertige Referenz-Exporte
    /// kalibriert.)
    #[test]
    fn richtige_helligkeit_bleibt_stehen() {
        let ev = auto_exposure(&grau(0.23, color::IDENTITY));
        assert!(ev.abs() < 0.05, "sollte 0 sein, ist {ev}");
    }

    /// Halb so hell = genau eine Blende mehr.
    #[test]
    fn halbe_helligkeit_gibt_eine_blende() {
        let ev = auto_exposure(&grau(0.115, color::IDENTITY));
        assert!((ev - 1.0).abs() < 0.05, "sollte 1 sein, ist {ev}");
    }

    /// Der eigentliche Punkt: gemessen wird **nach** der Farbmatrix.
    ///
    /// Mit einer echten Kameramatrix ist der Gruenkanal des Sensors viel
    /// empfindlicher. Wuerde man auf den rohen Sensorwerten messen, faende man ein
    /// neutrales Grau zu hell und setzte die Belichtung zu knapp an — das Bild
    /// kaeme dunkel heraus. Neutral bleibt neutral, also muss auch hier 0 stehen.
    #[test]
    fn messung_beruecksichtigt_die_kameramatrix() {
        // Echte Matrix der Testkamera (40 MP, XYZ -> Kamera).
        let cam_xyz: color::Mat3 = [
            [1.1809, -0.5358, -0.1141],
            [-0.4248, 1.2164, 0.2343],
            [-0.0514, 0.1097, 0.5848],
        ];
        let ev = auto_exposure(&grau(0.23, cam_xyz));
        assert!(ev.abs() < 0.05, "neutrales Grau muss 0 ergeben, ist {ev}");
    }
}
