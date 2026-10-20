/*
 * Schmaler C-Anschluss an LibRaw.
 *
 * Warum ueberhaupt C? Die Werte, die wir brauchen (Farbmatrix der Kamera, Weissabgleich
 * ab Werk, Schwarz-/Weisspunkt, Sensormuster), stecken tief in `libraw_data_t`. Diese
 * Struktur in Rust nachzubauen waere ein Blindflug: schon ein anderes LibRaw-Release
 * verschiebt die Feld-Offsets und wir laesen still Muell. Der C-Compiler kennt das
 * Layout dagegen immer richtig — deshalb holen wir die Felder hier ab und reichen sie
 * als flache, stabile Struktur nach Rust weiter.
 *
 * Alles andere (oeffnen, entpacken, demosaicen) macht Rust ueber LibRaws offizielle
 * C-Schnittstelle; hier steht bewusst so wenig C wie moeglich.
 */
#include <string.h>
#include <libraw/libraw.h>

typedef struct {
    int   raw_width, raw_height;
    float cam_xyz[12];   /* [4][3] zeilenweise: Kamera-RGB -> CIE XYZ (D65-verankert) */
    float rgb_cam[12];   /* [3][4] zeilenweise: von LibRaw gesetzt nach dcraw_process  */
    float cam_mul[4];    /* Weissabgleich, wie die Kamera ihn aufgenommen hat          */
    float pre_mul[4];    /* Tageslicht-Referenz der Kamera                             */
    unsigned black;
    unsigned cblack[4];  /* Schwarzpunkt je Kanal (kommt bei RAF-Kameras vor)          */
    unsigned maximum;
    unsigned filters;    /* 9 = 6x6-Farbmuster, sonst Bayer-Muster                     */
    int   muster_6x6;
    int   flip;
    char  make[64];
    char  model[64];
    char  normalized_model[64];
    float iso, shutter, aperture, focal_len;
    int   colors;
    /* Fuer die EXIF-Uebernahme in Exportdateien: */
    long long timestamp;      /* Aufnahmezeitpunkt, Unix-Sekunden                  */
    char  lens[128];          /* Objektivbezeichnung, wie die Kamera sie notiert   */
    char  artist[64];         /* Fotograf, falls in der Kamera hinterlegt          */
    /* Belichtungs-Versatz der Kamera (RAF-Kameras: DR200/DR400 belichten das
       RAW knapper; die Referenz-Entwicklung gleicht das aus). */
    float expo_versatz;          /* ExpoMidPointShift (RAF 0x9650), EV  */
    unsigned short dynamikbereich[6];      /* DynamicRange, -Setting, Development-, Auto-, DRangePriority, frei */
    float dng_baseline_exposure;    /* BaselineExposure einer DNG, EV       */
    float focal_35mm;               /* Brennweite in Kleinbild-mm (EXIF), 0 = unbekannt */
    float focus_distance;           /* Fokusabstand in m (0 = unbekannt)     */
} fr_meta;

static void copy_str(char *dst, const char *src, size_t n)
{
    if (!src) { dst[0] = 0; return; }
    strncpy(dst, src, n - 1);
    dst[n - 1] = 0;
}

void fr_fill_meta(libraw_data_t *d, fr_meta *m)
{
    int i, j;
    memset(m, 0, sizeof(*m));
    if (!d || !m) return;

    m->raw_width  = d->sizes.raw_width;
    m->raw_height = d->sizes.raw_height;
    m->flip       = d->sizes.flip;

    for (i = 0; i < 4; i++)
        for (j = 0; j < 3; j++)
            m->cam_xyz[i * 3 + j] = d->color.cam_xyz[i][j];
    for (i = 0; i < 3; i++)
        for (j = 0; j < 4; j++)
            m->rgb_cam[i * 4 + j] = d->color.rgb_cam[i][j];

    for (i = 0; i < 4; i++) {
        m->cam_mul[i] = d->color.cam_mul[i];
        m->pre_mul[i] = d->color.pre_mul[i];
        m->cblack[i]  = d->color.cblack[i];
    }
    m->black   = d->color.black;
    m->maximum = d->color.maximum;
    m->filters = d->idata.filters;
    /* LibRaw kennzeichnet das 6x6-Farbmuster mit filters==9 (statt 2x2-Bayer). */
    m->muster_6x6 = (d->idata.filters == 9) ? 1 : 0;
    m->colors    = d->idata.colors;

    copy_str(m->make,  d->idata.make,  sizeof(m->make));
    copy_str(m->model, d->idata.model, sizeof(m->model));
    copy_str(m->normalized_model, d->idata.normalized_model, sizeof(m->normalized_model));

    m->iso       = d->other.iso_speed;
    m->shutter   = d->other.shutter;
    m->aperture  = d->other.aperture;
    m->focal_len = d->other.focal_len;

    m->timestamp = (long long) d->other.timestamp;
    m->expo_versatz = d->makernotes.fuji.ExpoMidPointShift; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[0] = d->makernotes.fuji.DynamicRange; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[1] = d->makernotes.fuji.DynamicRangeSetting; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[2] = d->makernotes.fuji.DevelopmentDynamicRange; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[3] = d->makernotes.fuji.AutoDynamicRange; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[4] = d->makernotes.fuji.DRangePriority; /* Kennung: LibRaw-Feldname */
    m->dynamikbereich[5] = 0;
    m->dng_baseline_exposure = d->color.dng_levels.baseline_exposure;
    m->focal_35mm = (float) d->lens.FocalLengthIn35mmFormat;
    m->focus_distance = 0.0f; /* LibRaw fuehrt keinen allgemeinen Fokusabstand */
    copy_str(m->lens,   d->lens.Lens,     sizeof(m->lens));
    copy_str(m->artist, d->other.artist,  sizeof(m->artist));
}

size_t fr_meta_size(void) { return sizeof(fr_meta); }

/* Fuer `half_size` gibt es in LibRaws C-Schnittstelle keine Setz-Funktion —
   hier ist der Feldzugriff ohnehin schon erlaubt, also machen wir es gleich mit. */
void fr_set_half_size(libraw_data_t *d, int value)
{
    if (d) d->params.half_size = value;
}

/* Median-Paesse ueber die Farbdifferenz-Ebenen (R-G, B-G) nach dem Demosaic —
   das klassische "-m" der Kommandozeilen-Werkzeuge (med_passes). Fuer das
   6x6-Farbmuster der RAF-Kameras (statt 2x2-Bayer) ist das der Standard-
   Begleiter zu Markesteijn: es tilgt einzelne falschfarbige Pixel an Kanten
   ("Uebersprechen"), ohne Helligkeit/Schaerfe anzutasten. LibRaws
   C-Schnittstelle hat dafuer keinen Setter. */
void fr_set_med_passes(libraw_data_t *d, int value)
{
    if (d) d->params.med_passes = value;
}

/* Drehung gehoert UNS, nicht LibRaw: dcraw_make_mem_image wendet die
   Kameralage sonst selbst an (user_flip = -1 heisst "aus der Datei") UND
   sizes.flip meldet den Wert trotzdem weiter — Miniatur und Editor drehten
   dann ein zweites Mal, Hochformat lag wieder quer. Eine Zustaendigkeit:
   LibRaw dreht nie, unsere Drehlogik (drehung_aus_kamera + ausrichten)
   uebernimmt in Miniatur, Vorschau und Export. */
void fr_set_user_flip(libraw_data_t *d, int value)
{
    if (d) d->params.user_flip = value;
}

/* Die Kameralage MUSS vor dem Entwickeln gelesen werden: mit user_flip = 0
   ueberschreibt LibRaw sizes.flip beim Verarbeiten — danach steht dort 0,
   und niemand wuesste mehr, wie herum die Aufnahme gehoert. */
int fr_get_flip(libraw_data_t *d)
{
    return d ? d->sizes.flip : 0;
}

/* Anzahl der Farbkanaele (3 = RGB, auch bei Bayer mit zwei Gruen; 4 = CMYG
   und Co.). Danach richtet sich, ob der vierte Weissabgleich-Faktor gilt. */
int fr_get_colors(libraw_data_t *d)
{
    return d ? d->idata.colors : 3;
}

/* Sensormuster; 0 = Daten ohne Mosaik (schon entwickelte RGB-Werte). Vor dem
   Entwickeln lesen — half_size veraendert es. */
unsigned fr_get_filters(libraw_data_t *d)
{
    return d ? d->idata.filters : 0;
}

/* Eingebettete Vorschau (bei RAF ein grosses JPEG) entpacken. Das ist um
   ein Vielfaches billiger als das RAW zu entwickeln — LibRaw kopiert nur den
   fertigen JPEG-Block aus der Datei, ohne Entpacken/Demosaic. Ideal fuer die
   Galerie, die sonst 200 Fotos einzeln voll dekodieren wuerde (100 % CPU,
   Vorschauen troepfeln).

   Ablauf in Rust: init -> open_file -> fr_thumb_unpack; ist das Format JPEG
   (tformat==1), Puffer der gemeldeten Laenge anlegen und mit fr_thumb_copy
   fuellen. `flip` kommt aus sizes.flip — dieselbe Kameralage wie beim RAW,
   damit die Miniatur genauso gedreht wird wie die grosse Ansicht. */
int fr_thumb_unpack(libraw_data_t *d, int *format, unsigned *length,
                    int *twidth, int *theight, int *flip)
{
    if (!d) return -1;
    int rc = libraw_unpack_thumb(d);
    if (rc != 0) return rc;
    if (format)  *format  = d->thumbnail.tformat;
    if (length)  *length  = d->thumbnail.tlength;
    if (twidth)  *twidth  = d->thumbnail.twidth;
    if (theight) *theight = d->thumbnail.theight;
    if (flip)    *flip    = d->sizes.flip;
    return 0;
}

void fr_thumb_copy(libraw_data_t *d, unsigned char *dst)
{
    if (d && dst && d->thumbnail.thumb && d->thumbnail.tlength)
        memcpy(dst, d->thumbnail.thumb, d->thumbnail.tlength);
}
