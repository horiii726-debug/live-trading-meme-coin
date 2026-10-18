//! Port Rust dari `rhc_vps/bot/filter_feats.js` (34 fitur filter beli menit-1 + g_eig_max_nonpool).
//! Hasil harus identik dengan JS/V8 (Node 24): lihat `acuan/` untuk uji kesamaan.

pub mod dataset;
pub mod fdlibm;
pub mod features;
pub mod jsnum;
pub mod orderflow;

pub use features::{features, Features, Meta, Swap, Transfer, KEYS, NATIVE, PM};
