//! Operaciones sobre la bóveda de Obsidian:
//!
//! - Estructura de directorios: `vault/Clases/<Materia>/` con
//!   `attachments/` para los audios.
//! - Escritura de notas con colisiones manejadas.
//! - Generación automática de un **MOC** (Map of Content) por materia,
//!   que enlaza todas las notas existentes de esa materia en orden
//!   cronológico inverso, agrupadas por mes.
//!
//! Layout resultante:
//!
//! ```text
//! ~/Obsidian/
//! └── Clases/
//!     ├── Cálculo II/
//!     │   ├── _MOC.md                     ← índice de la materia
//!     │   ├── 2026-08-30-limites.md       ← notas
//!     │   ├── 2026-09-01-derivadas.md
//!     │   └── attachments/
//!     │       ├── 2026-08-30-limites.wav
//!     │       └── 2026-09-01-derivadas.wav
//!     └── Física/
//!         ├── _MOC.md
//!         └── ...
//! ```

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

use crate::config::ObsidianConfig;

pub struct ObsidianVault {
    root: PathBuf,
    notes_subdir: String,
}

impl ObsidianVault {
    pub fn new(cfg: &ObsidianConfig) -> Self {
        Self {
            root: cfg.vault_path.clone(),
            notes_subdir: cfg.notes_subdir.clone(),
        }
    }

    /// Ruta al directorio de notas (`vault/Clases/`).
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join(&self.notes_subdir)
    }

    /// Ruta al directorio de la materia (`vault/Clases/<Materia>/`).
    /// Crea el directorio si no existe.
    pub fn materia_dir(&self, materia: &str) -> Result<PathBuf> {
        let dir = self.notes_dir().join(slugify_materia(materia));
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creando directorio de materia {}", dir.display()))?;
        }
        Ok(dir)
    }

    /// Ruta al directorio de adjuntos de la materia.
    pub fn attachments_dir(&self, materia: &str) -> Result<PathBuf> {
        let dir = self.materia_dir(materia)?.join("attachments");
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(dir)
    }

    /// Verifica que la bóveda exista. Si no, intenta crearla.
    pub fn ensure(&self) -> Result<()> {
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root).with_context(|| {
                format!("creando bóveda en {}", self.root.display())
            })?;
        }
        if !self.notes_dir().exists() {
            std::fs::create_dir_all(&self.notes_dir())?;
        }
        Ok(())
    }

    /// Copia el audio a `attachments/<Materia>/` y devuelve el nombre
    /// final (sin la ruta) para usarlo como `[[wikilink]]` en la nota.
    pub fn copy_audio(&self, materia: &str, audio_path: &Path) -> Result<String> {
        self.ensure()?;
        let attach_dir = self.attachments_dir(materia)?;
        let file_name = audio_path
            .file_name()
            .context("el audio no tiene nombre de archivo")?
            .to_string_lossy()
            .to_string();
        let mut dest = attach_dir.join(&file_name);
        let mut n = 1;
        while dest.exists() {
            let stem = audio_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("audio");
            let ext = audio_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("wav");
            dest = attach_dir.join(format!("{}-{}.{}", stem, n, ext));
            n += 1;
        }
        std::fs::copy(audio_path, &dest).with_context(|| {
            format!(
                "copiando audio de {} a {}",
                audio_path.display(),
                dest.display()
            )
        })?;
        Ok(dest
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.wav")
            .to_string())
    }

    /// Escribe la nota Markdown en el directorio de la materia.
    /// Devuelve la ruta final.
    pub fn write_note(
        &self,
        materia: &str,
        tema: &str,
        date: NaiveDate,
        content: &str,
    ) -> Result<PathBuf> {
        self.ensure()?;
        let dir = self.materia_dir(materia)?;
        crate::notes::markdown::write_note(&dir, materia, tema, date, content)
    }

    /// Crea o actualiza el MOC (Map of Content) de la materia.
    /// Lista todas las notas `.md` (excepto el propio MOC) en orden
    /// cronológico inverso, agrupadas por mes. Devuelve la ruta del MOC.
    pub fn update_moc(&self, materia: &str) -> Result<PathBuf> {
        self.ensure()?;
        let dir = self.materia_dir(materia)?;
        let moc_path = dir.join("_MOC.md");

        // Recolectar notas: solo .md, excluyendo _MOC.md.
        let mut notes: Vec<MocEntry> = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("leyendo {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name == "_MOC.md" {
                continue;
            }
            // Extraer fecha del nombre (formato: YYYY-MM-DD-...)
            if let Some(date) = parse_date_from_filename(&name) {
                let title = read_title_from_note(&path).unwrap_or_else(|| {
                    name.trim_end_matches(".md").to_string()
                });
                notes.push(MocEntry { date, title, name });
            }
        }

        // Ordenar cronológicamente inverso.
        notes.sort_by(|a, b| b.date.cmp(&a.date));

        // Agrupar por mes (YYYY-MM).
        let mut by_month: std::collections::BTreeMap<String, Vec<&MocEntry>> =
            std::collections::BTreeMap::new();
        for n in &notes {
            let month = n.date.format("%Y-%m").to_string();
            by_month.entry(month).or_default().push(n);
        }

        // Renderizar MOC.
        let mut body = String::new();
        body.push_str(&format!("---\n"));
        body.push_str(&format!("title: \"MOC: {}\"\n", materia));
        body.push_str(&format!("subject: \"{}\"\n", materia));
        body.push_str("type: moc\n");
        body.push_str("tags: [moc, indice]\n");
        body.push_str(&format!("updated: {}\n", chrono::Local::now().date_naive()));
        body.push_str("---\n\n");
        body.push_str(&format!("# {} — Índice de clases\n\n", materia));
        body.push_str(&format!(
            "Mapa de contenido con todas las clases registradas de **{}**, \
             en orden cronológico inverso. Se actualiza automáticamente \
             cada vez que se añade una nueva clase.\n\n",
            materia
        ));
        body.push_str(&format!("**Total de clases:** {}\n\n", notes.len()));

        if notes.is_empty() {
            body.push_str("_Aún no hay clases registradas en esta materia._\n");
        } else {
            // Renderizar en orden cronológico (más recientes primero).
            let months: Vec<_> = by_month.keys().rev().collect();
            for month in months {
                let entries = by_month.get(month).unwrap();
                let month_label = format_month(month);
                body.push_str(&format!("## {}\n\n", month_label));
                for n in entries {
                    body.push_str(&format!(
                        "- [[{}|{}]] — {} ({})\n",
                        n.name.trim_end_matches(".md"),
                        n.title,
                        n.date.format("%Y-%m-%d"),
                        n.date.format("%A"),
                    ));
                }
                body.push('\n');
            }
        }

        body.push_str("---\n\n");
        body.push_str("*Generado automáticamente por `clase-notes`*\n");

        std::fs::write(&moc_path, body)
            .with_context(|| format!("escribiendo MOC en {}", moc_path.display()))?;
        Ok(moc_path)
    }
}

struct MocEntry {
    date: NaiveDate,
    title: String,
    name: String,
}

fn parse_date_from_filename(name: &str) -> Option<NaiveDate> {
    // Formato esperado: YYYY-MM-DD-... .md
    let stem = name.trim_end_matches(".md");
    let parts: Vec<&str> = stem.splitn(4, '-').collect();
    if parts.len() < 3 {
        return None;
    }
    let y = parts[0].parse().ok()?;
    let m = parts[1].parse().ok()?;
    let d = parts[2].parse().ok()?;
    NaiveDate::from_ymd_opt(y, m, d)
}

fn read_title_from_note(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    // Buscar la primera línea que empiece con "# " (no "## ").
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    // Si no, intentar el frontmatter title.
    for line in content.lines().take(20) {
        if let Some(rest) = line.trim().strip_prefix("title:") {
            let t = rest.trim().trim_matches('"');
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn format_month(yyyymm: &str) -> String {
    // Convertir "2026-09" a "Septiembre 2026"
    let parts: Vec<&str> = yyyymm.split('-').collect();
    if parts.len() != 2 {
        return yyyymm.to_string();
    }
    let year = parts[0];
    let month = match parts[1] {
        "01" => "Enero",
        "02" => "Febrero",
        "03" => "Marzo",
        "04" => "Abril",
        "05" => "Mayo",
        "06" => "Junio",
        "07" => "Julio",
        "08" => "Agosto",
        "09" => "Septiembre",
        "10" => "Octubre",
        "11" => "Noviembre",
        "12" => "Diciembre",
        _ => return yyyymm.to_string(),
    };
    format!("{} {}", month, year)
}

/// Slugify para nombres de directorio de materia: preserva espacios y
/// solo quita caracteres problemáticos para sistemas de archivos.
fn slugify_materia(s: &str) -> String {
    let normalized: String = s
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    normalized
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_ascii_alphanumeric() || c.is_whitespace() || c == '-' || c == '_' => c,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_creates_structure() {
        let tmp = TempDir::new().unwrap();
        let cfg = ObsidianConfig {
            vault_path: tmp.path().to_path_buf(),
            notes_subdir: "Clases".into(),
        };
        let v = ObsidianVault::new(&cfg);
        v.ensure().unwrap();
        assert!(v.notes_dir().exists());
    }

    #[test]
    fn materia_dir_slugifies_accents() {
        let tmp = TempDir::new().unwrap();
        let cfg = ObsidianConfig {
            vault_path: tmp.path().to_path_buf(),
            notes_subdir: "Clases".into(),
        };
        let v = ObsidianVault::new(&cfg);
        let d = v.materia_dir("Cálculo II").unwrap();
        assert!(d.exists());
        let name = d.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, "Calculo II");
    }

    #[test]
    fn write_note_goes_into_materia_dir() {
        let tmp = TempDir::new().unwrap();
        let cfg = ObsidianConfig {
            vault_path: tmp.path().to_path_buf(),
            notes_subdir: "Clases".into(),
        };
        let v = ObsidianVault::new(&cfg);
        let path = v
            .write_note(
                "Física",
                "Newton",
                NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
                "# Física — Newton\n\ncontenido",
            )
            .unwrap();
        assert!(path.starts_with(v.materia_dir("Física").unwrap()));
    }

    #[test]
    fn update_moc_lists_all_notes() {
        let tmp = TempDir::new().unwrap();
        let cfg = ObsidianConfig {
            vault_path: tmp.path().to_path_buf(),
            notes_subdir: "Clases".into(),
        };
        let v = ObsidianVault::new(&cfg);
        v.write_note(
            "Mates",
            "Tema A",
            NaiveDate::from_ymd_opt(2026, 8, 30).unwrap(),
            "# Mates — Tema A",
        )
        .unwrap();
        v.write_note(
            "Mates",
            "Tema B",
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            "# Mates — Tema B",
        )
        .unwrap();
        let moc = v.update_moc("Mates").unwrap();
        let content = std::fs::read_to_string(&moc).unwrap();
        assert!(content.contains("Tema A"));
        assert!(content.contains("Tema B"));
        // B es más reciente, debe aparecer antes que A.
        let pos_b = content.find("Tema B").unwrap();
        let pos_a = content.find("Tema A").unwrap();
        assert!(pos_b < pos_a, "MOC debe ordenar cronológicamente inverso");
    }

    #[test]
    fn parse_date_from_filename_works() {
        assert_eq!(
            parse_date_from_filename("2026-09-01-calculo-limites.md"),
            Some(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap())
        );
        assert_eq!(parse_date_from_filename("random.md"), None);
        assert_eq!(parse_date_from_filename("_MOC.md"), None);
    }

    #[test]
    fn slugify_materia_preserves_spaces() {
        assert_eq!(slugify_materia("Cálculo II"), "Calculo II");
        assert_eq!(slugify_materia("Historia Universal"), "Historia Universal");
        // Caracteres problemáticos se reemplazan por '-' y luego se colapsan.
        assert_eq!(slugify_materia("Prog/Avanzada"), "Prog Avanzada");
        // Trim de guiones.
        assert_eq!(slugify_materia("  X  "), "X");
    }
}
