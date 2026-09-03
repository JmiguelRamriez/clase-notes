//! Integración con la bóveda de Obsidian: escritura de notas y
//! copia del WAV a la carpeta `attachments/` si se desea.

pub mod vault;

pub use vault::ObsidianVault;
