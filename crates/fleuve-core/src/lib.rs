//! Fleuve — moteur de téléchargement inspiré du fonctionnement observable
//! d'Internet Download Manager : segmentation dynamique, reprise, HLS,
//! file d'attente, planificateur et limite de débit.

pub mod engine;
pub mod hls;
pub mod limiter;
pub mod manager;
pub mod model;
pub mod server;
pub mod site;
pub mod util;

pub use manager::Manager;
pub use model::*;
