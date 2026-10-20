//! Rohe Deklarationen der LibRaw-C-Schnittstelle.
//!
//! Bewusst klein gehalten: nur was wir wirklich rufen. Alles, was Feldzugriffe in
//! `libraw_data_t` braucht, laeuft ueber `fr_fill_meta` aus `csrc/shim.c` — siehe
//! die Begruendung dort.

use std::ffi::{c_char, c_float, c_int, c_uint, c_void};

/// Gegenstueck zu `libraw_processed_image_t`. Diese Struktur ist seit Jahren stabil
/// und klein genug, um sie hier direkt zu spiegeln.
#[repr(C)]
pub struct LibrawProcessedImage {
    pub image_type: c_int,
    pub height: u16,
    pub width: u16,
    pub colors: u16,
    pub bits: u16,
    pub data_size: u32,
    /// Beginn des Pixelblocks; die echte Laenge steht in `data_size`.
    pub data: [u8; 1],
}

/// Gegenstueck zu `fr_meta` aus dem Shim — Reihenfolge und Typen muessen
/// Zeile fuer Zeile uebereinstimmen.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrMeta {
    pub raw_width: c_int,
    pub raw_height: c_int,
    pub cam_xyz: [f32; 12],
    pub rgb_cam: [f32; 12],
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub black: u32,
    pub cblack: [u32; 4],
    pub maximum: u32,
    pub filters: u32,
    pub muster_6x6: c_int,
    pub flip: c_int,
    pub make: [c_char; 64],
    pub model: [c_char; 64],
    pub normalized_model: [c_char; 64],
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub colors: c_int,
    pub timestamp: i64,
    pub lens: [c_char; 128],
    pub artist: [c_char; 64],
    pub expo_versatz: f32,
    pub dynamikbereich: [u16; 6],
    pub dng_baseline_exposure: f32,
    pub focal_35mm: f32,
    pub focus_distance: f32,
}

impl Default for FrMeta {
    fn default() -> Self {
        // Alles genullt — `fr_fill_meta` ueberschreibt ohnehin komplett.
        unsafe { std::mem::zeroed() }
    }
}

extern "C" {
    pub fn libraw_init(flags: u32) -> *mut c_void;
    pub fn libraw_close(lr: *mut c_void);
    pub fn libraw_open_file(lr: *mut c_void, path: *const c_char) -> c_int;
    pub fn libraw_unpack(lr: *mut c_void) -> c_int;
    pub fn libraw_dcraw_process(lr: *mut c_void) -> c_int;
    pub fn libraw_dcraw_make_mem_image(lr: *mut c_void, errcode: *mut c_int)
        -> *mut LibrawProcessedImage;
    pub fn libraw_dcraw_clear_mem(img: *mut LibrawProcessedImage);
    pub fn libraw_strerror(errorcode: c_int) -> *const c_char;
    /// LibRaw nennt die Funktion schlicht `libraw_version`.
    pub fn libraw_version() -> *const c_char;

    pub fn libraw_set_demosaic(lr: *mut c_void, value: c_int);
    // Bewusst behalten (gemessen und verworfen, siehe decode): wer die
    // Median-Paesse je wieder erwaegt, findet den Setter hier.
    #[allow(dead_code)]
    pub fn fr_set_med_passes(lr: *mut c_void, value: c_int);
    pub fn libraw_set_output_color(lr: *mut c_void, value: c_int);
    pub fn libraw_set_output_bps(lr: *mut c_void, value: c_int);
    pub fn libraw_set_gamma(lr: *mut c_void, index: c_int, value: c_float);
    pub fn libraw_set_no_auto_bright(lr: *mut c_void, value: c_int);
    pub fn libraw_set_highlight(lr: *mut c_void, value: c_int);
    pub fn libraw_set_user_mul(lr: *mut c_void, index: c_int, val: c_float);
    pub fn libraw_set_adjust_maximum_thr(lr: *mut c_void, value: c_float);
    pub fn libraw_get_cam_mul(lr: *mut c_void, index: c_int) -> c_float;
    pub fn libraw_get_pre_mul(lr: *mut c_void, index: c_int) -> c_float;

    // Aus unserem Shim:
    pub fn fr_fill_meta(lr: *mut c_void, out: *mut FrMeta);
    pub fn fr_meta_size() -> usize;
    pub fn fr_set_half_size(lr: *mut c_void, value: c_int);
    pub fn fr_set_user_flip(lr: *mut c_void, value: c_int);
    pub fn fr_get_flip(lr: *mut c_void) -> c_int;
    pub fn fr_get_colors(lr: *mut c_void) -> c_int;
    pub fn fr_get_filters(lr: *mut c_void) -> c_uint;
    pub fn fr_thumb_unpack(
        lr: *mut c_void,
        format: *mut c_int,
        length: *mut u32,
        twidth: *mut c_int,
        theight: *mut c_int,
        flip: *mut c_int,
    ) -> c_int;
    pub fn fr_thumb_copy(lr: *mut c_void, dst: *mut u8);
}

/// LibRaw hat fuer `half_size` keine offizielle Setz-Funktion in der C-Schnittstelle —
/// deshalb liegt sie mit im Shim.
pub unsafe fn libraw_set_half_size(lr: *mut c_void, value: c_int) {
    fr_set_half_size(lr, value)
}

pub unsafe fn libraw_set_user_flip(lr: *mut c_void, value: c_int) {
    fr_set_user_flip(lr, value)
}

pub unsafe fn libraw_get_flip(lr: *mut c_void) -> c_int {
    fr_get_flip(lr)
}

pub unsafe fn libraw_get_colors(lr: *mut c_void) -> c_int {
    fr_get_colors(lr)
}

pub unsafe fn libraw_get_filters(lr: *mut c_void) -> c_uint {
    fr_get_filters(lr)
}
